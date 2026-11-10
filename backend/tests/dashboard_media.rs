use axum::{Json, Router, extract::Query, routing::get};
use emby_toolkit::{AppState, auth::SessionSigner, dashboard, logs::LogStore};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, RwLock};

async fn fixture(failure: &str) -> (Value, Vec<HashMap<String, String>>) {
    let requests = Arc::new(Mutex::new(vec![]));
    let observed = requests.clone();
    let failure = failure.to_string();
    let mode = failure.clone();
    let app = Router::new()
        .route("/Users/admin/Items", get(move |Query(query): Query<HashMap<String,String>>| {
            let requests = requests.clone(); let failure = failure.clone();
            async move {
                requests.lock().await.push(query.clone());
                if query.get("Limit").is_some_and(|v| v == "0") {
                    let kind = query.get("IncludeItemTypes").unwrap();
                    if failure == "count" && kind == "Episode" { return Json(json!({"TotalRecordCount":"bad"})); }
                    return Json(json!({"Items":[],"TotalRecordCount":match kind.as_str(){"Movie"=>11,"Series"=>7,"Episode"=>42,_=>3}}));
                }
                if failure == "recent" { return Json(json!({"Items":false})); }
                let id = if failure == "unsafe-id" { "../bad" } else { "101" };
                Json(json!({"Items":[{"Id":id,"Name":"新入库电影","Type":"Movie","DateCreated":"2026-11-10T01:00:00Z","ImageTags":{"Primary":"emby-poster"},"Path":"must-not-reach-ui","ProviderIds":{"Tmdb":"999"}}]}))
            }
        }))
        .route("/Users/admin/Views", get(move || {
            let mode=mode.clone(); async move {
                if mode == "libraries" { return Json(json!({"Items":null})); }
                Json(json!({"Items":[{"Id":"movies","Name":"我的电影","CollectionType":"movies","ImageTags":{"Primary":"emby-library-composite"}},{"Id":"ignored-boxset","Name":"Collections","CollectionType":"boxsets"}]}))
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let state = AppState {
        db: sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap(),
        config: Arc::new(RwLock::new(
            json!({"emby_server_url":upstream,"emby_api_key":"fixture","emby_user_id":"admin"}),
        )),
        http: reqwest::Client::new(),
        sessions: Arc::new(SessionSigner::load(root.path()).unwrap()),
        recovery_tokens: Arc::default(),
        data_dir: root.path().into(),
        notifications: Arc::default(),
        webhooks: Arc::default(),
        ai: Arc::default(),
        jobs: Arc::default(),
        logs: LogStore::open(&root.path().join("logs")).unwrap(),
        p115: Arc::default(),
        strm: Arc::default(),
        douban: Arc::default(),
        docker: Arc::default(),
    };
    use tower::ServiceExt;
    let response = dashboard::router()
        .with_state(state)
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/dashboard/media")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let value: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    server.abort();
    let observed = observed.lock().await.clone();
    (value, observed)
}

#[tokio::test]
async fn dashboard_reads_emby_counts_latest_and_library_primary_artwork() {
    let (value, requests) = fixture("").await;
    assert_eq!(value["statistics"]["movie_count"]["count"], 11);
    assert_eq!(value["statistics"]["episode_count"]["count"], 42);
    assert_eq!(value["statistics"]["music_count"]["count"], 3);
    assert_eq!(value["recent"][0]["id"], "101");
    assert_eq!(value["recent"][0]["image_tag"], "emby-poster");
    assert!(value["recent"][0].get("Path").is_none());
    assert!(value["recent"][0].get("ProviderIds").is_none());
    assert_eq!(value["libraries"].as_array().unwrap().len(), 1);
    assert_eq!(value["libraries"][0]["image_tag"], "emby-library-composite");
    let recent = requests.iter().find(|q| q.get("SortBy").is_some()).unwrap();
    assert_eq!(recent["SortBy"], "DateCreated");
    assert_eq!(recent["SortOrder"], "Descending");
    assert_eq!(recent["Limit"], "24");
    assert_eq!(
        requests
            .iter()
            .filter(|q| q.get("Limit").is_some_and(|v| v == "0"))
            .count(),
        4
    );
    assert_eq!(value["errors"], json!({}));
}

#[tokio::test]
async fn a_failed_statistic_is_unknown_and_does_not_hide_other_media_data() {
    let (value, _) = fixture("count").await;
    assert!(value["statistics"]["episode_count"]["count"].is_null());
    assert!(value["statistics"]["episode_count"]["error"].is_string());
    assert_eq!(value["statistics"]["movie_count"]["count"], 11);
    assert_eq!(value["recent"][0]["id"], "101");
}

#[tokio::test]
async fn failed_media_sections_are_explicit_errors_not_confirmed_empty_lists() {
    for key in ["recent", "libraries"] {
        let (value, _) = fixture(key).await;
        assert!(value[key].is_null());
        assert!(value["errors"][key].is_string());
        assert_eq!(value["statistics"]["movie_count"]["count"], 11);
    }
    let (value, _) = fixture("unsafe-id").await;
    assert!(value["recent"].is_null());
    assert!(value["errors"]["recent"].is_string());
}
