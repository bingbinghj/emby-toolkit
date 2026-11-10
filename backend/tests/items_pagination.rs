use axum::{Json, Router, extract::Query, routing::get};
use emby_toolkit::{AppState, auth::SessionSigner, emby, jobs::JobManager, logs::LogStore};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, RwLock};

async fn scan(pages: Vec<Value>) -> (Value, Vec<usize>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = requests.clone();
    let app = Router::new().route(
        "/Items",
        get(move |Query(query): Query<HashMap<String, String>>| {
            let requests = requests.clone();
            let pages = pages.clone();
            async move {
                let mut requests = requests.lock().await;
                let page = pages[requests.len().min(pages.len() - 1)].clone();
                requests.push(query["StartIndex"].parse::<usize>().unwrap());
                Json(page)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let state = AppState {
        db: sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap(),
        config: Arc::new(RwLock::new(json!({
            "emby_server_url":format!("http://{address}"),"emby_api_key":"fixture"
        }))),
        http: reqwest::Client::new(),
        sessions: Arc::new(SessionSigner::load(root.path()).unwrap()),
        recovery_tokens: Arc::new(RwLock::new(HashMap::new())),
        data_dir: root.path().into(),
        notifications: Arc::default(),
        webhooks: Arc::default(),
        ai: Arc::default(),
        jobs: Arc::new(JobManager::default()),
        logs: LogStore::open(&root.path().join("logs")).unwrap(),
        p115: Arc::default(),
        strm: Arc::default(),
        douban: Arc::default(),
        docker: Arc::default(),
    };
    let work = state.clone();
    let id = state
        .jobs
        .submit("分页验收", move |context| async move {
            Ok(json!({"items":emby::items(&work,"library","Path",&context).await?}))
        })
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let result = state.jobs.result(&id).await.unwrap();
            if result["is_running"] == false {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pagination must terminate, including exhausted retries");
    server.abort();
    let requests = observed.lock().await.clone();
    (result, requests)
}

#[tokio::test]
async fn short_pages_continue_until_the_declared_total_is_reached() {
    let (result, requests) = scan(vec![
        json!({"Items":[{"Id":"one"}],"TotalRecordCount":2}),
        json!({"Items":[{"Id":"two"}],"TotalRecordCount":2}),
    ])
    .await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(requests, [0, 1]);
    assert_eq!(result["result"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(result["result"]["items"][1]["Id"], "two");
    assert_eq!(result["result"]["items"][1]["_SourceLibraryId"], "library");
}

#[tokio::test]
async fn an_empty_incomplete_page_is_retried_at_the_same_start_index() {
    for (pages, expected_starts) in [
        (
            vec![
                json!({"Items":[],"TotalRecordCount":2}),
                json!({"Items":[{"Id":"one"}],"TotalRecordCount":2}),
                json!({"Items":[{"Id":"two"}],"TotalRecordCount":2}),
            ],
            vec![0, 0, 1],
        ),
        (
            vec![
                json!({"Items":[{"Id":"one"}],"TotalRecordCount":2}),
                json!({"Items":[]}),
                json!({"Items":[{"Id":"two"}]}),
            ],
            vec![0, 1, 1],
        ),
    ] {
        let (result, requests) = scan(pages).await;
        assert_eq!(result["state"], "completed", "{result}");
        assert_eq!(requests, expected_starts);
        assert_eq!(result["result"]["items"].as_array().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn an_unrecoverable_empty_page_reports_incomplete_pagination() {
    let (result, requests) = scan(vec![
        json!({"Items":[{"Id":"one"}],"TotalRecordCount":2}),
        json!({"Items":[],"TotalRecordCount":2}),
    ])
    .await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(requests, [0, 1, 1, 1]);
    assert!(
        result["result"]["error"]
            .as_str()
            .unwrap()
            .contains("分页不完整")
    );
}

#[tokio::test]
async fn an_omitted_total_on_later_pages_does_not_hide_known_remaining_items() {
    let (result, requests) = scan(vec![
        json!({"Items":[{"Id":"one"}],"TotalRecordCount":3}),
        json!({"Items":[{"Id":"two"}]}),
        json!({"Items":[{"Id":"three"}]}),
    ])
    .await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(requests, [0, 1, 2]);
    assert_eq!(result["result"]["items"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn short_pages_without_a_total_and_declared_empty_libraries_still_complete() {
    for page in [
        json!({"Items":[{"Id":"one"}]}),
        json!({"Items":[],"TotalRecordCount":0}),
    ] {
        let (result, requests) = scan(vec![page]).await;
        assert_eq!(result["state"], "completed", "{result}");
        assert_eq!(requests, [0]);
    }
}

#[tokio::test]
async fn continuing_short_pages_still_rejects_duplicate_or_missing_ids() {
    for second in [json!({"Id":"one"}), json!({"Name":"missing"})] {
        let (result, requests) = scan(vec![
            json!({"Items":[{"Id":"one"}],"TotalRecordCount":2}),
            json!({"Items":[second],"TotalRecordCount":2}),
        ])
        .await;
        assert_eq!(result["state"], "failed", "{result}");
        assert_eq!(requests, [0, 1]);
        assert!(
            result["result"]["error"]
                .as_str()
                .unwrap()
                .contains("缺失或重复 ID")
        );
    }
}

#[tokio::test]
async fn malformed_or_underreported_totals_do_not_report_a_successful_scan() {
    for total in [json!(-1), json!("2"), json!(0)] {
        let (result, requests) = scan(vec![json!({
            "Items":[{"Id":"one"}],"TotalRecordCount":total
        })])
        .await;
        assert_eq!(result["state"], "failed", "{result}");
        assert_eq!(requests, [0]);
        assert!(
            result["result"]["error"]
                .as_str()
                .unwrap()
                .contains("分页总数无效")
        );
    }
}
