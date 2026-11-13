use axum::{Router, body::Body, http::StatusCode, routing::get};
use emby_toolkit::emby;
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;

#[tokio::test]
async fn retry_delays_leave_slots_available_but_unread_responses_keep_their_slots() {
    let retries = Arc::new(Semaphore::new(0));
    let observed = retries.clone();
    let body_ready = Arc::new(Semaphore::new(0));
    let release_body = body_ready.clone();
    let app = Router::new()
        .route(
            "/retry",
            get(move || {
                let observed = observed.clone();
                async move {
                    observed.add_permits(1);
                    (StatusCode::SERVICE_UNAVAILABLE, [("Retry-After", "3600")])
                }
            }),
        )
        .route("/healthy", get(|| async { "healthy" }))
        .route(
            "/body",
            get(move || {
                let release_body = release_body.clone();
                async move {
                    Body::from_stream(futures_util::stream::once(async move {
                        release_body.acquire().await.unwrap().forget();
                        Ok::<_, std::io::Error>("healthy")
                    }))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    let mut pending = Vec::new();
    for _ in 0..10 {
        let request = client.get(format!("{base}/retry"));
        pending.push(tokio::spawn(async move { emby::send(request).await }));
    }
    tokio::time::timeout(Duration::from_secs(2), retries.acquire_many(10))
        .await
        .unwrap()
        .unwrap()
        .forget();
    let healthy = tokio::time::timeout(
        Duration::from_millis(500),
        emby::send(client.get(format!("{base}/healthy"))),
    )
    .await;
    for request in pending {
        request.abort();
        let _ = request.await;
    }
    let response = healthy.expect("Retry-After sleeps must not occupy all Emby request slots");
    assert_eq!(response.unwrap().text().await.unwrap(), "healthy");

    // Returning response headers must not release a slot before its body is read.
    let mut unread = Vec::new();
    let reading = emby::send(client.get(format!("{base}/body")))
        .await
        .unwrap();
    for _ in 0..9 {
        unread.push(
            emby::send(client.get(format!("{base}/healthy")))
                .await
                .unwrap(),
        );
    }
    let reading = tokio::spawn(async move { reading.text().await });
    let request = client.get(format!("{base}/healthy"));
    let mut waiting = tokio::spawn(async move { emby::send(request).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(150), &mut waiting)
            .await
            .is_err(),
        "unread response bodies must continue to occupy their request slots"
    );
    body_ready.add_permits(1);
    assert_eq!(reading.await.unwrap().unwrap(), "healthy");
    let response = tokio::time::timeout(Duration::from_millis(500), waiting)
        .await
        .expect("reading one response body should make its slot available")
        .unwrap()
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "healthy");
    drop(unread);
    server.abort();
}
