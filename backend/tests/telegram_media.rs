use axum::{Json, Router, body::Bytes, extract::State, http::HeaderMap, routing::post};
use base64::Engine;
use emby_toolkit::telegram_media;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::test]
async fn native_images_and_files_preserve_topics_buttons_and_local_source() {
    let calls = Arc::new(Mutex::new(Vec::<(String, Vec<u8>)>::new()));
    let app = Router::new()
        .route(
            "/bottest/{method}",
            post(
                |State(calls): State<Arc<Mutex<Vec<(String, Vec<u8>)>>>>,
                 headers: HeaderMap,
                 body: Bytes| async move {
                    calls.lock().await.push((
                        headers["content-type"].to_str().unwrap().into(),
                        body.to_vec(),
                    ));
                    Json(json!({"ok":true,"result":{"message_id":1}}))
                },
            ),
        )
        .with_state(calls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    let png = b"\x89PNG\r\n\x1a\nfixture";
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("lyrics.lrc");
    tokio::fs::write(&file, b"[00:01]original").await.unwrap();
    let photo = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    );
    telegram_media::request(&client,&base,"test","sendPhoto",&json!({"chat_id":"123","native_image":true,"photo":photo,"caption":"图片说明","message_thread_id":7,"reply_markup":{"inline_keyboard":[[{"text":"查看","callback_data":"open"}]]}})).await.unwrap();
    telegram_media::request(
        &client,
        &base,
        "test",
        "sendDocument",
        &json!({"chat_id":"123","document":{"path":file,"name":"歌曲.lrc"},"message_thread_id":7}),
    )
    .await
    .unwrap();
    telegram_media::request(
        &client,
        &base,
        "test",
        "sendMessage",
        &json!({"chat_id":"123","text":"普通消息"}),
    )
    .await
    .unwrap();
    let observed = calls.lock().await;
    for (headers, body) in observed.iter().take(2) {
        assert!(headers.starts_with("multipart/form-data"));
        let text = String::from_utf8_lossy(body);
        assert!(text.contains("message_thread_id") && text.contains("\r\n7\r\n"));
        assert!(!text.contains("native_image"));
    }
    let first = String::from_utf8_lossy(&observed[0].1);
    assert!(
        first.contains("image/png") && first.contains("reply_markup") && first.contains("图片说明")
    );
    assert!(String::from_utf8_lossy(&observed[1].1).contains("[00:01]original"));
    assert!(observed[2].0.starts_with("application/json"));
    assert_eq!(tokio::fs::read(&file).await.unwrap(), b"[00:01]original");
    drop(observed);
    assert!(
        telegram_media::request(
            &client,
            &base,
            "test",
            "sendDocument",
            &json!({"document":{"data":"bad base64"}})
        )
        .await
        .is_err()
    );
    assert_eq!(calls.lock().await.len(), 3);
    server.abort();
}
