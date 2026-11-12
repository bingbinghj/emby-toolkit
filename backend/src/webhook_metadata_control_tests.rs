use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

async fn controlled_metadata_update(
    action: &str,
    blocked_request: usize,
) -> (Value, String, usize) {
    let fixture = Fixture::new().await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let requests = Arc::new(AtomicUsize::new(0));
    let fake = Router::new()
        .route(
            "/Users/user/Items/movie",
            axum::routing::get({
                let entered = entered.clone();
                let release = release.clone();
                let requests = requests.clone();
                move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    let requests = requests.clone();
                    async move {
                        if requests.fetch_add(1, Ordering::SeqCst) == blocked_request {
                            entered.notify_one();
                            release.notified().await;
                        }
                        Json(
                            json!({"Id":"movie","Type":"Movie","Path":"/library/movie.strm",
                            "ProviderIds":{"Tmdb":"101"},"Name":"新标题"}),
                        )
                    }
                }
            }),
        )
        .route(
            "/Library/VirtualFolders",
            axum::routing::get(|| async { Json(json!([])) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });
    fixture
        .state
        .save_config(&json!({"emby_server_url":base,"emby_api_key":"mock","emby_user_id":"user"}))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO media_metadata(tmdb_id,item_type,title) VALUES('101','Movie','旧标题')",
    )
    .execute(&fixture.state.db)
    .await
    .unwrap();
    let work = fixture.state.clone();
    let id = fixture
        .state
        .jobs
        .submit("元数据镜像控制", move |context| async move {
            process_emby(
                &work,
                &json!({"source":"emby","id":"movie","event":"metadata.update",
                    "original":{"Id":"movie"}}),
                &context,
            )
            .await
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    fixture.state.jobs.control(Some(&id), action).await.unwrap();
    release.notify_one();
    let observed = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let result = fixture.state.jobs.result(&id).await.unwrap();
            if result["state"] == "paused" || result["is_running"] == false {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let title: String = sqlx::query_scalar("SELECT title FROM media_metadata WHERE tmdb_id='101'")
        .fetch_one(&fixture.state.db)
        .await
        .unwrap();
    if observed["state"] == "paused" {
        fixture
            .state
            .jobs
            .control(Some(&id), "resume")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while fixture.state.jobs.result(&id).await.unwrap()["is_running"] == true {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let completed = fixture.state.jobs.result(&id).await.unwrap();
        assert_eq!(completed["result"]["updated"], 1, "{completed}");
    }
    fixture.close().await;
    server.abort();
    (observed, title, requests.load(Ordering::SeqCst))
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn metadata_update_pauses_before_mirroring_and_resumes_the_same_job() {
    let (observed, title, requests) = controlled_metadata_update("pause", 0).await;
    assert_eq!(observed["state"], "paused", "{observed}");
    assert_eq!(title, "旧标题");
    assert_eq!(requests, 2);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn metadata_update_stopped_during_mirror_read_does_not_write_the_database() {
    let (observed, title, requests) = controlled_metadata_update("stop", 1).await;
    assert_eq!(observed["state"], "cancelled", "{observed}");
    assert_eq!(title, "旧标题");
    assert_eq!(requests, 2);
}
