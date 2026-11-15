extern crate self as emby_toolkit;
include!("../title_reverse_restore.rs");

#[test]
fn http_success_does_not_hide_failed_or_unverified_native_readback() {
    for state in ["failed", "unverified"] {
        let result = json!({
            "message":"媒体信息 JSON 已保存，Emby 刷新或回读尚未确认，请查看提醒",
            "failures":[],
            "warnings":1,
            "warning_details":["61616：Emby 回读仍返回原音轨标题"],
            "metadata_refresh":{"state":state}
        });
        let error = super::ensure_rebuild_completed(200, &result).unwrap_err();
        assert!(
            error.1.contains("61616") && error.1.contains("原音轨标题"),
            "{}",
            error.1
        );
    }
}

#[test]
fn verified_native_readback_with_cleanup_warning_allows_followup_scrape() {
    let result = json!({
        "message":"媒体信息 JSON 已保存，Emby 媒体流、标题和章节已通过回读验收",
        "failures":[],
        "warnings":1,
        "warning_details":["ETK 恢复暂存文件清理失败，已保留恢复备份"],
        "metadata_refresh":{"state":"verified"}
    });
    super::ensure_rebuild_completed(200, &result).unwrap();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; all unchanged title targets are localhost"]
async fn unchanged_title_reload_finishes_when_the_bound_version_list_contains_itself() {
    let f = Fixture::new(false).await;
    f.single_audio("jpn", "日语").await;
    let target = crate::media_edit::repair_target(&f.state, "61616")
        .await
        .unwrap();
    assert!(
        target
            .versions
            .iter()
            .any(|version| version["emby_id"] == "61616"),
        "must reproduce the self-reinserting version list"
    );
    let before = f.host.item.lock().await.clone();
    let path = f.directory.path().join("61616-mediainfo.json");
    let original = tokio::fs::read(&path).await.unwrap();
    let context = crate::jobs::JobContext::for_recovery(None).await;
    let confirmed = AtomicBool::new(true);
    let recovery = f.directory.path().join("unchanged-title-recovery");
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        crate::media_save::reload_filled_titles(
            &f.state,
            &["61616".into()],
            true,
            &context,
            &confirmed,
            &recovery,
        ),
    )
    .await
    .expect("a processed unchanged version must not reinsert itself forever")
    .unwrap();
    assert!(result.is_empty(), "{result:?}");
    assert_eq!(*f.host.item.lock().await, before);
    assert_eq!(tokio::fs::read(&path).await.unwrap(), original);
    assert_eq!(f.host.clears.load(Ordering::SeqCst), 0);
    assert_eq!(f.host.syncs.load(Ordering::SeqCst), 0);
    assert_eq!(f.host.starts.load(Ordering::SeqCst), 0);
    assert!(
        !recovery.exists(),
        "an unchanged title needs no native recovery operation"
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn partial_native_restore_keeps_target_failure_and_stops_followup_scrape() {
    let f = Fixture::new(false).await;
    let state = f.state.clone();
    let info = f.original.clone();
    let continued = Arc::new(AtomicBool::new(false));
    let scrape_started = continued.clone();
    let task = f
        .state
        .jobs
        .submit("重建部分结果验收", move |context| async move {
            let body =
                json!({"mediainfo":info,"target_item_ids":["61616","61617"],"stream_patches":[]});
            let rebuilt =
                std::collections::BTreeMap::from([("61616".into(), body["mediainfo"].clone())]);
            let (status, result) = emby_toolkit::media_save::save_with_rebuilt(
                &state, "61616", &body, &rebuilt, &context,
            )
            .await?;
            super::ensure_rebuild_completed(status, &result)?;
            scrape_started.store(true, Ordering::SeqCst);
            Ok(json!({"updated":1,"message":"后续元数据处理成功"}))
        })
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let result = f.state.jobs.result(&task).await.unwrap();
            if result["is_running"] == false {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let saved: Value = serde_json::from_slice(
        &tokio::fs::read(f.directory.path().join("61616-mediainfo.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    let starts = f.host.starts.load(Ordering::SeqCst);
    f.close().await;
    assert_eq!(result["state"], "failed", "{result}");
    let error = result["result"]["error"].as_str().unwrap();
    assert!(
        error.contains("HTTP 207") && error.contains("61617") && error.contains("HTTP 404"),
        "{error}"
    );
    assert!(
        !continued.load(Ordering::SeqCst),
        "the partial restore must not be replaced by a successful scrape result"
    );
    assert_eq!(
        starts, 1,
        "the valid target was actually imported before the 207 result"
    );
    assert!(emby_toolkit::media_info::recoverable(&saved));
}

#[derive(Clone)]
struct ReorganizeProxy {
    base: String,
    original: Host,
    related: Arc<tokio::sync::Mutex<Value>>,
    writes: Arc<tokio::sync::Mutex<Vec<String>>>,
    import_ids: Arc<tokio::sync::Mutex<Vec<String>>>,
    cloud_files: Arc<tokio::sync::Mutex<Vec<Value>>>,
}

async fn reorganize_proxy(
    State(proxy): State<ReorganizeProxy>,
    request: axum::extract::Request,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let path = request.uri().path().to_string();
    let query = url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
        .into_owned()
        .collect::<HashMap<_, _>>();
    let method = request.method().clone();
    if method != axum::http::Method::GET {
        proxy.writes.lock().await.push(path.clone());
    }
    if method == axum::http::Method::GET {
        if path == "/swagger.json" {
            let mut contract: Value =
                serde_json::from_str(include_str!("pro_sync_contract.json")).unwrap();
            contract["paths"]["/Items/{Id}/Refresh"] = json!({"post":{"parameters":[
                {"name":"Id","in":"path","type":"string"},
                {"name":"MetadataRefreshMode","in":"query","type":"string"},
                {"name":"ImageRefreshMode","in":"query","type":"string"},
                {"name":"ReplaceAllMetadata","in":"query","type":"boolean"},
                {"name":"ReplaceAllImages","in":"query","type":"boolean"}
            ]}});
            return Json(contract).into_response();
        }
        if path == "/files" {
            let files = if query.get("cid").is_some_and(|cid| cid == "9") {
                proxy.cloud_files.lock().await.clone()
            } else {
                vec![]
            };
            return Json(json!({"state":true,"count":files.len(),"data":files})).into_response();
        }
        if path == "/files/file" {
            let file = proxy
                .cloud_files
                .lock()
                .await
                .iter()
                .find(|file| query.get("file_id").is_some_and(|id| file["id"] == *id))
                .cloned();
            return Json(json!({"state":file.is_some(),"data":file})).into_response();
        }
        if path == "/movie/113094" {
            return Json(json!({"id":113094,"title":"重新整理后电影","original_title":"Reorganized Movie","original_language":"en","release_date":"2000-01-01","overview":"已更新简介","genres":[{"id":16,"name":"动画"}],"credits":{"cast":[],"crew":[]},"images":{"posters":[],"backdrops":[],"logos":[]},"keywords":{"keywords":[]},"release_dates":{"results":[]},"external_ids":{"imdb_id":"tt-reorganized"}})).into_response();
        }
        if path == "/Users/user" {
            return Json(json!({"Id":"user","Policy":{"IsAdministrator":true,"MaxParentalRating":null,"BlockUnratedItems":[],"BlockedTags":[],"IsTagBlockingModeInclusive":false,"AllowTagOrRating":false}})).into_response();
        }
        if path == "/Users/user/Items/61617" {
            return Json(api_item(proxy.related.lock().await.clone())).into_response();
        }
        if path == "/Items" {
            let original = proxy.original.item.lock().await.clone();
            let related = proxy.related.lock().await.clone();
            let values = [original, related]
                .into_iter()
                .filter(|item| {
                    query
                        .get("Ids")
                        .is_none_or(|ids| ids.split(',').any(|id| native_item_id_matches(item, id)))
                })
                .map(api_item)
                .collect::<Vec<_>>();
            return Json(json!({"Items":values,"TotalRecordCount":values.len()})).into_response();
        }
        if path == "/Videos/61616/AdditionalParts" || path == "/Videos/61617/AdditionalParts" {
            return Json(json!({"Items":[],"TotalRecordCount":0})).into_response();
        }
        if path == "/Items/61616/ThumbnailSet" || path == "/Items/61617/ThumbnailSet" {
            return Json(json!({"Thumbnails":[]})).into_response();
        }
        if path == "/Users/user/Items/61617/SpecialFeatures" {
            return Json(json!([])).into_response();
        }
    }
    if method == axum::http::Method::POST
        && matches!(
            path.as_str(),
            "/Items/61616/Refresh" | "/Items/61617/Refresh"
        )
    {
        let id = if path.contains("61617") {
            "61617"
        } else {
            "61616"
        };
        assert_eq!(query["Recursive"], "false");
        assert_eq!(query["ReplaceAllMetadata"], "false");
        assert_eq!(query["ReplaceAllImages"], "false");
        proxy
            .original
            .refresh_logs
            .lock()
            .await
            .push(format!("Info fixture: RefreshItem Start: {id} fixture"));
        let mut item = if id == "61617" {
            proxy.related.lock().await
        } else {
            proxy.original.item.lock().await
        };
        if query["MetadataRefreshMode"] == "Default"
            && query["ImageRefreshMode"] == "ValidationOnly"
        {
            assert_eq!(
                proxy.original.settings.lock().await["PersistMediaInfoMode"],
                "Restore"
            );
            let info: Value = serde_json::from_slice(
                &tokio::fs::read(
                    proxy
                        .original
                        .directory
                        .join(format!("{id}-mediainfo.json")),
                )
                .await
                .unwrap(),
            )
            .unwrap();
            assert!(emby_toolkit::media_info::recoverable(&info));
            let mut source = info[0]["MediaSourceInfo"].clone();
            item["MediaStreams"] = source["MediaStreams"].clone();
            item["Size"] = source["Size"].clone();
            item["RunTimeTicks"] = source["RunTimeTicks"].clone();
            item["Chapters"] = info[0]["Chapters"].clone();
            source["Id"] = json!(format!("mediasource_{id}"));
            source["ItemId"] = json!(id);
            item["MediaSources"] = json!([source]);
            proxy.import_ids.lock().await.push(id.into());
        } else {
            assert!(matches!(
                query["MetadataRefreshMode"].as_str(),
                "Default" | "FullRefresh"
            ));
            assert_eq!(query["ImageRefreshMode"], query["MetadataRefreshMode"]);
            let nfo = tokio::fs::read_to_string(proxy.original.directory.join(format!("{id}.nfo")))
                .await
                .unwrap();
            let tag = |name: &str| {
                regex::Regex::new(&format!(r"(?s)<{name}[^>]*>(.*?)</{name}>"))
                    .unwrap()
                    .captures(&nfo)
                    .map(|value| quick_xml::escape::unescape(&value[1]).unwrap().into_owned())
                    .unwrap_or_default()
            };
            item["Name"] = json!(tag("title"));
            item["Overview"] = json!(tag("plot"));
            if let Ok(year) = tag("year").parse::<i32>() {
                item["ProductionYear"] = json!(year);
            }
        }
        item["DateLastRefreshed"] = json!(format!(
            "after-{id}-{}",
            proxy.original.refresh_logs.lock().await.len()
        ));
        drop(item);
        proxy
            .original
            .refresh_logs
            .lock()
            .await
            .push(format!("Info fixture: RefreshItem Complete: {id} fixture"));
        return StatusCode::NO_CONTENT.into_response();
    }
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
    let response = reqwest::Client::new()
        .request(method, format!("{}{}", proxy.base, parts.uri))
        .headers(parts.headers)
        .body(body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let mut output =
        axum::response::Response::new(axum::body::Body::from(response.bytes().await.unwrap()));
    *output.status_mut() = status;
    *output.headers_mut() = headers;
    output
}

async fn reorganize_source(
    directory: &std::path::Path,
    id: &str,
    color: &str,
) -> (String, u64, Value) {
    use sha1::{Digest, Sha1};
    let path = directory.join(format!("source-{id}.mkv"));
    let output = tokio::process::Command::new("ffmpeg")
        .kill_on_drop(true)
        .args([
            "-nostdin",
            "-hide_banner",
            "-v",
            "error",
            "-filter_threads",
            "1",
            "-filter_complex_threads",
            "1",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg(format!("color=c={color}:s=16x16:r=10:d=0.3"))
        .args([
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=48000:cl=stereo",
            "-t",
            "0.3",
            "-threads",
            "1",
            "-c:v",
            "mpeg4",
            "-c:a",
            "aac",
            "-metadata:s:a:0",
            "language=jpn",
            "-metadata:s:a:0",
            "title=日语",
            "-y",
        ])
        .arg(&path)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = tokio::fs::read(&path).await.unwrap();
    let probe = tokio::process::Command::new("ffprobe")
        .kill_on_drop(true)
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-show_chapters",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .await
        .unwrap();
    assert!(probe.status.success());
    (
        format!("{:X}", Sha1::digest(&bytes)),
        bytes.len() as u64,
        serde_json::from_slice(&probe.stdout).unwrap(),
    )
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn selected_movie_reorganizes_its_related_versions_and_scrapes_without_touching_another_movie()
 {
    let mut f = Fixture::new(false).await;
    f.single_audio("eng", "旧英语").await;
    f.host.item.lock().await["ProviderIds"] = json!({"Tmdb":"113094"});
    let mut current_files = vec![];
    for (id, color) in [("61616", "black"), ("61617", "blue")] {
        let (sha, size, raw) = reorganize_source(f.directory.path(), id, color).await;
        current_files.push(json!({"id":id,"parent_id":"9","name":format!("{id}.mkv"),"pick_code":id,"sha1":sha,"size":size,"fc":"1"}));
        sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,raw_ffprobe_json) VALUES($1,$2)")
            .bind(&sha)
            .bind(raw)
            .execute(&f.state.db)
            .await
            .unwrap();
    }
    let size = current_files[0]["size"].as_u64().unwrap();
    {
        let mut item = f.host.item.lock().await;
        item["Size"] = json!(size);
        item["MediaSources"][0]["Size"] = json!(size);
        item["Chapters"] = json!([]);
    }
    f.original[0]["MediaSourceInfo"]["Size"] = json!(size);
    f.original[0]["Chapters"] = json!([]);
    tokio::fs::write(
        f.directory.path().join("61616-mediainfo.json"),
        f.original.to_string(),
    )
    .await
    .unwrap();
    f.host.settings.lock().await["ExclusiveExtract"] = json!(true);
    let related_path = f.directory.path().join("61617.strm");
    tokio::fs::write(&related_path, "/cd2/61617.mkv")
        .await
        .unwrap();
    let source = json!({"Id":"mediasource_61617","ItemId":"61617","Path":"/cd2/61617.mkv","Container":"mkv","Size":current_files[1]["size"],"RunTimeTicks":50788670000_u64,"MediaStreams":f.host.item.lock().await["MediaStreams"]});
    let related = Arc::new(tokio::sync::Mutex::new(
        json!({"Id":"61617","Guid":native_guid(&related_path,"Movie"),"Type":"Movie","Name":"关联版本","PresentationUniqueKey":"fixture-key-61617","Path":related_path,"PartCount":1,"Size":current_files[1]["size"],"RunTimeTicks":50788670000_u64,"MediaStreams":source["MediaStreams"],"MediaSources":[source],"Chapters":[],"ProviderIds":{"Tmdb":"113094"}}),
    ));
    let mut related_info = json!([{"MediaSourceInfo":source,"Chapters":[]}]);
    for field in ["Id", "ItemId"] {
        related_info[0]["MediaSourceInfo"]
            .as_object_mut()
            .unwrap()
            .remove(field);
    }
    tokio::fs::write(
        f.directory.path().join("61617-mediainfo.json"),
        related_info.to_string(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE media_metadata SET emby_item_ids_json=$1,asset_details_json=$2,file_sha1_json=$3,file_pickcode_json=$4 WHERE tmdb_id='113094' AND item_type='Movie'")
        .bind(json!(["61616","61617"])).bind(json!([{"emby_item_id":"61616","path":f.directory.path().join("61616.strm"),"size_bytes":size},{"emby_item_id":"61617","path":related_path,"size_bytes":current_files[1]["size"]}])).bind(json!([null,null])).bind(json!([null,null])).execute(&f.state.db).await.unwrap();
    let other = f.directory.path().join("other.strm");
    tokio::fs::write(&other, "unrelated playback")
        .await
        .unwrap();
    let other_nfo = other.with_extension("nfo");
    tokio::fs::write(&other_nfo, "unrelated NFO bytes")
        .await
        .unwrap();
    let other_info = f.directory.path().join("other-mediainfo.json");
    tokio::fs::write(&other_info, "unrelated JSON")
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,emby_item_ids_json,asset_details_json) VALUES('9001','Movie','原作品',$1,$2)").bind(json!(["9001"])).bind(json!([{"emby_item_id":"9001","path":other}])).execute(&f.state.db).await.unwrap();
    let original_other: Value =
        sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id='9001'")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    let base = f.state.config.read().await["emby_server_url"]
        .as_str()
        .unwrap()
        .to_string();
    let writes = Arc::new(tokio::sync::Mutex::new(vec![]));
    let import_ids = Arc::new(tokio::sync::Mutex::new(vec![]));
    let proxy = ReorganizeProxy {
        base,
        original: f.host.clone(),
        related: related.clone(),
        writes: writes.clone(),
        import_ids: import_ids.clone(),
        cloud_files: Arc::new(tokio::sync::Mutex::new(current_files.clone())),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(reorganize_proxy).with_state(proxy),
        )
        .await
        .unwrap();
    });
    f.state.p115 = Arc::new(crate::p115::Service::with_endpoints(
        crate::p115::Endpoints {
            qrcode: base.clone(),
            passport: base.clone(),
            openapi: base.clone(),
            webapi: base.clone(),
            profile: base.clone(),
        },
    ));
    crate::db::save_setting(
        &f.state.db,
        "p115_auth_tokens",
        &json!({"cookie":"UID=1_A1_fixture; CID=fixture; SEID=fixture"}),
    )
    .await
    .unwrap();
    f.state.save_config(&json!({"emby_server_url":base,"tmdb_api_base_url":base,"tmdb_api_key":"fixture","monitor_sha1_pc_search":true,"cd2_media_root":"/cd2","p115_fingerprint_media_root_cid":"9","p115_api_priority":"cookie","douban_enable_online_api":false,"ai_api_key":"","item_delay":0})).await.unwrap();
    let token = f.state.sessions.sign(&crate::auth::Session {
        user_id: "fixture-operator".into(),
        name: "Fixture Operator".into(),
        setup_mode: false,
        expires: chrono::Utc::now().timestamp() + 120,
    });
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::COOKIE,
        format!("etk_session={token}").parse().unwrap(),
    );
    let (_, accepted) = super::reorganize(
        State(f.state.clone()),
        headers,
        Json(json!({"item_ids":["61616"]})),
    )
    .await
    .unwrap();
    let queue_id = accepted["queue_ids"][0].as_str().unwrap().to_string();
    let cancellation = tokio_util::sync::CancellationToken::new();
    let worker = tokio::spawn(crate::media_organize_queue::run(
        f.state.clone(),
        cancellation.clone(),
    ));
    let outcome = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let result: Value =
                sqlx::query_scalar("SELECT to_jsonb(q) FROM media_organize_queue q WHERE id=$1")
                    .bind(&queue_id)
                    .fetch_one(&f.state.db)
                    .await
                    .unwrap();
            if !matches!(result["status"].as_str(), Some("queued" | "running")) {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    cancellation.cancel();
    worker.await.unwrap();
    let actual_other: Value =
        sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id='9001'")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    let actual_title: Option<String> = sqlx::query_scalar(
        "SELECT title FROM media_metadata WHERE tmdb_id='113094' AND item_type='Movie'",
    )
    .fetch_one(&f.state.db)
    .await
    .unwrap();
    let primary_language = f.host.item.lock().await["MediaStreams"][1]["Language"].clone();
    let related_language = related.lock().await["MediaStreams"][1]["Language"].clone();
    let other_bytes = tokio::fs::read(other).await.unwrap();
    let other_info_bytes = tokio::fs::read(other_info).await.unwrap();
    let mutations = writes.lock().await.clone();
    let mut imports = import_ids.lock().await.clone();
    imports.sort();
    let nfo = tokio::fs::read_to_string(f.directory.path().join("61616.nfo"))
        .await
        .unwrap_or_default();
    let related_nfo = tokio::fs::read_to_string(f.directory.path().join("61617.nfo"))
        .await
        .unwrap_or_default();
    let other_nfo_bytes = tokio::fs::read(other_nfo).await.unwrap();
    server.abort();
    f.close().await;
    assert_eq!(outcome["result"]["updated"], 1, "{outcome}");
    assert_eq!(
        outcome["result"]["failed"].as_u64().unwrap_or(0),
        0,
        "{outcome}"
    );
    assert!(
        crate::media_edit::list(&outcome["result"]["failures"]).is_empty(),
        "{outcome}"
    );
    assert!(outcome["status"] == "completed", "{outcome}");
    assert_eq!(primary_language, "jpn");
    assert_eq!(related_language, "jpn");
    assert_eq!(
        imports,
        vec!["61616", "61617"],
        "each selected native version must import its verified current file JSON"
    );
    assert_eq!(actual_title.as_deref(), Some("重新整理后电影"));
    assert!(nfo.contains("重新整理后电影"), "{nfo}");
    assert!(related_nfo.contains("重新整理后电影"), "{related_nfo}");
    assert_eq!(other_nfo_bytes, b"unrelated NFO bytes");
    assert_eq!(actual_other, original_other);
    assert_eq!(other_bytes, b"unrelated playback");
    assert_eq!(other_info_bytes, b"unrelated JSON");
    assert!(
        !mutations.iter().any(|path| path.contains("ClearMediaInfo")
            || path.contains("SyncMediaInfo")
            || path.contains("9001")),
        "{mutations:?}"
    );
}
