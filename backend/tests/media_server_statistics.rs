use axum::{
    Json, Router,
    extract::Query,
    http::{HeaderMap, StatusCode},
    routing::get,
};
use emby_toolkit::{AppState, auth::SessionSigner, dashboard, logs::LogStore};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, RwLock};
use tower::ServiceExt;

async fn card_fixture(counts: Value, status: StatusCode, mode: &str) -> (StatusCode, Value, usize) {
    let requests = Arc::new(Mutex::new(0));
    let observed = requests.clone();
    let delayed = mode == "delayed";
    let app = Router::new().route(
        "/Items/Counts",
        get(
            move |Query(query): Query<HashMap<String, String>>, headers: HeaderMap| {
                let counts = counts.clone();
                let requests = requests.clone();
                async move {
                    assert!(
                        query.is_empty(),
                        "MP uses server totals without a UserId filter"
                    );
                    assert_eq!(headers["X-Emby-Token"], "fixture");
                    *requests.lock().await += 1;
                    if delayed {
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }
                    (status, Json(counts))
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let db = sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap();
    db.close().await;
    let state = AppState {
        db,
        config: Arc::new(RwLock::new(
            json!({"emby_server_url":upstream,"emby_api_key":"fixture"}),
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
    let router = if mode == "unauthenticated" {
        emby_toolkit::router(state)
    } else {
        dashboard::router().with_state(state)
    };
    let response = router
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/media_server/statistics")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let code = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    server.abort();
    let requests = *observed.lock().await;
    (code, value, requests)
}

#[tokio::test]
async fn server_card_reads_both_counts_from_one_native_request() {
    let (status, value, requests) = card_fixture(
        json!({"MovieCount":11,"SeriesCount":7,"MusicAlbumCount":3,"SongCount":99}),
        StatusCode::OK,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(requests, 1);
    assert_eq!(
        value,
        json!({"statistics":{
            "movie_count":{"count":11},"tv_count":{"count":7}
        }})
    );
}

#[tokio::test]
async fn server_card_preserves_valid_zero_and_excludes_music_counts() {
    let (status, value, requests) = card_fixture(
        json!({"MovieCount":0,"SeriesCount":0,"MusicAlbumCount":99,"AlbumCount":99,"SongCount":999}),
        StatusCode::OK,
        "",
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(requests, 1);
    assert_eq!(
        value,
        json!({"statistics":{
            "movie_count":{"count":0},"tv_count":{"count":0}
        }})
    );
}

#[tokio::test]
async fn server_card_failed_or_invalid_responses_do_not_invent_zero() {
    for (counts, status) in [
        (json!([]), StatusCode::OK),
        (
            json!({"MovieCount":"11","SeriesCount":true,"SongCount":-1}),
            StatusCode::OK,
        ),
        (json!({"MovieCount":1.5,"SeriesCount":null}), StatusCode::OK),
        (
            json!({"error":"unavailable"}),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let (status, value, requests) = card_fixture(counts, status, "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            requests, 1,
            "a card refresh must not schedule implicit native retries"
        );
        for key in ["movie_count", "tv_count"] {
            assert!(value["statistics"][key]["count"].is_null());
            assert!(value["statistics"][key]["error"].is_string());
        }
    }
}

#[tokio::test]
async fn server_card_statistics_keep_the_existing_administrator_gate() {
    let (status, _, requests) = card_fixture(json!({}), StatusCode::OK, "unauthenticated").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(requests, 0);
}

#[tokio::test]
async fn server_card_statistics_stop_waiting_when_the_native_service_stalls() {
    let started = std::time::Instant::now();
    let (status, value, requests) = card_fixture(json!({}), StatusCode::OK, "delayed").await;
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "the card must not wait for the stalled native response"
    );
    assert_eq!(status, StatusCode::OK);
    assert_eq!(requests, 1);
    for key in ["movie_count", "tv_count"] {
        assert!(value["statistics"][key]["count"].is_null());
        assert!(
            value["statistics"][key]["error"]
                .as_str()
                .unwrap()
                .contains("超时")
        );
    }
}
