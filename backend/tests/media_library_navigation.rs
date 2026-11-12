use axum::{Json, Router, extract::Query, http::StatusCode, response::IntoResponse, routing::get};
use emby_toolkit::{AppState, auth::SessionSigner, logs::LogStore, media};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, RwLock};

async fn request(
    mode: &'static str,
    uri: &str,
) -> (StatusCode, Value, Vec<HashMap<String, String>>) {
    let requests = Arc::new(Mutex::new(vec![]));
    let observed = requests.clone();
    let app = Router::new()
        .route("/Users/admin/Items", get(move |Query(query): Query<HashMap<String, String>>| {
            let requests = requests.clone();
            async move {
                requests.lock().await.push(query.clone());
                if query.contains_key("NameStartsWithOrGreater") {
                    if mode == "forbidden" { return StatusCode::FORBIDDEN.into_response(); }
                    if mode == "failure" { return (StatusCode::SERVICE_UNAVAILABLE, [("Retry-After", "1")]).into_response(); }
                    return Json(match mode {
                        "invalid-count" => json!({"Items":[],"TotalRecordCount":"unknown"}),
                        "changed-count" => json!({"Items":[],"TotalRecordCount":250}),
                        "empty" => json!({"Items":[],"TotalRecordCount":0}),
                        _ => json!({"Items":[],"TotalRecordCount":148}),
                    }).into_response();
                }
                if query.get("Limit").is_some_and(|v| v == "0") {
                    return Json(json!({"Items":[],"TotalRecordCount":240})).into_response();
                }
                let start = query["StartIndex"].parse::<u64>().unwrap();
                let limit = query["Limit"].parse::<u64>().unwrap();
                let items: Vec<Value> = (start..(start + limit).min(240))
                    .map(|index| json!({"Id":index.to_string(),"Name":format!("Movie {index}"),"Type":"Movie","CommunityRating":8.2,"CriticRating":91,"OfficialRating":"PG-13","CustomRating":"青少年","UnrecognizedNativeField":{"retained":true}})).collect();
                Json(json!({"Items":items,"TotalRecordCount":240})).into_response()
            }
        }))
        .route("/emby/Users/admin/Views", get(|| async { Json(json!({"Items":[
            {"Id":"movies","Name":"电影","CollectionType":"movies"}
        ]})) }));
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
    let response = media::router()
        .with_state(state)
        .oneshot(
            axum::http::Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    server.abort();
    let requests = observed.lock().await.clone();
    (status, value, requests)
}

#[tokio::test]
async fn letter_navigation_keeps_the_full_library_and_anchors_inside_a_page() {
    let (status, value, requests) = request(
        "ok",
        "/api/media_center/libraries/movies/items?page_size=30&letter=d&search=Movie",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["anchor_index"], 92);
    assert_eq!(value["start_index"], 90);
    assert_eq!(value["total"], 240);
    assert_eq!(value["anchor_found"], true);
    assert_eq!(value["items"][2]["Id"], "92");
    for item in value["items"].as_array().unwrap() {
        assert!(item.get("CommunityRating").is_none());
        assert!(item.get("CriticRating").is_none());
        assert_eq!(item["OfficialRating"], "PG-13");
        assert_eq!(item["CustomRating"], "青少年");
        assert_eq!(item["UnrecognizedNativeField"], json!({"retained":true}));
    }
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1]["NameStartsWithOrGreater"], "D");
    assert!(!requests[2].contains_key("NameStartsWithOrGreater"));
    for query in requests {
        assert_eq!(query["SearchTerm"], "Movie");
        assert_eq!(query["SortBy"], "SortName");
        assert_eq!(query["EnableTotalRecordCount"], "true");
        assert!(
            query
                .get("Fields")
                .is_none_or(|fields| !fields.split(',').any(|field| field == "CommunityRating"))
        );
    }
}

#[tokio::test]
async fn letter_query_failures_are_visible_instead_of_silently_returning_the_first_page() {
    for mode in ["forbidden", "failure", "invalid-count", "changed-count"] {
        let (status, value, requests) =
            request(mode, "/api/media_center/libraries/movies/items?letter=D").await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{mode}: {value}");
        assert!(value["error"].is_string());
        assert_eq!(requests.len(), if mode == "failure" { 7 } else { 2 });
        assert!(
            requests.iter().all(|query| query["Limit"] == "0"),
            "must not fetch a misleading first page"
        );
    }
}

#[tokio::test]
async fn absent_letter_returns_an_explicit_notice_and_a_usable_first_page() {
    let (status, value, _) =
        request("empty", "/api/media_center/libraries/movies/items?letter=Z").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["anchor_found"], false);
    assert_eq!(value["anchor_index"], 0);
    assert_eq!(value["items"][0]["Id"], "0");
}

#[tokio::test]
async fn following_pages_use_the_explicit_position_without_repeating_letter_counts() {
    let (status, value, requests) = request(
        "ok",
        "/api/media_center/libraries/movies/items?page_size=30&start_index=180",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["start_index"], 180);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["StartIndex"], "180");
}
