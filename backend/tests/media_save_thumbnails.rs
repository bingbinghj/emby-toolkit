mod native {
    // Reuse the native media fixture for file refresh and legacy rebuilt coverage.
    include!("title_reverse_restore.rs");

    use axum::{body::Body, http::Request, response::Response};
    use emby_toolkit::{media_save, state::string};

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
        import_metadata: Arc<AtomicBool>,
        delayed_metadata: Arc<AtomicBool>,
        reject_config_once: Arc<AtomicBool>,
        metadata_logs: Arc<Mutex<Vec<String>>>,
        reject_page_save_once: Arc<AtomicBool>,
        fail_repaired_read_once: Arc<AtomicBool>,
        saw_repaired_cache: Arc<AtomicBool>,
        db: sqlx::PgPool,
        drift_size_after_refresh: Arc<AtomicBool>,
        reject_settings_restore: Arc<AtomicBool>,
        restore_rejections: Arc<AtomicUsize>,
        reject_clear: Arc<AtomicBool>,
        incomplete_clear: Arc<AtomicBool>,
        delayed_clear: Arc<AtomicBool>,
        parts: Arc<Mutex<HashMap<String, Vec<Value>>>>,
        page_limit: Arc<AtomicUsize>,
        cached_image_reads: Arc<AtomicUsize>,
        video_reads: Arc<AtomicUsize>,
        mutate_unselected_once: Arc<AtomicBool>,
    }

    impl Proxy {
        async fn finish_metadata(&self, id: &str, full: bool) {
            if full {
                let settings = self.host.settings.lock().await;
                let flags = settings["ExclusiveControlFeatures"]
                    .as_str()
                    .unwrap()
                    .split(',')
                    .map(str::trim)
                    .collect::<Vec<_>>();
                assert!(flags.contains(&"IgnoreFileChange"));
                assert!(!flags.contains(&"CatchAllAllow"));
                assert!(!flags.contains(&"ExtractOnFileChange"));
            }
            let restoring = self.host.settings.lock().await["PersistMediaInfoMode"] == "Restore";
            if self.import_metadata.load(Ordering::SeqCst) && full {
                for item in std::iter::once(&mut *self.host.item.lock().await)
                    .chain(self.extra.lock().await.values_mut())
                {
                    if full && item["Id"] != id {
                        continue;
                    }
                    if !restoring || !item["MediaStreams"].as_array().unwrap().is_empty() {
                        continue;
                    }
                    let file = info_path(item);
                    if !file.is_file() {
                        continue;
                    }
                    let info: Value =
                        serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
                    if item["Type"] == "Audio"
                        && let Some(embedded) = info[0]["EmbeddedImage"].as_str()
                    {
                        use base64::Engine;
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(embedded)
                            .unwrap();
                        tokio::fs::write(
                            item_metadata_directory(&self.host, item).join("primary.jpg"),
                            bytes,
                        )
                        .await
                        .unwrap();
                        item["ImageTags"]["Primary"] = json!("cached-old-cover");
                    }
                    item["MediaStreams"] = info[0]["MediaSourceInfo"]["MediaStreams"].clone();
                    item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
                    item["Size"] = info[0]["MediaSourceInfo"]["Size"].clone();
                    item["Chapters"] = info[0]["Chapters"].clone();
                    let images = native_chapter_images(&self.host, item);
                    for (chapter, exists) in item["Chapters"]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .zip(images)
                    {
                        if exists {
                            chapter["ImageTag"] = json!("old-thumbnail");
                        }
                    }
                }
            }
            if full && self.drift_size_after_refresh.swap(false, Ordering::SeqCst) {
                let mut item = self.host.item.lock().await;
                item["Size"] = json!(item["Size"].as_u64().unwrap() + 1);
            }
            self.metadata_logs.lock().await.push(format!(
                "2026-11-09 00:00:02.000 RefreshItem Complete: {id} fixture"
            ));
        }
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
    fn requested_fields(mut item: Value, query: &HashMap<String, String>) -> Value {
        let fields = query.get("Fields").map(String::as_str).unwrap_or("");
        if !fields.split(',').any(|field| field == "MediaStreams") {
            item.as_object_mut().unwrap().remove("MediaStreams");
        }
        if !fields.split(',').any(|field| field == "MediaSources") {
            item.as_object_mut().unwrap().remove("MediaSources");
        }
        item
    }

    async fn proxy(State(proxy): State<Proxy>, request: Request<Body>) -> Response {
        use axum::response::IntoResponse;
        let path = request.uri().path().to_string();
        let query: HashMap<String, String> =
            url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
                .into_owned()
                .collect();
        if request.method() == axum::http::Method::GET {
            if path.starts_with("/Items/") && path.ends_with("/ThumbnailSet") {
                let id = path.split('/').nth(2).unwrap();
                let item = proxy
                    .items()
                    .await
                    .into_iter()
                    .find(|item| item["Id"] == id)
                    .unwrap();
                let frames = native_bif_frames(&item).unwrap_or_default();
                return Json(json!({"Thumbnails":frames.iter().map(|(ms,_)|json!({"Time":ms,"Tag":"old-thumbnail"})).collect::<Vec<_>>()})).into_response();
            }
            if path.contains("/stream") || path.ends_with("/Download") || path.starts_with("/cd2/")
            {
                proxy.video_reads.fetch_add(1, Ordering::SeqCst);
                return StatusCode::BAD_REQUEST.into_response();
            }
            if path.starts_with("/Items/") && path.contains("/Images/Chapter/") {
                let id = path.split('/').nth(2).unwrap();
                let index = path.rsplit('/').next().unwrap().parse::<usize>().unwrap();
                let item = proxy
                    .items()
                    .await
                    .into_iter()
                    .find(|item| item["Id"] == id)
                    .unwrap();
                let Some(chapter) = item["Chapters"]
                    .as_array()
                    .and_then(|chapters| chapters.get(index))
                else {
                    return StatusCode::NOT_FOUND.into_response();
                };
                proxy.cached_image_reads.fetch_add(1, Ordering::SeqCst);
                return match tokio::fs::read(chapter_image(&proxy.host, &item, chapter)).await {
                    Ok(bytes) => {
                        ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], bytes).into_response()
                    }
                    Err(_) => StatusCode::NOT_FOUND.into_response(),
                };
            }
            if path.starts_with("/Items/") && path.ends_with("/SpecialFeatures") {
                return Json(json!([])).into_response();
            }
            if path.starts_with("/Videos/") && path.ends_with("/AdditionalParts") {
                let id = path.split('/').nth(2).unwrap();
                let parts = proxy
                    .parts
                    .lock()
                    .await
                    .get(id)
                    .cloned()
                    .unwrap_or_default();
                return Json(json!({"Items":parts})).into_response();
            }
            if path == "/UI/View" && proxy.reject_config_once.swap(false, Ordering::SeqCst) {
                return StatusCode::BAD_REQUEST.into_response();
            }
            if path.starts_with("/Users/user/Items/") {
                if query.get("Fields").is_some_and(|fields| {
                    fields == "Id,Path,Type,Size,Chapters,MediaSources,MediaStreams"
                }) && proxy.fail_repaired_read_once.load(Ordering::SeqCst)
                {
                    let file = info_path(&*proxy.host.item.lock().await);
                    let info: Value =
                        serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
                    if info[0]["MediaSourceInfo"]["MediaStreams"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|stream| stream["Type"] == "Audio")
                        .count()
                        == 2
                    {
                        proxy.fail_repaired_read_once.store(false, Ordering::SeqCst);
                        let cached: Value = sqlx::query_scalar(
                            "SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1",
                        )
                        .bind(format!("{:040X}", 42))
                        .fetch_one(&proxy.db)
                        .await
                        .unwrap();
                        proxy
                            .saw_repaired_cache
                            .store(cached == info, Ordering::SeqCst);
                        return StatusCode::BAD_REQUEST.into_response();
                    }
                }
                if query.get("Fields").is_some_and(|fields| {
                    fields == "Id,Path,Type,Chapters,MediaSources,MediaStreams"
                }) {
                    proxy.readbacks.fetch_add(1, Ordering::SeqCst);
                }
                let id = path.rsplit('/').next().unwrap();
                let extra_item = proxy.extra.lock().await.get(id).cloned();
                if let Some(item) = extra_item {
                    if id == "61617"
                        && query.get("Fields").is_some_and(|fields| {
                            fields == "Id,Path,Size,MediaSources,MediaStreams,Chapters"
                        })
                        && proxy.mutate_unselected_once.swap(false, Ordering::SeqCst)
                    {
                        let mut extra = proxy.extra.lock().await;
                        let item = extra.get_mut(id).unwrap();
                        item["MediaStreams"][1]["Title"] = json!("V2 最新用户标题");
                        item["MediaStreams"][1]["IsDefault"] = json!(true);
                        item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
                        return Json(requested_fields(item.clone(), &query)).into_response();
                    }
                    if query.get("Fields").is_some_and(|fields| {
                        fields == "Id,Path,Size,MediaSources,MediaStreams,Chapters"
                    }) && proxy.fail_second_rename.swap(false, Ordering::SeqCst)
                    {
                        let file = info_path(&item);
                        std::fs::remove_file(&file).unwrap();
                        std::fs::create_dir(&file).unwrap();
                    }
                    return Json(requested_fields(item, &query)).into_response();
                }
                if id == "61616" {
                    return Json(requested_fields(
                        proxy.host.item.lock().await.clone(),
                        &query,
                    ))
                    .into_response();
                }
            } else if path == "/ScheduledTasks" {
                let native = json!({"Id":"media","Key":"MediaInfoExtractTask","State":"Idle","LastExecutionResult":{"StartTimeUtc":proxy.host.starts.load(Ordering::SeqCst).to_string(),"Status":"Completed"}});
                return Json(json!([native, proxy.thumbnail_task.lock().await.clone()]))
                    .into_response();
            } else if path == "/System/Logs/embyserver.txt/Lines" {
                let mut lines = proxy.metadata_logs.lock().await.clone();
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
                        (!query.contains_key("Path") && !query.contains_key("Ids"))
                            || query.get("Path").is_some_and(|path| item["Path"] == *path)
                            || query
                                .get("Ids")
                                .is_some_and(|ids| ids.split(',').any(|id| item["Id"] == id))
                    })
                    .collect::<Vec<_>>();
                let total = found.len();
                let start = query
                    .get("StartIndex")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let limit = query
                    .get("Limit")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(total)
                    .min(proxy.page_limit.load(Ordering::SeqCst));
                let page = found
                    .into_iter()
                    .skip(start)
                    .take(limit)
                    .collect::<Vec<_>>();
                return Json(json!({"Items":page,"TotalRecordCount":total})).into_response();
            }
        } else if request.method() == axum::http::Method::POST {
            if path == "/UI/Command" && proxy.reject_page_save_once.swap(false, Ordering::SeqCst) {
                return StatusCode::BAD_REQUEST.into_response();
            }
            if path == "/UI/Command" && proxy.reject_settings_restore.load(Ordering::SeqCst) {
                let (parts, body) = request.into_parts();
                let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
                let command: Value = serde_json::from_slice(&bytes).unwrap();
                let desired: Value =
                    serde_json::from_str(command["Data"].as_str().unwrap()).unwrap();
                if desired["ExclusiveControlFeatures"]
                    == proxy.original_settings["ExclusiveControlFeatures"]
                {
                    proxy.restore_rejections.fetch_add(1, Ordering::SeqCst);
                    return StatusCode::BAD_REQUEST.into_response();
                }
                return proxy
                    .forward(Request::from_parts(parts, Body::from(bytes)))
                    .await;
            }
            if path.ends_with("/Refresh") {
                let id = path.split('/').nth(2).unwrap().to_string();
                proxy.metadata_ids.lock().await.push(id.clone());
                if proxy.reject_refresh.load(Ordering::SeqCst)
                    || matches!(id.as_str(), "directory-two" | "61617")
                        && proxy.reject_second_refresh.load(Ordering::SeqCst)
                {
                    return StatusCode::BAD_REQUEST.into_response();
                }
                let full = query["MetadataRefreshMode"] == "FullRefresh";
                assert_eq!(query["Recursive"], if full { "false" } else { "true" });
                assert_eq!(
                    query["ImageRefreshMode"],
                    if full { "ValidationOnly" } else { "Default" }
                );
                assert_eq!(query["ReplaceAllMetadata"], "false");
                assert_eq!(query["ReplaceAllImages"], "false");
                proxy.metadata_logs.lock().await.push(format!(
                    "2026-11-09 00:00:01.000 RefreshItem Start: {id} fixture"
                ));
                if full && proxy.delayed_metadata.load(Ordering::SeqCst) {
                    let copy = proxy.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        copy.finish_metadata(&id, full).await;
                    });
                } else {
                    proxy.finish_metadata(&id, full).await;
                }
                return StatusCode::NO_CONTENT.into_response();
            } else if path.ends_with("/ClearMediaInfo") {
                let id = path.split('/').nth(2).unwrap();
                let current = proxy
                    .items()
                    .await
                    .into_iter()
                    .find(|item| item["Id"] == id)
                    .expect("Clear must target a known native media ID");
                proxy.host.clear_ids.lock().await.push(id.into());
                proxy.host.clears.fetch_add(1, Ordering::SeqCst);
                if proxy.reject_clear.load(Ordering::SeqCst) {
                    return StatusCode::BAD_REQUEST.into_response();
                }
                if proxy.incomplete_clear.load(Ordering::SeqCst) {
                    return StatusCode::NO_CONTENT.into_response();
                }
                if proxy.delayed_clear.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(2500)).await;
                }
                let members = proxy
                    .items()
                    .await
                    .into_iter()
                    .filter(|item| {
                        item["PresentationUniqueKey"] == current["PresentationUniqueKey"]
                            && item["ExtraType"].as_str().unwrap_or("").is_empty()
                                == current["ExtraType"].as_str().unwrap_or("").is_empty()
                    })
                    .map(|item| string(&item, "Id"))
                    .collect::<Vec<_>>();
                let mut affected = members
                    .iter()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>();
                for member in members {
                    affected.extend(
                        proxy
                            .parts
                            .lock()
                            .await
                            .get(&member)
                            .into_iter()
                            .flatten()
                            .map(|part| string(part, "Id")),
                    );
                }
                for item in std::iter::once(&mut *proxy.host.item.lock().await)
                    .chain(proxy.extra.lock().await.values_mut())
                {
                    if !affected.contains(item["Id"].as_str().unwrap()) {
                        continue;
                    }
                    delete_item_resources(&proxy.host, item).await;
                    item["MediaStreams"] = json!([]);
                    item["MediaSources"][0]["MediaStreams"] = json!([]);
                    item["Chapters"] = json!([]);
                    item["Size"] = json!(0);
                    let file = info_path(item);
                    if file.is_file() {
                        tokio::fs::remove_file(file).await.unwrap();
                    }
                }
                return StatusCode::NO_CONTENT.into_response();
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
                    assert_eq!(proxy.metadata_ids.lock().await.len(), items.len());
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
            seed_standard_bif(&native.host, &*native.host.item.lock().await).await;
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
                import_metadata: Arc::new(AtomicBool::new(true)),
                delayed_metadata: Arc::default(),
                reject_config_once: Arc::default(),
                metadata_logs: Arc::new(Mutex::new(vec![
                    "2026-11-08 00:00:00.000 fixture log anchor".into(),
                ])),
                reject_page_save_once: Arc::default(),
                fail_repaired_read_once: Arc::default(),
                saw_repaired_cache: Arc::default(),
                db: native.state.db.clone(),
                drift_size_after_refresh: Arc::default(),
                reject_settings_restore: Arc::default(),
                restore_rejections: Arc::default(),
                reject_clear: Arc::default(),
                incomplete_clear: Arc::default(),
                delayed_clear: Arc::default(),
                parts: Arc::default(),
                page_limit: Arc::new(AtomicUsize::new(500)),
                cached_image_reads: Arc::default(),
                video_reads: Arc::default(),
                mutate_unselected_once: Arc::default(),
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
            tokio::time::timeout(Duration::from_secs(80), async {
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
            item["Guid"] = json!("00000000-0000-0000-0000-00000000f0b1");
            item["PresentationUniqueKey"] = json!(format!("fixture-key-{id}"));
            item["Type"] = json!("Episode");
            item["Path"] = json!(path);
            item["MediaSources"][0]["Id"] = json!(format!("mediasource_{id}"));
            item["MediaSources"][0]["ItemId"] = json!(id);
            item["MediaSources"][0]["Path"] = json!(source);
            seed_chapter_images(&self.native.host, &item).await;
            seed_standard_bif(&self.native.host, &item).await;
            self.proxy.extra.lock().await.insert(id.into(), item);
            sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES('5646','Episode',$1,$2,$3,$3)")
                .bind(json!([id])).bind(json!([{"emby_item_id":id,"path":path,"size_bytes":info[0]["MediaSourceInfo"]["Size"]}])).bind(json!([null])).execute(&self.native.state.db).await.unwrap();
        }
        async fn run_audio(&self, restore: bool) -> Value {
            let state = self.native.state.clone();
            let task = state
                .jobs
                .clone()
                .submit("audio file refresh regression", move |context| async move {
                    if restore {
                        emby_toolkit::audio_tasks::restore(&state, &context).await
                    } else {
                        emby_toolkit::audio_tasks::repair(&state, &["61616".into()], &context).await
                    }
                })
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(80), async {
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
        async fn backup(&self, info: &Value) {
            let file = self.native.directory.path().join("61616-mediainfo.json");
            let root = self
                .native
                .state
                .data_dir
                .join("backups/truehd-core-shift/20260801-000001");
            tokio::fs::create_dir_all(&root).await.unwrap();
            let name = format!(
                "61616-{}.json",
                emby_toolkit::audio_tasks::path_hash(file.to_str().unwrap())
            );
            tokio::fs::write(root.join(name), serde_json::to_vec(info).unwrap())
                .await
                .unwrap();
        }
        async fn core_shift(&self) -> Value {
            let raw = json!({"format":{"size":self.native.original[0]["MediaSourceInfo"]["Size"].to_string(),"duration":"600","format_name":"matroska"},"streams":[
                {"codec_type":"video","codec_name":"h264","index":0},
                {"codec_type":"audio","codec_name":"truehd","index":1,"channels":8,"sample_rate":"48000","bit_rate":"4000000","tags":{"language":"jpn","title":"日语"}},
                {"codec_type":"audio","codec_name":"aac","index":3,"channels":2,"sample_rate":"48000","bit_rate":"128000","tags":{"language":"chi","title":"国语"}}
            ]});
            let reference =
                emby_toolkit::probe::build(&raw, &json!({"fn":"61616.mkv"}), &json!([])).unwrap();
            let mut old = reference.clone();
            old[0]["Chapters"] = self.native.original[0]["Chapters"].clone();
            let streams = old[0]["MediaSourceInfo"]["MediaStreams"]
                .as_array_mut()
                .unwrap();
            let mut core = streams[2].clone();
            core["Index"] = json!(2);
            core["Codec"] = json!("ac3");
            core["Channels"] = json!(6);
            core["SampleRate"] = json!(48000);
            core["BitRate"] = json!(640000);
            streams[2].as_object_mut().unwrap().remove("Language");
            streams[2].as_object_mut().unwrap().remove("Title");
            streams.insert(2, core);
            let analysis =
                emby_toolkit::audio::analyze(&old, Some(&reference), "ffprobe 原始结构", true);
            assert_eq!(analysis["safe"], true, "{analysis}");
            tokio::fs::write(
                self.native.directory.path().join("61616-mediainfo.json"),
                serde_json::to_vec(&old).unwrap(),
            )
            .await
            .unwrap();
            let mut item = self.native.host.item.lock().await;
            item["MediaStreams"] = old[0]["MediaSourceInfo"]["MediaStreams"].clone();
            item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
            sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json,raw_ffprobe_json) VALUES($1,$2,$3) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json,raw_ffprobe_json=EXCLUDED.raw_ffprobe_json")
                .bind(format!("{:040X}",42)).bind(&old).bind(raw).execute(&self.native.state.db).await.unwrap();
            old
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
                std::fs::rename(
                    self.native.directory.path().join(format!("{id}-320-1.bif")),
                    root.join(format!("{id}-320-1.bif")),
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
            if !legacy {
                assert_eq!(
                    counts.1, 0,
                    "manual/automatic title edits must not start MediaInfoExtractTask"
                );
                assert_eq!(
                    self.proxy.video_reads.load(Ordering::SeqCst),
                    0,
                    "media save must not request video bytes"
                );
                if counts.2 > 0 {
                    let pending = self.native.state.data_dir.join("backups/media-info-reload");
                    let protected = pending.is_dir()
                        && std::fs::read_dir(pending).unwrap().any(|entry| {
                            let path = entry.unwrap().path();
                            path.join("pending-scan.json").is_file()
                                || path.join("media-reload.json").is_file()
                        });
                    if !protected {
                        assert_eq!(
                            *self.native.host.settings.lock().await,
                            self.proxy.original_settings
                        );
                    }
                }
            }
            self.server.abort();
            self.native.close().await;
        }
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn manual_save_temporarily_blocks_catch_all_allow_and_restores_original_flags() {
        let mut f = ThumbnailFixture::with_flag(Some(false)).await;
        let original = json!("CatchAllAllow,CatchAllBlock,ExtractOnFileChange,CustomFlag");
        f.native.host.settings.lock().await["ExclusiveControlFeatures"] = original.clone();
        f.proxy.original_settings["ExclusiveControlFeatures"] = original.clone();
        let result = f.save(json!({"mediainfo":f.native.original}), false).await;
        let restored = f.native.host.settings.lock().await["ExclusiveControlFeatures"].clone();
        f.close().await;
        assert_eq!(
            result["result"]["metadata_refresh"]["state"], "verified",
            "{result}"
        );
        assert_eq!(restored, original);
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
        assert_eq!(f.proxy.metadata_ids.lock().await.len(), 2);
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
    async fn audio_reload_keeps_the_native_cover_instead_of_importing_the_stale_embedded_image() {
        use base64::Engine;
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        let mut info = f.native.original.clone();
        info[0]["MediaSourceInfo"]["MediaStreams"]
            .as_array_mut()
            .unwrap()
            .retain(|stream| stream["Type"] == "Audio");
        info[0]["MediaSourceInfo"]["MediaStreams"][0]["Title"] = json!("音频新标题");
        let original = f.native.host.item.lock().await.clone();
        let old_bytes = std::fs::read(chapter_image(
            &f.native.host,
            &original,
            &original["Chapters"][1],
        ))
        .unwrap();
        let new_bytes = std::fs::read(chapter_image(
            &f.native.host,
            &original,
            &original["Chapters"][0],
        ))
        .unwrap();
        info[0]["EmbeddedImage"] =
            json!(base64::engine::general_purpose::STANDARD.encode(&old_bytes));
        {
            let mut item = f.native.host.item.lock().await;
            item["Type"] = json!("Audio");
            item["Chapters"] = json!([]);
            item["ImageTags"] = json!({"Primary":"native-user-new-cover"});
            item["MediaStreams"] = info[0]["MediaSourceInfo"]["MediaStreams"].clone();
            item["MediaStreams"][0]["Title"] = json!("音频旧标题");
            item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
        }
        let before = f.native.host.item.lock().await.clone();
        let cover = item_metadata_directory(&f.native.host, &before).join("primary.jpg");
        tokio::fs::write(&cover, &new_bytes).await.unwrap();
        let result = f.save(json!({"mediainfo":info.clone()}), false).await;
        let live = f.native.host.item.lock().await.clone();
        let actual_cover = std::fs::read(&cover).unwrap();
        let saved: Value =
            serde_json::from_slice(&std::fs::read(info_path(&live)).unwrap()).unwrap();
        let cached: Value =
            sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind(format!("{:040X}", 42))
                .fetch_one(&f.native.state.db)
                .await
                .unwrap();
        f.close().await;
        assert_eq!(
            result["result"]["metadata_refresh"]["state"], "verified",
            "{result}"
        );
        assert_eq!(live["MediaStreams"][0]["Title"], "音频新标题");
        assert_eq!(live["ImageTags"]["Primary"], "native-user-new-cover");
        assert_eq!(actual_cover, new_bytes);
        assert!(saved[0].get("EmbeddedImage").is_none());
        assert_eq!(
            cached[0]["EmbeddedImage"], info[0]["EmbeddedImage"],
            "only the native reload copy may strip stale cover data"
        );
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
        tokio::fs::remove_file(local_bif(&*f.native.host.item.lock().await))
            .await
            .unwrap();
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
                result["result"]["metadata_refresh"]["state"], "verified",
                "{result}"
            );
            assert_eq!(
                result["result"]["video_thumbnails"]["state"], "disabled",
                "{result}"
            );
            assert_eq!(starts, 0);
            assert_eq!(refreshes, vec!["61616"]);
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
            vec!["61616"],
            "failed file's target must not be refreshed"
        );
    }
    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn manual_save_requires_target_refresh_and_readback_after_json_save() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        let mut edited = f.native.original.clone();
        edited[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("本次新标题");
        let result = f.save(json!({"mediainfo":edited}), false).await;
        let refreshes = f.proxy.metadata_ids.lock().await.clone();
        f.close().await;
        assert_eq!(
            result["result"]["metadata_refresh"]["state"], "verified",
            "{result}"
        );
        assert_eq!(refreshes, vec!["61616"]);
        assert_eq!(
            result["result"]["metadata_refresh"]["verified_ids"],
            json!(["61616"])
        );
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn manual_save_holds_ignore_through_delayed_target_refresh() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.proxy.delayed_metadata.store(true, Ordering::SeqCst);
        let mut edited = f.native.original.clone();
        edited[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("延迟导入的新标题");
        let result = f.save(json!({"mediainfo":edited}), false).await;
        assert_eq!(
            result["result"]["metadata_refresh"]["state"], "verified",
            "{result}"
        );
        assert_eq!(
            f.native.host.item.lock().await["MediaStreams"][1]["Title"],
            "延迟导入的新标题"
        );
        assert_eq!(
            *f.native.host.settings.lock().await,
            f.proxy.original_settings
        );
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn manual_save_reports_failure_when_restore_does_not_import_streams() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.proxy.import_metadata.store(false, Ordering::SeqCst);
        let mut edited = f.native.original.clone();
        edited[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("仅本地保存的标题");
        let result = f.save(json!({"mediainfo":edited}), false).await;
        assert_eq!(result["state"], "completed_with_warnings", "{result}");
        assert_eq!(
            result["result"]["metadata_refresh"]["state"], "failed",
            "{result}"
        );
        assert_eq!(
            result["result"]["metadata_refresh"]["failed_ids"],
            json!(["61616"])
        );
        assert!(result["result"]["warnings"].as_u64().unwrap() > 0);
        assert_ne!(
            f.native.host.item.lock().await["MediaStreams"][1]["Title"],
            "仅本地保存的标题"
        );
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn manual_save_stops_readback_immediately_when_media_size_changes() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.proxy
            .drift_size_after_refresh
            .store(true, Ordering::SeqCst);
        let started = tokio::time::Instant::now();
        let result = f.save(json!({"mediainfo":f.native.original}), false).await;
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "an identity conflict must stop without waiting the 30-second readback window"
        );
        assert_eq!(
            result["result"]["metadata_refresh"]["state"], "failed",
            "{result}"
        );
        assert_eq!(
            result["result"]["metadata_refresh"]["failed_ids"],
            json!(["61616"])
        );
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn cancelled_manual_save_finishes_accepted_clear_and_restore_before_releasing_protection()
    {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.proxy.delayed_clear.store(true, Ordering::SeqCst);
        let state = f.native.state.clone();
        let mut edited = f.native.original.clone();
        edited[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("取消时仍需恢复的新标题");
        let body = json!({"mediainfo":edited});
        let task = state
            .jobs
            .clone()
            .submit_response("cancelled target clear", move |context| async move {
                media_save::save(&state, "61616", &body, &context).await
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while f.native.host.clear_ids.lock().await.is_empty() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        f.native
            .state
            .jobs
            .control(Some(&task), "stop")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            f.native.state.jobs.result(&task).await.unwrap()["is_running"],
            true
        );
        assert!(reload_recovery(&f).join("media-reload.json").is_file());
        assert_eq!(
            f.native.host.settings.lock().await["PersistMediaInfoMode"],
            "Restore"
        );
        assert!(
            f.proxy.metadata_ids.lock().await.is_empty(),
            "refresh must wait for synchronous clear to finish"
        );
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let result = f.native.state.jobs.result(&task).await.unwrap();
                if result["is_running"] == false {
                    break result;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let live = f.native.host.item.lock().await.clone();
        let settings = f.native.host.settings.lock().await.clone();
        let original = f.proxy.original_settings.clone();
        let backup = f.native.state.data_dir.join("backups/media-info-reload");
        let backup_empty = !backup.exists() || std::fs::read_dir(backup).unwrap().next().is_none();
        f.close().await;
        assert_eq!(result["state"], "cancelled", "{result}");
        assert_eq!(live["MediaStreams"][1]["Title"], "取消时仍需恢复的新标题");
        assert_eq!(settings, original);
        assert!(
            backup_empty,
            "completed restoration must remove the cancelled task snapshot"
        );
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn completed_refresh_keeps_recovery_when_settings_restore_fails() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.proxy
            .reject_settings_restore
            .store(true, Ordering::SeqCst);
        let result = f.save(json!({"mediainfo":f.native.original}), false).await;
        assert_eq!(
            result["result"]["metadata_refresh"]["state"], "failed",
            "{result}"
        );
        assert_eq!(
            result["result"]["metadata_refresh"]["verified_ids"],
            json!(["61616"])
        );
        assert!(f.proxy.restore_rejections.load(Ordering::SeqCst) >= 3);
        let root = f.native.state.data_dir.join("backups/media-info-reload");
        let recovery = std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.join("pending-scan.json").is_file())
            .expect("accepted scan and original settings must be retained after restore failure");
        assert!(recovery.join("original-settings.json").is_file());
        f.proxy
            .reject_settings_restore
            .store(false, Ordering::SeqCst);
        emby_toolkit::strm::with_settings(&f.native.state, "ignore_file_change", || async {
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(
            *f.native.host.settings.lock().await,
            f.proxy.original_settings
        );
        assert!(!recovery.join("pending-scan.json").exists());
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn rebuilt_staging_does_not_change_cache_when_settings_fail_before_file_commit() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2)")
            .bind(format!("{:040X}", 42))
            .bind(&f.native.original)
            .execute(&f.native.state.db)
            .await
            .unwrap();
        f.proxy.reject_page_save_once.store(true, Ordering::SeqCst);
        let mut edited = f.native.original.clone();
        edited[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("不应提交的缓存标题");
        let result = f.save(json!({"mediainfo":edited}), true).await;
        let cached: Value =
            sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind(format!("{:040X}", 42))
                .fetch_one(&f.native.state.db)
                .await
                .unwrap();
        let stored: Value = serde_json::from_slice(
            &std::fs::read(f.native.directory.path().join("61616-mediainfo.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(result["state"], "failed", "{result}");
        assert_eq!(cached, f.native.original);
        assert_eq!(stored, f.native.original);
        f.close_with_legacy(true).await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn audio_repair_uses_only_exact_target_refresh_and_verifies_removed_core() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.core_shift().await;
        let result = f.run_audio(false).await;
        assert_eq!(result["state"], "completed", "{result}");
        assert_eq!(result["result"]["repaired"], 1, "{result}");
        let item = f.native.host.item.lock().await.clone();
        assert_eq!(
            item["MediaStreams"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|stream| stream["Type"] == "Audio")
                .count(),
            2
        );
        assert_eq!(*f.proxy.metadata_ids.lock().await, vec!["61616"]);
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn audio_repair_failure_restores_files_and_previous_cache_before_retry_refresh() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        let original = f.core_shift().await;
        f.proxy
            .fail_repaired_read_once
            .store(true, Ordering::SeqCst);
        let result = f.run_audio(false).await;
        let cached: Value =
            sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind(format!("{:040X}", 42))
                .fetch_one(&f.native.state.db)
                .await
                .unwrap();
        let stored: Value = serde_json::from_slice(
            &std::fs::read(f.native.directory.path().join("61616-mediainfo.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(result["state"], "failed", "{result}");
        assert!(
            f.proxy.saw_repaired_cache.load(Ordering::SeqCst),
            "the failure must occur after both the repaired JSON and cache were committed"
        );
        assert_eq!(cached, original);
        assert_eq!(stored, original);
        assert_eq!(
            f.native.host.item.lock().await["MediaStreams"],
            original[0]["MediaSourceInfo"]["MediaStreams"]
        );
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn audio_backup_restore_updates_cache_and_verifies_actual_titles() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        let mut backup = f.native.original.clone();
        backup[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("备份原标题");
        f.backup(&backup).await;
        let result = f.run_audio(true).await;
        let cached: Value =
            sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind(format!("{:040X}", 42))
                .fetch_one(&f.native.state.db)
                .await
                .unwrap();
        let stored: Value = serde_json::from_slice(
            &std::fs::read(f.native.directory.path().join("61616-mediainfo.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(result["state"], "completed", "{result}");
        assert_eq!(result["result"]["verified"], 1);
        assert_eq!(cached, stored);
        assert_eq!(
            f.native.host.item.lock().await["MediaStreams"][1]["Title"],
            "备份原标题"
        );
        assert_eq!(*f.proxy.metadata_ids.lock().await, vec!["61616"]);
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn matching_local_audio_backup_still_retries_when_emby_titles_are_stale() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.backup(&f.native.original).await;
        f.native.host.item.lock().await["MediaStreams"][1]["Title"] = json!("尚未刷新");
        let result = f.run_audio(true).await;
        assert_eq!(result["state"], "completed", "{result}");
        assert_eq!(result["result"]["restored"], 0);
        assert_eq!(result["result"]["already_restored"], 1);
        assert_eq!(result["result"]["verified"], 1);
        assert_eq!(
            f.native.host.item.lock().await["MediaStreams"][1]["Title"],
            f.native.original[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"]
        );
        assert_eq!(*f.proxy.metadata_ids.lock().await, vec!["61616"]);
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn matching_local_and_emby_audio_backup_does_not_repeat_full_refresh() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.backup(&f.native.original).await;
        {
            let mut item = f.native.host.item.lock().await;
            item["MediaStreams"] = f.native.original[0]["MediaSourceInfo"]["MediaStreams"].clone();
            item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
        }
        let result = f.run_audio(true).await;
        let cached: Value =
            sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind(format!("{:040X}", 42))
                .fetch_one(&f.native.state.db)
                .await
                .unwrap();
        let stored: Value = serde_json::from_slice(
            &std::fs::read(f.native.directory.path().join("61616-mediainfo.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(result["state"], "completed", "{result}");
        assert_eq!(result["result"]["already_restored"], 1);
        assert_eq!(result["result"]["verified"], 1);
        assert_eq!(cached, stored);
        assert!(f.proxy.metadata_ids.lock().await.is_empty());
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn ignored_library_audio_backups_are_counted_as_skipped() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.backup(&f.native.original).await;
        f.native
            .state
            .save_config(&json!({"libraries_to_ignore":["library"]}))
            .await
            .unwrap();
        let result = f.run_audio(true).await;
        assert_eq!(result["result"]["skipped"], 1, "{result}");
        assert_eq!(result["result"]["restored"], 0);
        assert_eq!(result["result"]["verified"], 0);
        assert!(f.proxy.metadata_ids.lock().await.is_empty());
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn audio_backup_cache_failure_rolls_back_files_and_previous_cache() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2)")
            .bind(format!("{:040X}", 42))
            .bind(&f.native.original)
            .execute(&f.native.state.db)
            .await
            .unwrap();
        sqlx::query("CREATE FUNCTION reject_backup_cache() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.mediainfo_json #>> '{0,MediaSourceInfo,MediaStreams,1,Title}' = '拒绝还原' THEN RAISE EXCEPTION 'fixture rejects backup cache'; END IF; RETURN NEW; END $$").execute(&f.native.state.db).await.unwrap();
        sqlx::query("CREATE TRIGGER reject_backup_cache BEFORE INSERT OR UPDATE ON p115_mediainfo_cache FOR EACH ROW EXECUTE FUNCTION reject_backup_cache()").execute(&f.native.state.db).await.unwrap();
        let mut backup = f.native.original.clone();
        backup[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("拒绝还原");
        f.backup(&backup).await;
        let result = f.run_audio(true).await;
        let cached: Value =
            sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind(format!("{:040X}", 42))
                .fetch_one(&f.native.state.db)
                .await
                .unwrap();
        let stored: Value = serde_json::from_slice(
            &std::fs::read(f.native.directory.path().join("61616-mediainfo.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(result["state"], "failed", "{result}");
        assert_eq!(cached, f.native.original);
        assert_eq!(stored, f.native.original);
        assert_eq!(
            f.native.host.item.lock().await["MediaStreams"],
            f.native.original[0]["MediaSourceInfo"]["MediaStreams"]
        );
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn audio_backup_rollback_failure_is_reported_and_backup_is_kept() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2)")
            .bind(format!("{:040X}", 42))
            .bind(&f.native.original)
            .execute(&f.native.state.db)
            .await
            .unwrap();
        sqlx::query("CREATE FUNCTION reject_all_cache_updates() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture rejects every cache update'; END $$").execute(&f.native.state.db).await.unwrap();
        sqlx::query("CREATE TRIGGER reject_all_cache_updates BEFORE INSERT OR UPDATE ON p115_mediainfo_cache FOR EACH ROW EXECUTE FUNCTION reject_all_cache_updates()").execute(&f.native.state.db).await.unwrap();
        let mut backup = f.native.original.clone();
        backup[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("待还原备份标题");
        f.backup(&backup).await;
        let result = f.run_audio(true).await;
        assert_eq!(result["state"], "failed", "{result}");
        assert!(
            result["result"]["failures"]
                .as_array()
                .unwrap()
                .iter()
                .any(|failure| failure.as_str().unwrap().contains("回滚未完成")),
            "{result}"
        );
        assert!(
            result["result"]["message"]
                .as_str()
                .unwrap()
                .contains("回滚未完成"),
            "{result}"
        );
        let backup_root = f
            .native
            .state
            .data_dir
            .join("backups/truehd-core-shift/20260801-000001");
        assert!(
            std::fs::read_dir(backup_root).unwrap().next().is_some(),
            "a failed restore must retain its original backup"
        );
        f.close().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn after_save_partial_target_refresh_keeps_requests_and_protection() {
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
            result["result"]["metadata_refresh"]["state"], "failed",
            "{result}"
        );
        assert_eq!(
            result["result"]["metadata_refresh"]["requested_ids"],
            json!(["61616", "61617"]),
            "{result}"
        );
        assert!(
            !result["result"]["metadata_refresh"]["failures"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    fn reload_recovery(f: &ThumbnailFixture) -> PathBuf {
        std::fs::read_dir(f.native.state.data_dir.join("backups/media-info-reload"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.join("media-reload.json").is_file())
            .expect("failed reload must keep the target snapshot")
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn manual_save_overwrites_existing_languages_and_preserves_chapters_images_and_bif() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        {
            let mut item = f.native.host.item.lock().await;
            item["Chapters"].as_array_mut().unwrap().truncate(12);
            item["MediaStreams"][1]["Title"] = Value::Null;
            item["MediaStreams"][1]["Language"] = json!("chi");
            item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
        }
        let old = f.native.host.item.lock().await.clone();
        let images = old["Chapters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|chapter| {
                let file = chapter_image(&f.native.host, &old, chapter);
                let bytes = std::fs::read(&file).unwrap();
                (file, bytes)
            })
            .collect::<Vec<_>>();
        let bif = local_bif(&old);
        let original_bif = tokio::fs::read(&bif).await.unwrap();
        let mut edited = f.native.original.clone();
        edited[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("日语");
        edited[0]["MediaSourceInfo"]["MediaStreams"][1]["Language"] = json!("jpn");
        edited[0]["MediaSourceInfo"]["MediaStreams"][5]["Title"] = json!("手动更新繁中字幕");
        edited[0]["MediaSourceInfo"]["MediaStreams"][5]["Language"] = json!("cht");
        let result = f.save(json!({"mediainfo":edited}), false).await;
        let live = f.native.host.item.lock().await.clone();
        let saved: Value =
            serde_json::from_slice(&tokio::fs::read(info_path(&live)).await.unwrap()).unwrap();
        let resources = images
            .iter()
            .all(|(file, bytes)| std::fs::read(file).is_ok_and(|live| live == *bytes));
        let bif_bytes = tokio::fs::read(&bif).await.unwrap();
        let clear_ids = f.native.host.clear_ids.lock().await.clone();
        let refresh_ids = f.proxy.metadata_ids.lock().await.clone();
        let backup = f.native.state.data_dir.join("backups/media-info-reload");
        let backup_empty = !backup.exists() || std::fs::read_dir(backup).unwrap().next().is_none();
        f.close().await;
        assert_eq!(
            result["result"]["metadata_refresh"]["state"], "verified",
            "{result}"
        );
        for streams in [
            &live["MediaStreams"],
            &saved[0]["MediaSourceInfo"]["MediaStreams"],
        ] {
            assert_eq!(streams[1]["Title"], "日语");
            assert_eq!(streams[1]["Language"], "jpn");
            assert_eq!(streams[5]["Title"], "手动更新繁中字幕");
            assert_eq!(streams[5]["Language"], "cht");
        }
        assert_eq!(live["Chapters"], old["Chapters"]);
        assert!(resources);
        assert_eq!(bif_bytes, original_bif);
        assert_eq!(clear_ids, vec!["61616"]);
        assert_eq!(refresh_ids, vec!["61616"]);
        assert!(
            backup_empty,
            "successful reload must remove its task backup"
        );
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn manual_save_clears_related_versions_once_and_keeps_unrelated_media_unchanged() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.add_episode().await;
        let key = f.native.host.item.lock().await["PresentationUniqueKey"].clone();
        f.proxy.extra.lock().await.get_mut("61617").unwrap()["PresentationUniqueKey"] = key;
        let mut unrelated = f.native.host.item.lock().await.clone();
        unrelated["Id"] = json!("61618");
        unrelated["Guid"] = json!("00000000-0000-0000-0000-00000000f0b2");
        unrelated["PresentationUniqueKey"] = json!("unrelated-key");
        unrelated["Path"] = json!(f.native.directory.path().join("61618.strm"));
        unrelated["MediaSources"][0]["ItemId"] = json!("61618");
        unrelated["MediaSources"][0]["Id"] = json!("mediasource_61618");
        unrelated["MediaSources"][0]["Path"] = json!("/cd2/61618.mkv");
        tokio::fs::write(unrelated["Path"].as_str().unwrap(), "/cd2/61618.mkv")
            .await
            .unwrap();
        let unrelated_json = serde_json::to_vec(&f.native.original).unwrap();
        tokio::fs::write(info_path(&unrelated), &unrelated_json)
            .await
            .unwrap();
        f.proxy
            .extra
            .lock()
            .await
            .insert("61618".into(), unrelated.clone());
        let result = f.save(edited_batch(&f.native.original), false).await;
        let live = f.proxy.extra.lock().await.clone();
        let clears = f.native.host.clear_ids.lock().await.clone();
        let refreshes = f.proxy.metadata_ids.lock().await.clone();
        let after = tokio::fs::read(info_path(&unrelated)).await.unwrap();
        f.close().await;
        assert_eq!(
            result["result"]["metadata_refresh"]["state"], "verified",
            "{result}"
        );
        assert_eq!(clears, vec!["61616"]);
        assert_eq!(refreshes, vec!["61616", "61617"]);
        assert_eq!(live["61617"]["MediaStreams"][1]["Title"], "本次新标题");
        assert_eq!(live["61618"], unrelated);
        assert_eq!(after, unrelated_json);
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn manual_save_keeps_snapshot_and_settings_when_clear_fails_or_stays_incomplete() {
        for rejected in [true, false] {
            let f = ThumbnailFixture::with_flag(Some(false)).await;
            f.proxy.reject_clear.store(rejected, Ordering::SeqCst);
            f.proxy.incomplete_clear.store(!rejected, Ordering::SeqCst);
            let result = f.save(json!({"mediainfo":f.native.original}), false).await;
            let root = reload_recovery(&f);
            let marker: Value = serde_json::from_slice(
                &tokio::fs::read(root.join("media-reload.json"))
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(marker["target_ids"], json!(["61616"]));
            assert!(
                marker["json"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|entry| PathBuf::from(entry["backup"].as_str().unwrap()).is_file())
            );
            assert!(root.join("original-settings.json").is_file());
            let settings = f.native.host.settings.lock().await.clone();
            let refreshes = f.proxy.metadata_ids.lock().await.clone();
            f.close().await;
            assert_eq!(
                result["result"]["metadata_refresh"]["state"], "failed",
                "{result}"
            );
            assert!(
                settings["ExclusiveControlFeatures"]
                    .as_str()
                    .unwrap()
                    .contains("IgnoreFileChange")
            );
            assert_eq!(settings["PersistMediaInfoMode"], "Restore");
            assert!(refreshes.is_empty());
        }
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn manual_save_refuses_to_clear_an_unprepared_related_version() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.add_episode().await;
        // Emby may cap its page size below the requested Limit; the related
        // version must still be discovered on the next page before Clear.
        f.proxy.page_limit.store(1, Ordering::SeqCst);
        let key = f.native.host.item.lock().await["PresentationUniqueKey"].clone();
        f.proxy.extra.lock().await.get_mut("61617").unwrap()["PresentationUniqueKey"] = key;
        let result = f.save(json!({"mediainfo":f.native.original}), false).await;
        let clears = f.native.host.clear_ids.lock().await.clone();
        let refreshes = f.proxy.metadata_ids.lock().await.clone();
        let settings = f.native.host.settings.lock().await.clone();
        f.close().await;
        assert_eq!(
            result["result"]["metadata_refresh"]["state"], "failed",
            "{result}"
        );
        assert!(clears.is_empty());
        assert!(refreshes.is_empty());
        assert!(
            result.to_string().contains("617") || result.to_string().contains("61617"),
            "{result}"
        );
        assert_eq!(settings["PersistMediaInfoMode"], "Default");
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn manual_save_rebuilds_a_native_bif_from_cached_jpegs_without_reading_video() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        let original = f.native.host.item.lock().await.clone();
        tokio::fs::remove_file(local_bif(&original)).await.unwrap();
        let result = f.save(json!({"mediainfo":f.native.original}), false).await;
        let live = f.native.host.item.lock().await.clone();
        let frames = native_bif_frames(&live).expect(
            "native BIF reader must validate the generated header, index and JPEG payloads",
        );
        let image_reads = f.proxy.cached_image_reads.load(Ordering::SeqCst);
        let video_reads = f.proxy.video_reads.load(Ordering::SeqCst);
        let clears = f.native.host.clear_ids.lock().await.clone();
        f.close().await;
        assert_eq!(
            result["result"]["metadata_refresh"]["state"], "verified",
            "{result}"
        );
        assert_eq!(clears, vec!["61616"]);
        assert!(image_reads > 0);
        assert_eq!(video_reads, 0);
        assert_eq!(frames.len(), original["Chapters"].as_array().unwrap().len());
        assert_eq!(live["Chapters"], original["Chapters"]);
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn manual_save_protects_additional_parts_without_copying_the_main_title() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.add_episode().await;
        f.native.host.item.lock().await["PartCount"] = json!(2);
        let part = {
            let mut extra = f.proxy.extra.lock().await;
            let part = extra.get_mut("61617").unwrap();
            part["ParentId"] = json!("61616");
            part["ExtraType"] = json!("AdditionalPart");
            part.clone()
        };
        f.proxy
            .parts
            .lock()
            .await
            .insert("61616".into(), vec![part.clone()]);
        let mut edited = f.native.original.clone();
        edited[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("主文件新标题");
        let result = f.save(json!({"mediainfo":edited}), false).await;
        let live = f.proxy.extra.lock().await["61617"].clone();
        let clears = f.native.host.clear_ids.lock().await.clone();
        let refreshes = f.proxy.metadata_ids.lock().await.clone();
        let original_title =
            f.native.original[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"].clone();
        f.close().await;
        assert_eq!(
            result["result"]["metadata_refresh"]["state"], "verified",
            "{result}"
        );
        assert!(
            clears
                .iter()
                .all(|id| ["61616", "61617"].contains(&id.as_str()))
        );
        assert!(refreshes.contains(&"61616".into()) && refreshes.contains(&"61617".into()));
        assert_eq!(live["MediaStreams"][1]["Title"], original_title);
        assert_eq!(live["Chapters"], part["Chapters"]);
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn saving_one_bound_version_preserves_the_latest_unselected_native_streams_and_cache() {
        let f = ThumbnailFixture::with_flag(Some(false)).await;
        f.add_episode().await;
        let primary = f.native.host.item.lock().await.clone();
        let mut expected = {
            let mut extra = f.proxy.extra.lock().await;
            let item = extra.get_mut("61617").unwrap();
            item["Type"] = json!("Movie");
            item["PresentationUniqueKey"] = primary["PresentationUniqueKey"].clone();
            item["MediaStreams"][1]["Title"] = json!("V2 现场用户标题");
            item["MediaStreams"][1]["Language"] = json!("yue");
            item["MediaStreams"][1]["IsDefault"] = json!(false);
            item["MediaStreams"][5]["IsDefault"] = json!(false);
            item["MediaStreams"][5]["IsForced"] = json!(true);
            item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
            item["MediaStreams"].clone()
        };
        let secondary = f.proxy.extra.lock().await["61617"].clone();
        sqlx::query("DELETE FROM media_metadata WHERE emby_item_ids_json @> $1")
            .bind(json!(["61617"]))
            .execute(&f.native.state.db)
            .await
            .unwrap();
        sqlx::query("UPDATE media_metadata SET emby_item_ids_json=$1,asset_details_json=$2,file_sha1_json=$3,file_pickcode_json=$4")
            .bind(json!(["61616","61617"]))
            .bind(json!([{"emby_item_id":"61616","path":primary["Path"],"size_bytes":primary["Size"]},{"emby_item_id":"61617","path":secondary["Path"],"size_bytes":secondary["Size"]}]))
            .bind(json!([format!("{:040X}",42),format!("{:040X}",43)])).bind(json!([null,null])).execute(&f.native.state.db).await.unwrap();
        f.proxy.mutate_unselected_once.store(true, Ordering::SeqCst);
        expected[1]["Title"] = json!("V2 最新用户标题");
        expected[1]["IsDefault"] = json!(true);
        let mut edited = f.native.original.clone();
        edited[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("V1 编辑标题");
        let result = f.save(json!({"mediainfo":edited}), false).await;
        let first = f.native.host.item.lock().await.clone();
        let second = f.proxy.extra.lock().await["61617"].clone();
        let saved: Value =
            serde_json::from_slice(&std::fs::read(info_path(&second)).unwrap()).unwrap();
        let cached: Value =
            sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind(format!("{:040X}", 43))
                .fetch_one(&f.native.state.db)
                .await
                .unwrap();
        let mutated = !f.proxy.mutate_unselected_once.load(Ordering::SeqCst);
        f.close().await;
        assert_eq!(
            result["result"]["metadata_refresh"]["state"], "verified",
            "{result}"
        );
        assert_eq!(result["result"]["saved_item_ids"], json!(["61616"]));
        assert_eq!(first["MediaStreams"][1]["Title"], "V1 编辑标题");
        assert!(mutated);
        assert_eq!(second["MediaStreams"], expected);
        assert_eq!(saved[0]["MediaSourceInfo"]["MediaStreams"], expected);
        assert_eq!(cached[0]["MediaSourceInfo"]["MediaStreams"], expected);
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
    async fn manual_save_logs_final_readback_differences_success_and_refresh_errors() {
        use emby_toolkit::logs::{LogFormat, LogStore, LogWriter};
        use tracing::instrument::WithSubscriber;

        // Keep callsites enabled while other async tests run without a subscriber.
        static LOG_INTEREST: std::sync::Once = std::sync::Once::new();
        LOG_INTEREST.call_once(|| {
            tracing::subscriber::set_global_default(
                tracing_subscriber::fmt()
                    .with_writer(std::io::sink)
                    .with_max_level(tracing::Level::INFO)
                    .finish(),
            )
            .unwrap();
        });

        for (import, reject, expected_state) in [
            (false, false, "failed"),
            (true, false, "verified"),
            (true, true, "failed"),
        ] {
            let f = ThumbnailFixture::with_flag(Some(false)).await;
            f.proxy.import_metadata.store(import, Ordering::SeqCst);
            f.proxy.reject_refresh.store(reject, Ordering::SeqCst);
            let mut edited = f.native.original.clone();
            edited[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("日志诊断的新标题");
            let directory = tempfile::tempdir().unwrap();
            let subscriber = tracing_subscriber::fmt()
                .event_format(LogFormat)
                .with_writer(LogWriter(LogStore::open(directory.path()).unwrap()))
                .with_max_level(tracing::Level::INFO)
                .finish();
            let state = f.native.state.clone();
            let task = state
                .jobs
                .clone()
                .submit_response("保存日志回归", move |context| async move {
                    media_save::save(&state, "61616", &json!({"mediainfo":edited}), &context)
                        .with_subscriber(subscriber)
                        .await
                })
                .await
                .unwrap();
            let result = tokio::time::timeout(Duration::from_secs(80), async {
                loop {
                    let result = f.native.state.jobs.result(&task).await.unwrap();
                    if result["is_running"] == false {
                        break result;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            let clears = f.native.host.clears.load(Ordering::SeqCst);
            let starts = f.native.host.starts.load(Ordering::SeqCst);
            let refreshed = f
                .proxy
                .metadata_logs
                .lock()
                .await
                .iter()
                .any(|line| line.contains("RefreshItem Complete: 61616"));
            f.close().await;
            let logs = std::fs::read_to_string(directory.path().join("app.log")).unwrap();
            assert_eq!(clears, 1);
            assert_eq!(starts, 0);
            assert_eq!(
                result["result"]["metadata_refresh"]["state"], expected_state,
                "{result}"
            );
            if reject {
                assert_eq!(logs.matches("保存或刷新验收流程中断").count(), 1, "{logs}");
                assert!(logs.contains("目标媒体ID [61616]"), "{logs}");
                assert!(
                    logs.contains(
                        result["result"]["metadata_refresh"]["failures"][0]["reason"]
                            .as_str()
                            .unwrap()
                    ),
                    "{logs}"
                );
            } else if import {
                assert_eq!(
                    logs.matches("媒体ID 61616：媒体流、标题和章节已通过验收")
                        .count(),
                    1,
                    "{logs}"
                );
                assert!(!logs.contains("未通过验收"), "{logs}");
            } else {
                assert!(
                    refreshed,
                    "fixture must finish refresh without importing streams"
                );
                assert_eq!(logs.matches("保存或刷新验收流程中断").count(), 1, "{logs}");
                assert!(logs.contains("音轨数量不一致（期望 4，实际 0）"), "{logs}");
                assert!(!logs.contains("媒体流、标题和章节已通过验收"), "{logs}");
            }
        }
    }
}
