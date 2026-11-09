mod native {
    // Reuse the native media fixture for file refresh and legacy rebuilt coverage.
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
        reject_refresh: Arc<AtomicBool>,
        skip_saved: Arc<AtomicBool>,
        fail_second_rename: Arc<AtomicBool>,
        reject_second_refresh: Arc<AtomicBool>,
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

    fn info_path(item: &Value) -> std::path::PathBuf {
        std::path::PathBuf::from(format!(
            "{}-mediainfo.json",
            std::path::Path::new(item["Path"].as_str().unwrap())
                .with_extension("")
                .display()
        ))
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
                    if query.get("Fields").is_some_and(|fields| {
                        fields == "Id,Path,Size,MediaSources,MediaStreams,Chapters"
                    }) && proxy.fail_second_rename.swap(false, Ordering::SeqCst)
                    {
                        let file = info_path(&item);
                        std::fs::remove_file(&file).unwrap();
                        std::fs::create_dir(&file).unwrap();
                    }
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
                if let Some(path) = query.get("Path") {
                    for item in proxy.items().await {
                        if std::path::Path::new(item["Path"].as_str().unwrap()).parent()
                            == Some(std::path::Path::new(path))
                        {
                            let primary = proxy.host.item.lock().await;
                            let id = if std::path::Path::new(primary["Path"].as_str().unwrap())
                                .parent()
                                == Some(std::path::Path::new(path))
                            {
                                "directory"
                            } else {
                                "directory-two"
                            };
                            return Json(json!({"Items":[{"Id":id,"Path":path,"IsFolder":true}]}))
                                .into_response();
                        }
                    }
                }
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
                proxy.metadata_ids.lock().await.push(id.clone());
                if proxy.reject_refresh.load(Ordering::SeqCst)
                    || id == "directory-two" && proxy.reject_second_refresh.load(Ordering::SeqCst)
                {
                    return StatusCode::BAD_REQUEST.into_response();
                }
                if id == "directory" || id == "directory-two" {
                    assert_eq!(query["Recursive"], "true");
                    assert_eq!(query["ReplaceAllMetadata"], "false");
                    assert_eq!(query["ReplaceAllImages"], "false");
                    let primary = proxy.host.item.lock().await.clone();
                    let directory = if id == "directory" {
                        std::path::Path::new(primary["Path"].as_str().unwrap())
                            .parent()
                            .unwrap()
                            .to_path_buf()
                    } else {
                        std::path::Path::new(
                            proxy.extra.lock().await.values().next().unwrap()["Path"]
                                .as_str()
                                .unwrap(),
                        )
                        .parent()
                        .unwrap()
                        .to_path_buf()
                    };
                    for item in std::iter::once(&mut *proxy.host.item.lock().await)
                        .chain(proxy.extra.lock().await.values_mut())
                    {
                        let file = info_path(item);
                        if !file.is_file() || file.parent() != Some(&directory) {
                            continue;
                        }
                        let info: Value =
                            serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
                        item["MediaStreams"] = info[0]["MediaSourceInfo"]["MediaStreams"].clone();
                        item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
                        // A directory refresh keeps existing chapter image tags.
                    }
                }
                assert_eq!(*proxy.host.settings.lock().await, proxy.original_settings);
                return StatusCode::NO_CONTENT.into_response();
            } else if path.ends_with("/ClearMediaInfo") {
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
                assert_eq!(*proxy.host.settings.lock().await, proxy.original_settings);
                let items = proxy.items().await;
                if proxy.host.clears.load(Ordering::SeqCst) == 0 {
                    assert_eq!(*proxy.metadata_ids.lock().await, vec!["directory"]);
                    assert!(
                        items
                            .iter()
                            .all(|item| item["Chapters"].as_array().unwrap().iter().all(
                                |chapter| if proxy.skip_saved.load(Ordering::SeqCst) {
                                    chapter["ImageTag"].is_null()
                                } else {
                                    chapter["ImageTag"] == "old-thumbnail"
                                }
                            )),
                        "ordinary media editing must preserve old chapter image tags before extraction"
                    );
                }
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
            Self::with_flag(Some(true)).await
        }
        async fn with_flag(enabled: Option<bool>) -> Self {
            let native = Fixture::new(false).await;
            sqlx::query("UPDATE media_metadata SET file_sha1_json=$1")
                .bind(json!([format!("{:040X}", 42)]))
                .execute(&native.state.db)
                .await
                .unwrap();
            if let Some(enabled) = enabled {
                native
                    .state
                    .save_config(&json!({"extract_video_thumbnails_after_save":enabled}))
                    .await
                    .unwrap();
            }
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
                reject_refresh: Arc::default(),
                skip_saved: Arc::default(),
                fail_second_rename: Arc::default(),
                reject_second_refresh: Arc::default(),
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

        async fn split_directories(&self) {
            self.add_episode().await;
            for (id, directory) in [("61616", "first"), ("61617", "second")] {
                let root = self.native.directory.path().join(directory);
                std::fs::create_dir(&root).unwrap();
                let path = root.join(format!("{id}.strm"));
                std::fs::rename(
                    self.native.directory.path().join(format!("{id}.strm")),
                    &path,
                )
                .unwrap();
                std::fs::rename(
                    self.native
                        .directory
                        .path()
                        .join(format!("{id}-mediainfo.json")),
                    root.join(format!("{id}-mediainfo.json")),
                )
                .unwrap();
                if id == "61616" {
                    self.native.host.item.lock().await["Path"] = json!(path);
                } else {
                    self.proxy.extra.lock().await.get_mut(id).unwrap()["Path"] = json!(path);
                }
                sqlx::query("UPDATE media_metadata SET asset_details_json=jsonb_set(asset_details_json,'{0,path}',$1) WHERE emby_item_ids_json @> $2").bind(json!(path)).bind(json!([id])).execute(&self.native.state.db).await.unwrap();
            }
            sqlx::query(
                "UPDATE media_metadata SET file_sha1_json=$1 WHERE emby_item_ids_json @> $2",
            )
            .bind(json!([format!("{:040X}", 43)]))
            .bind(json!(["61617"]))
            .execute(&self.native.state.db)
            .await
            .unwrap();
            sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2)")
                .bind(format!("{:040X}", 43))
                .bind(&self.native.original)
                .execute(&self.native.state.db)
                .await
                .unwrap();
        }

        async fn close(self) {
            self.close_with_legacy(false).await;
        }
        async fn close_with_legacy(self, legacy: bool) {
            let counts = (
                self.native.host.clears.load(Ordering::SeqCst),
                self.native.host.starts.load(Ordering::SeqCst),
                self.native.host.config_writes.load(Ordering::SeqCst),
            );
            self.server.abort();
            self.native.close().await;
            if !legacy {
                assert_eq!(
                    counts.0, 0,
                    "manual/automatic title edits must not clear media info"
                );
                assert_eq!(
                    counts.1, 0,
                    "manual/automatic title edits must not start MediaInfoExtractTask"
                );
                assert_eq!(
                    counts.2, 0,
                    "manual/automatic title edits must not change plugin configuration"
                );
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_video_extracts_when_enabled_without_clearing_media_info() {
        let f = ThumbnailFixture::new().await;
        let result = f.save(json!({"mediainfo":f.native.original}), false).await;
        let starts = f.proxy.thumbnail_starts.load(Ordering::SeqCst);
        let items = f.proxy.items().await;
        f.close().await;
        assert_eq!(result["state"], "completed", "{result}");
        assert_eq!(
            starts, 1,
            "enabled extraction must run after a successful file save"
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
        assert_eq!(f.proxy.metadata_ids.lock().await.len(), 1);
        assert_eq!(
            result["result"]["video_thumbnails"]["extraction"]["verified"], 2,
            "{result}"
        );
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_failed_identity_check_does_not_start_image_extraction() {
        let f = ThumbnailFixture::new().await;
        f.native.host.item.lock().await["Path"] =
            json!(f.native.directory.path().join("wrong.strm"));
        let result = f.save(json!({"mediainfo":f.native.original}), false).await;
        let starts = f.proxy.thumbnail_starts.load(Ordering::SeqCst);
        let refreshes = f.proxy.metadata_ids.lock().await.clone();
        f.close().await;
        assert_eq!(result["state"], "failed", "{result}");
        assert_eq!(starts, 0);
        assert!(refreshes.is_empty());
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
        f.close_with_legacy(true).await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_automatic_titles_do_not_duplicate_extraction() {
        let f = ThumbnailFixture::new().await;
        let result = f.native.run().await;
        assert_eq!(result["state"], "completed", "{result}");
        assert_eq!(result["result"]["completed"], true, "{result}");
        assert_eq!(f.proxy.thumbnail_starts.load(Ordering::SeqCst), 0);
        assert_eq!(f.proxy.metadata_ids.lock().await.len(), 1);
        f.close().await;
    }
    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_native_scope_skip_does_not_hide_missing_saved_images() {
        let f = ThumbnailFixture::new().await;
        f.proxy.skip_saved.store(true, Ordering::SeqCst);
        for chapter in f.native.host.item.lock().await["Chapters"]
            .as_array_mut()
            .unwrap()
        {
            chapter.as_object_mut().unwrap().remove("ImageTag");
        }
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
    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_default_and_disabled_flags_preserve_images_without_extraction() {
        for enabled in [None, Some(false)] {
            let f = ThumbnailFixture::with_flag(enabled).await;
            let flag =
                f.native.state.config.read().await["extract_video_thumbnails_after_save"].clone();
            let mut edited = f.native.original.clone();
            edited[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("用户保存的音轨标题");
            let result = f.save(json!({"mediainfo":edited}), false).await;
            let stored: Value = serde_json::from_slice(
                &std::fs::read(f.native.directory.path().join("61616-mediainfo.json")).unwrap(),
            )
            .unwrap();
            let cached: Value =
                sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1")
                    .bind(format!("{:040X}", 42))
                    .fetch_one(&f.native.state.db)
                    .await
                    .unwrap();
            let items = f.proxy.items().await;
            let starts = f.proxy.thumbnail_starts.load(Ordering::SeqCst);
            let refreshes = f.proxy.metadata_ids.lock().await.clone();
            f.close().await;
            assert_eq!(flag, false, "save extraction flag must default to false");
            assert_eq!(result["result"]["media_info_saved"], true, "{result}");
            assert_eq!(
                result["result"]["metadata_refresh"]["state"], "submitted",
                "{result}"
            );
            assert_eq!(
                result["result"]["video_thumbnails"]["state"], "disabled",
                "{result}"
            );
            assert_eq!(starts, 0);
            assert_eq!(refreshes, vec!["directory"]);
            assert_eq!(stored, cached);
            assert_eq!(
                stored[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"],
                "用户保存的音轨标题"
            );
            assert!(
                items[0]["Chapters"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|chapter| chapter["ImageTag"] == "old-thumbnail")
            );
        }
    }
    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_failed_refresh_keeps_written_json_and_cache() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.proxy.reject_refresh.store(true, Ordering::SeqCst);
        let mut edited = f.native.original.clone();
        edited[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("已经保存的标题");
        let result = f.save(json!({"mediainfo":edited}), false).await;
        let stored: Value = serde_json::from_slice(
            &std::fs::read(f.native.directory.path().join("61616-mediainfo.json")).unwrap(),
        )
        .unwrap();
        let cached: Value =
            sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind(format!("{:040X}", 42))
                .fetch_one(&f.native.state.db)
                .await
                .unwrap();
        f.close().await;
        assert_eq!(result["result"]["media_info_saved"], true, "{result}");
        assert_eq!(
            result["result"]["metadata_refresh"]["state"], "failed",
            "{result}"
        );
        assert!(
            result["result"]["warnings"].as_u64().unwrap() > 0,
            "{result}"
        );
        assert_eq!(stored, cached);
        assert_eq!(
            stored[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"],
            "已经保存的标题"
        );
    }
    fn edited_batch(original: &Value) -> Value {
        let mut info = original.clone();
        info[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("本次新标题");
        let stream = &original[0]["MediaSourceInfo"]["MediaStreams"][1];
        json!({"mediainfo":info,"target_item_ids":["61617"],"stream_patches":[{"identity":{"type":"Audio","ordinal":1,"title":stream["Title"],"language":stream["Language"],"codec":stream["Codec"],"channels":stream["Channels"]},"changes":{"Title":"本次新标题"}}]})
    }
    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_partial_file_failure_keeps_failed_cache_and_notifies_only_saved_directory()
    {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.split_directories().await;
        f.proxy.fail_second_rename.store(true, Ordering::SeqCst);
        let result = f.save(edited_batch(&f.native.original), false).await;
        let first_cache: Value =
            sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind(format!("{:040X}", 42))
                .fetch_one(&f.native.state.db)
                .await
                .unwrap();
        let second_cache: Value =
            sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind(format!("{:040X}", 43))
                .fetch_one(&f.native.state.db)
                .await
                .unwrap();
        let original = f.native.original.clone();
        let refreshes = f.proxy.metadata_ids.lock().await.clone();
        f.close().await;
        assert_eq!(result["result_status"], 207, "{result}");
        assert_eq!(result["result"]["media_info_saved"], true, "{result}");
        assert_eq!(
            first_cache[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"],
            "本次新标题"
        );
        assert_eq!(
            second_cache, original,
            "failed file must keep the previous recovery cache"
        );
        assert_eq!(
            refreshes,
            vec!["directory"],
            "failed file's separate directory must not be notified"
        );
    }
    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_partial_directory_refresh_keeps_successful_requested_ids() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.split_directories().await;
        f.proxy.reject_second_refresh.store(true, Ordering::SeqCst);
        let result = f.save(edited_batch(&f.native.original), false).await;
        f.close().await;
        assert_eq!(
            result["result_status"], 200,
            "both files were written: {result}"
        );
        assert_eq!(
            result["result"]["metadata_refresh"]["state"], "partial",
            "{result}"
        );
        assert_eq!(
            result["result"]["metadata_refresh"]["requested_ids"],
            json!(["directory"]),
            "{result}"
        );
        assert!(
            !result["result"]["metadata_refresh"]["failures"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
