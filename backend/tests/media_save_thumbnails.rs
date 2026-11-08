mod native {
    // Reuse the real clear/JSON restore fixture without changing existing tests.
    include!("title_reverse_restore.rs");

    use axum::{body::Body, http::Request, response::Response};
    use emby_toolkit::media_save;

    #[derive(Clone)]
    struct Proxy {
        original_url: String,
        host: Host,
        original_settings: Value,
        extra: Arc<Mutex<HashMap<String, Value>>>,
        thumbnail_starts: Arc<AtomicUsize>,
        metadata_ids: Arc<Mutex<Vec<String>>>,
        readbacks: Arc<AtomicUsize>,
        thumbnail_task: Arc<Mutex<Value>>,
        reject_thumbnail: Arc<AtomicBool>,
        reject_clear: Arc<AtomicBool>,
        skip_saved: Arc<AtomicBool>,
    }

    impl Proxy {
        async fn items(&self) -> Vec<Value> {
            let mut items = vec![self.host.item.lock().await.clone()];
            items.extend(self.extra.lock().await.values().cloned());
            items
        }

        async fn forward(&self, request: Request<Body>) -> Response {
            let url = format!("{}{}", self.original_url, request.uri());
            let (parts, body) = request.into_parts();
            let body = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
            let response = reqwest::Client::new()
                .request(parts.method, url)
                .headers(parts.headers)
                .body(body)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let headers = response.headers().clone();
            let mut output = Response::new(Body::from(response.bytes().await.unwrap()));
            *output.status_mut() = status;
            *output.headers_mut() = headers;
            output
        }
    }

    async fn proxy(State(proxy): State<Proxy>, request: Request<Body>) -> Response {
        use axum::response::IntoResponse;
        let path = request.uri().path().to_string();
        let query: HashMap<String, String> =
            url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
                .into_owned()
                .collect();
        if request.method() == axum::http::Method::GET {
            if path.starts_with("/Users/user/Items/") {
                if query.get("Fields").is_some_and(|fields| {
                    fields == "Id,Path,Type,Chapters,MediaSources,MediaStreams"
                }) {
                    proxy.readbacks.fetch_add(1, Ordering::SeqCst);
                }
                let id = path.rsplit('/').next().unwrap();
                if let Some(item) = proxy.extra.lock().await.get(id).cloned() {
                    return Json(item).into_response();
                }
            } else if path == "/ScheduledTasks" {
                let native = json!({"Id":"media","Key":"MediaInfoExtractTask","State":"Idle","LastExecutionResult":{"StartTimeUtc":proxy.host.starts.load(Ordering::SeqCst).to_string(),"Status":"Completed"}});
                return Json(json!([native, proxy.thumbnail_task.lock().await.clone()]))
                    .into_response();
            } else if path == "/System/Logs/embyserver.txt/Lines" {
                let mut lines = vec!["2026-11-08 00:00:00.000 fixture log anchor".to_string()];
                if proxy.thumbnail_starts.load(Ordering::SeqCst) > 0 {
                    let items = if proxy.skip_saved.load(Ordering::SeqCst) {
                        vec![]
                    } else {
                        proxy.items().await
                    };
                    lines.push(
                        "2026-11-08 00:00:01.000 VideoThumbnailExtract - Scheduled Task Execute"
                            .into(),
                    );
                    lines.push(format!(
                        "2026-11-08 00:00:01.001 VideoThumbnailExtract - Number of items: {}",
                        items.len()
                    ));
                    for (index, item) in items.iter().enumerate() {
                        lines.push(format!("2026-11-08 00:00:02.{index:03} VideoThumbnailExtract - Progress {}/{} - Task 1: {}",index+1,items.len(),item["Path"].as_str().unwrap()));
                    }
                    lines.push(
                        "2026-11-08 00:00:03.000 VideoThumbnailExtract - Scheduled Task Complete"
                            .into(),
                    );
                }
                lines.reverse();
                return Json(json!({"Items":lines,"TotalRecordCount":lines.len()})).into_response();
            } else if path == "/Library/VirtualFolders" {
                return Json(json!([{"ItemId":"library","Locations":[proxy.host.directory],"LibraryOptions":{"EnableChapterImageExtraction":true,"ThumbnailImagesIntervalSeconds":0}}])).into_response();
            } else if path == "/Items" {
                let items = proxy.items().await;
                let found = items
                    .into_iter()
                    .filter(|item| {
                        query.get("Path").is_some_and(|path| item["Path"] == *path)
                            || query
                                .get("Ids")
                                .is_some_and(|ids| ids.split(',').any(|id| item["Id"] == id))
                    })
                    .collect::<Vec<_>>();
                return Json(json!({"Items":found,"TotalRecordCount":found.len()})).into_response();
            }
        } else if request.method() == axum::http::Method::POST {
            if path.ends_with("/Refresh") {
                let id = path.split('/').nth(2).unwrap().to_string();
                assert_eq!(query["Recursive"], "false");
                assert!(proxy.readbacks.load(Ordering::SeqCst) > 0);
                assert_eq!(*proxy.host.settings.lock().await, proxy.original_settings);
                proxy.metadata_ids.lock().await.push(id);
                return StatusCode::NO_CONTENT.into_response();
            } else if path.ends_with("/ClearMediaInfo") {
                if proxy.reject_clear.load(Ordering::SeqCst) {
                    return StatusCode::BAD_REQUEST.into_response();
                }
                let id = path.split('/').nth(2).unwrap();
                let mut extra = proxy.extra.lock().await;
                if let Some(item) = extra.get_mut(id) {
                    item["MediaStreams"] = json!([]);
                    item["MediaSources"][0]["MediaStreams"] = json!([]);
                    item["Chapters"] = json!([]);
                    tokio::fs::remove_file(
                        proxy.host.directory.join(format!("{id}-mediainfo.json")),
                    )
                    .await
                    .unwrap();
                    return StatusCode::NO_CONTENT.into_response();
                }
            } else if path == "/ScheduledTasks/Running/media" {
                let response = proxy.forward(request).await;
                for (id, item) in proxy.extra.lock().await.iter_mut() {
                    let data: Value = serde_json::from_slice(
                        &tokio::fs::read(proxy.host.directory.join(format!("{id}-mediainfo.json")))
                            .await
                            .unwrap(),
                    )
                    .unwrap();
                    item["MediaStreams"] = data[0]["MediaSourceInfo"]["MediaStreams"].clone();
                    item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
                    item["Chapters"] = data[0]["Chapters"].clone();
                }
                return response;
            } else if path == "/ScheduledTasks/Running/thumbnail" {
                assert!(
                    proxy.host.starts.load(Ordering::SeqCst) > 0,
                    "JSON restoration must finish before thumbnail extraction"
                );
                assert!(
                    proxy.readbacks.load(Ordering::SeqCst) > 0,
                    "real media readback must finish before thumbnail extraction"
                );
                assert_eq!(*proxy.host.settings.lock().await, proxy.original_settings);
                let items = proxy.items().await;
                assert!(
                    items.iter().all(|item| item["Chapters"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|chapter| chapter["ImageTag"].is_null())),
                    "chapter images from before editing must have been lost on clear and JSON restore"
                );
                assert_eq!(proxy.metadata_ids.lock().await.len(), items.len());
                assert!(items.iter().all(|item| {
                    item["MediaStreams"]
                        .as_array()
                        .is_some_and(|streams| !streams.is_empty())
                }));
                let run = proxy.thumbnail_starts.fetch_add(1, Ordering::SeqCst) + 1;
                if proxy.reject_thumbnail.load(Ordering::SeqCst) {
                    return StatusCode::BAD_REQUEST.into_response();
                }
                if !proxy.skip_saved.load(Ordering::SeqCst) {
                    for item in std::iter::once(&mut *proxy.host.item.lock().await)
                        .chain(proxy.extra.lock().await.values_mut())
                    {
                        for chapter in item["Chapters"].as_array_mut().unwrap() {
                            chapter["ImageTag"] = json!("new-thumbnail");
                        }
                    }
                }
                *proxy.thumbnail_task.lock().await = json!({"Id":"thumbnail","Key":"VideoThumbnailExtractTask","State":"Idle","LastExecutionResult":{"StartTimeUtc":run.to_string(),"Status":"Completed"}});
                return StatusCode::NO_CONTENT.into_response();
            }
        }
        proxy.forward(request).await
    }

    struct ThumbnailFixture {
        native: Fixture,
        proxy: Proxy,
        server: tokio::task::JoinHandle<()>,
    }

    impl ThumbnailFixture {
        async fn new() -> Self {
            let native = Fixture::new(false).await;
            for chapter in native.host.item.lock().await["Chapters"]
                .as_array_mut()
                .unwrap()
            {
                chapter["ImageTag"] = json!("old-thumbnail");
            }
            let proxy = Proxy {
                original_url: native.state.config.read().await["emby_server_url"]
                    .as_str()
                    .unwrap()
                    .into(),
                host: native.host.clone(),
                original_settings: native.original_settings.clone(),
                extra: Arc::default(),
                thumbnail_starts: Arc::default(),
                metadata_ids: Arc::default(),
                readbacks: Arc::default(),
                thumbnail_task: Arc::new(Mutex::new(
                    json!({"Id":"thumbnail","Key":"VideoThumbnailExtractTask","State":"Idle","LastExecutionResult":{"StartTimeUtc":"old","Status":"Completed"}}),
                )),
                reject_thumbnail: Arc::default(),
                reject_clear: Arc::default(),
                skip_saved: Arc::default(),
            };
            let app = Router::new()
                .fallback(self::proxy)
                .with_state(proxy.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            native
                .state
                .save_config(&json!({"emby_server_url":format!("http://{address}")}))
                .await
                .unwrap();
            Self {
                native,
                proxy,
                server,
            }
        }

        async fn save(&self, body: Value, rebuilt: bool) -> Value {
            let state = self.native.state.clone();
            let task = state
                .jobs
                .clone()
                .submit_response("保存后提取回归", move |context| async move {
                    if rebuilt {
                        media_save::save_with_rebuilt(
                            &state,
                            "61616",
                            &body,
                            &std::collections::BTreeMap::from([(
                                "61616".into(),
                                body["mediainfo"].clone(),
                            )]),
                            &context,
                        )
                        .await
                    } else {
                        media_save::save(&state, "61616", &body, &context).await
                    }
                })
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(25), async {
                loop {
                    let result = self.native.state.jobs.result(&task).await.unwrap();
                    if result["is_running"] == false {
                        return result;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap()
        }

        async fn add_episode(&self) {
            let id = "61617";
            let path = self.native.directory.path().join(format!("{id}.strm"));
            let source = format!("/cd2/{id}.mkv");
            tokio::fs::write(&path, &source).await.unwrap();
            let info = self.native.original.clone();
            tokio::fs::write(
                self.native
                    .directory
                    .path()
                    .join(format!("{id}-mediainfo.json")),
                serde_json::to_vec(&info).unwrap(),
            )
            .await
            .unwrap();
            let mut item = self.native.host.item.lock().await.clone();
            item["Id"] = json!(id);
            item["Type"] = json!("Episode");
            item["Path"] = json!(path);
            item["MediaSources"][0]["Id"] = json!(format!("mediasource_{id}"));
            item["MediaSources"][0]["ItemId"] = json!(id);
            item["MediaSources"][0]["Path"] = json!(source);
            self.proxy.extra.lock().await.insert(id.into(), item);
            sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES('5646','Episode',$1,$2,$3,$3)")
                .bind(json!([id])).bind(json!([{"emby_item_id":id,"path":path,"size_bytes":info[0]["MediaSourceInfo"]["Size"]}])).bind(json!([null])).execute(&self.native.state.db).await.unwrap();
        }

        async fn close(self) {
            self.server.abort();
            self.native.close().await;
        }
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_video_rebuilds_images_after_restore_and_readback() {
        let f = ThumbnailFixture::new().await;
        let result = f.save(json!({"mediainfo":f.native.original}), false).await;
        let starts = f.proxy.thumbnail_starts.load(Ordering::SeqCst);
        let items = f.proxy.items().await;
        f.close().await;
        assert_eq!(result["state"], "completed", "{result}");
        assert_eq!(
            starts, 1,
            "saved video must rebuild chapter images lost during clear"
        );
        assert_eq!(
            result["result"]["video_thumbnails"]["failed"], 0,
            "{result}"
        );
        assert!(
            items[0]["Chapters"]
                .as_array()
                .unwrap()
                .iter()
                .all(|chapter| chapter["ImageTag"] == "new-thumbnail")
        );
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_batch_extracts_once_after_both_targets_are_saved() {
        let f = ThumbnailFixture::new().await;
        f.add_episode().await;
        let mut info = f.native.original.clone();
        info[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("DVD粤语");
        let stream = &f.native.original[0]["MediaSourceInfo"]["MediaStreams"][1];
        let result=f.save(json!({"mediainfo":info,"target_item_ids":["61617"],"stream_patches":[{"identity":{"type":"Audio","ordinal":1,"title":stream["Title"],"language":stream["Language"],"codec":stream["Codec"],"channels":stream["Channels"]},"changes":{"Title":"DVD粤语"}}]}),false).await;
        assert_eq!(result["state"], "completed", "{result}");
        assert_eq!(
            f.proxy.thumbnail_starts.load(Ordering::SeqCst),
            1,
            "one native extraction must serve the whole successful batch"
        );
        assert_eq!(f.proxy.metadata_ids.lock().await.len(), 2);
        assert_eq!(
            result["result"]["video_thumbnails"]["extraction"]["verified"], 2,
            "{result}"
        );
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_failed_clear_does_not_start_image_extraction() {
        let f = ThumbnailFixture::new().await;
        f.proxy.reject_clear.store(true, Ordering::SeqCst);
        let result = f.save(json!({"mediainfo":f.native.original}), false).await;
        assert_eq!(result["state"], "failed", "{result}");
        assert_eq!(f.proxy.thumbnail_starts.load(Ordering::SeqCst), 0);
        assert!(f.proxy.metadata_ids.lock().await.is_empty());
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_thumbnail_failure_keeps_verified_media_save_successful() {
        let f = ThumbnailFixture::new().await;
        f.proxy.reject_thumbnail.store(true, Ordering::SeqCst);
        let result = f.save(json!({"mediainfo":f.native.original}), false).await;
        let starts = f.proxy.thumbnail_starts.load(Ordering::SeqCst);
        let items = f.proxy.items().await;
        f.close().await;
        assert_eq!(result["state"], "completed_with_warnings", "{result}");
        assert_eq!(result["result_status"], 200, "{result}");
        assert_eq!(starts, 1);
        assert_eq!(
            result["result"]["video_thumbnails"]["failed"], 1,
            "{result}"
        );
        assert!(result["result"]["warnings"].as_u64().unwrap() > 0);
        assert!(result["result"]["failures"].as_array().unwrap().is_empty());
        assert!(
            items[0]["MediaStreams"]
                .as_array()
                .unwrap()
                .iter()
                .any(|stream| stream["Type"] == "Video")
        );
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_audio_only_does_not_start_video_extraction() {
        let f = ThumbnailFixture::new().await;
        let mut info = f.native.original.clone();
        let streams = info[0]["MediaSourceInfo"]["MediaStreams"]
            .as_array_mut()
            .unwrap();
        streams.retain(|stream| stream["Type"] == "Audio");
        {
            let mut item = f.native.host.item.lock().await;
            item["Type"] = json!("Audio");
            item["MediaStreams"] = json!(streams);
            item["MediaSources"][0]["MediaStreams"] = json!(streams);
        }
        tokio::fs::write(
            f.native.directory.path().join("61616-mediainfo.json"),
            serde_json::to_vec(&info).unwrap(),
        )
        .await
        .unwrap();
        let result = f.save(json!({"mediainfo":info}), false).await;
        assert_eq!(result["state"], "completed", "{result}");
        assert_eq!(f.proxy.thumbnail_starts.load(Ordering::SeqCst), 0);
        assert_eq!(
            result["result"]["video_thumbnails"]["state"], "not_requested",
            "{result}"
        );
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_rebuilt_path_does_not_duplicate_extraction() {
        let f = ThumbnailFixture::new().await;
        let result = f.save(json!({"mediainfo":f.native.original}), true).await;
        assert_eq!(result["state"], "completed", "{result}");
        assert_eq!(f.proxy.thumbnail_starts.load(Ordering::SeqCst), 0);
        assert!(f.proxy.metadata_ids.lock().await.is_empty());
        assert_eq!(
            result["result"]["video_thumbnails"]["state"], "not_requested",
            "{result}"
        );
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_automatic_titles_do_not_duplicate_extraction() {
        let f = ThumbnailFixture::new().await;
        let result = f.native.run().await;
        assert_eq!(result["state"], "completed", "{result}");
        assert_eq!(result["result"]["completed"], true, "{result}");
        assert_eq!(f.proxy.thumbnail_starts.load(Ordering::SeqCst), 0);
        assert!(f.proxy.metadata_ids.lock().await.is_empty());
        f.close().await;
    }
    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_native_scope_skip_does_not_hide_missing_saved_images() {
        let f = ThumbnailFixture::new().await;
        f.proxy.skip_saved.store(true, Ordering::SeqCst);
        let result = f.save(json!({"mediainfo":f.native.original}), false).await;
        let starts = f.proxy.thumbnail_starts.load(Ordering::SeqCst);
        let items = f.proxy.items().await;
        f.close().await;
        assert_eq!(starts, 1);
        assert_eq!(result["state"], "completed_with_warnings", "{result}");
        assert_eq!(result["result_status"], 200, "{result}");
        assert!(
            result["result"]["warnings"].as_u64().unwrap() > 0,
            "{result}"
        );
        assert!(
            result["result"]["video_thumbnails"]["failed"]
                .as_u64()
                .unwrap()
                > 0,
            "{result}"
        );
        assert!(
            items[0]["Chapters"]
                .as_array()
                .unwrap()
                .iter()
                .all(|chapter| chapter["ImageTag"].is_null())
        );
    }
}
