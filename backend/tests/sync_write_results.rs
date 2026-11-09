use axum::{
    Json, Router,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use emby_toolkit::{AppState, metadata, sync};
use serde_json::json;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and TMDb are localhost"]
async fn sync_counts_committed_rows_reports_sql_failures_and_preserves_failed_tmdb_metadata() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("sync_write_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = format!("SET search_path TO {schema}");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(move |connection, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&search).execute(connection).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let state = AppState::new(pool, temp.path().into()).await.unwrap();
    let items = vec![
        json!({"Id":"good","Type":"Movie","ProviderIds":{"Tmdb":"123"},"Name":"A movie","Path":"/media/good.mkv","CustomRating":"限制分级"}),
        json!({"Id":"bad","Type":"Movie","ProviderIds":{"Tmdb":"456"},"Name":"B movie","Path":"/media/bad.mkv","CustomRating":"不能写入的新分级"}),
        json!({"Id":"fallback","Type":"Movie","ProviderIds":{"Tmdb":"789"},"Name":"C movie","Path":"/media/fallback.mkv","CustomRating":"新自定义分级","OfficialRating":"PG-13","Genres":["剧情"],"Tags":["Emby新标签"],"CommunityRating":8.5}),
    ];
    let requested_fields = Arc::new(Mutex::new(Vec::<String>::new()));
    let requests = requested_fields.clone();
    let upstream = Router::new()
        .route("/Users/admin/Views", get(|| async { Json(json!({"Items":[{"Id":"library"}]})) }))
        .route("/Items", get(move |Query(query): Query<HashMap<String, String>>| {
            let items = items.clone();
            let requests = requests.clone();
            async move {
                let fields = query.get("Fields").cloned().unwrap_or_default();
                requests.lock().await.push(fields.clone());
                let items = items.into_iter().filter(|item| query.get("Ids").is_none_or(|ids|ids.split(',').any(|id|item["Id"]==id))).map(|mut item| {
                    // CustomRating is optional in the native DTO and requires Fields.
                    if !fields.split(',').any(|field|field=="CustomRating") { item.as_object_mut().unwrap().remove("CustomRating"); }
                    item
                }).collect::<Vec<_>>();
                Json(json!({"Items":items,"TotalRecordCount":items.len()}))
            }
        }))
        .route("/Library/VirtualFolders", get(|| async { Json(json!([])) }))
        .route("/movie/{id}", get(|Path(id):Path<String>|async move {
            if id=="789" { return StatusCode::FORBIDDEN.into_response(); }
            Json(json!({"id":id.parse::<i32>().unwrap(),"title":"Movie","original_title":"Movie","original_language":"en","runtime":if id=="123" {json!(90)} else {json!("not-an-integer")}})).into_response()
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"admin","tmdb_api_base_url":base,"tmdb_api_key":"fixture","ai_translate_on_sync":false,"p115_fingerprint_media_root_cid":"","libraries_to_ignore":[]})).await.unwrap();
    for (tmdb, native) in [("123", "good"), ("456", "bad"), ("789", "fallback")] {
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,in_library,emby_item_ids_json,asset_details_json,runtime_minutes,custom_rating) VALUES($1,'Movie','手工标题',true,$2,$3,15,'旧分级')")
            .bind(tmdb)
            .bind(json!([native]))
            .bind(json!([{"emby_item_id":native,"path":format!("/media/{native}.mkv")}]))
            .execute(&state.db)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE media_metadata SET production_companies_json='[{\"id\":1,\"name\":\"原制作公司\"}]',networks_json='[{\"id\":2,\"name\":\"原平台\"}]',directors_json='[{\"id\":3,\"name\":\"原导演\"}]',countries_json='[\"JP\"]',keywords_json='[{\"id\":4,\"name\":\"原关键词\"}]',official_rating_json='{\"US\":\"R\",\"JP\":\"G\"}',overview='原简介',genres_json='[{\"id\":5,\"name\":\"旧类型\"}]',tags_json='[\"旧标签\"]' WHERE tmdb_id='789'")
        .execute(&state.db).await.unwrap();
    let fallback_before = metadata::row(&state, "789", "Movie")
        .await
        .unwrap()
        .unwrap();
    let before = metadata::row(&state, "456", "Movie")
        .await
        .unwrap()
        .unwrap();
    let work = state.clone();
    let job = state
        .jobs
        .submit("同步写入验收", move |context| async move {
            sync::cache(&work, true, &context).await
        })
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let result = state.jobs.result(&job).await.unwrap();
            if result["is_running"] == false {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let good = metadata::row(&state, "123", "Movie")
        .await
        .unwrap()
        .unwrap();
    let bad = metadata::row(&state, "456", "Movie")
        .await
        .unwrap()
        .unwrap();
    let fallback = metadata::row(&state, "789", "Movie")
        .await
        .unwrap()
        .unwrap();
    state.jobs.shutdown().await;
    server.abort();
    state.db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["result"]["updated"], 2, "{result}");
    assert_eq!(result["result"]["offline"], 0, "{result}");
    let failures = result["result"]["failures"].as_array().unwrap();
    assert_eq!(failures.len(), 1, "{result}");
    let failure = failures[0].as_str().unwrap();
    assert!(
        failure.contains("Movie") && failure.contains("456") && failure.contains("22P02"),
        "{failure}"
    );
    assert!(
        !failure.contains("not-an-integer"),
        "SQL parameter values must not appear in the report"
    );
    assert_eq!(good["runtime_minutes"], 90);
    assert_eq!(good["custom_rating"], "限制分级");
    assert_eq!(good["title"], "手工标题");
    assert_eq!(
        bad, before,
        "a failed write must preserve the original record"
    );
    for field in [
        "production_companies_json",
        "networks_json",
        "directors_json",
        "countries_json",
        "keywords_json",
        "official_rating_json",
        "overview",
    ] {
        assert_eq!(
            fallback[field], fallback_before[field],
            "a failed TMDb read must preserve {field}"
        );
    }
    assert_eq!(fallback["title"], "手工标题");
    assert_eq!(fallback["runtime_minutes"], 15);
    assert_eq!(fallback["custom_rating"], "新自定义分级");
    assert_eq!(fallback["genres_json"], json!([{"id":0,"name":"剧情"}]));
    assert_eq!(fallback["tags_json"], json!(["Emby新标签"]));
    assert_eq!(fallback["rating"], 8.5);
    assert_eq!(fallback["emby_item_ids_json"], json!(["fallback"]));
    let warnings = result["result"]["warning_details"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{result}");
    assert!(warnings[0].as_str().unwrap().contains("789"), "{result}");
    assert!(
        requested_fields
            .lock()
            .await
            .iter()
            .filter(|fields| fields.contains("MediaStreams"))
            .all(|fields| fields.split(',').any(|field| field == "CustomRating"))
    );
}
