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
    sync_ids: Arc<tokio::sync::Mutex<Vec<String>>>,
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
    if method == axum::http::Method::POST && path == "/Items/SyncMediaInfo" {
        proxy
            .sync_ids
            .lock()
            .await
            .push(query.get("Id").cloned().unwrap_or_default());
    }
    if method == axum::http::Method::POST
        && matches!(
            path.as_str(),
            "/Items/61616/Refresh" | "/Items/61617/Refresh"
        )
        && query
            .get("MetadataRefreshMode")
            .is_some_and(|mode| mode == "Default")
    {
        assert_eq!(query["Recursive"], "false");
        assert_eq!(query["ImageRefreshMode"], "Default");
        assert_eq!(query["ReplaceAllMetadata"], "false");
        assert_eq!(query["ReplaceAllImages"], "false");
        // Ordinary refresh accepts the request but leaves the old media streams.
        // Only the nonempty SyncMediaInfo branches below perform the import.
        return StatusCode::NO_CONTENT.into_response();
    }
    if method == axum::http::Method::GET {
        if path == "/movie/113094" {
            return Json(json!({"id":113094,"title":"重新整理后电影","original_title":"Reorganized Movie","original_language":"en","release_date":"2000-01-01","overview":"已更新简介","genres":[{"id":16,"name":"动画"}],"credits":{"cast":[],"crew":[]},"images":{"posters":[],"backdrops":[],"logos":[]},"keywords":{"keywords":[]},"release_dates":{"results":[]},"external_ids":{"imdb_id":"tt-reorganized"}})).into_response();
        }
        if path == "/Users/user/Items/61617" {
            return Json(api_item(proxy.related.lock().await.clone())).into_response();
        }
        if path == "/Items"
            && query
                .get("Ids")
                .is_some_and(|ids| ids.split(',').any(|id| id == "61617"))
        {
            let mut values = vec![api_item(proxy.related.lock().await.clone())];
            if query["Ids"].split(',').any(|id| id == "61616") {
                values.push(api_item(proxy.original.item.lock().await.clone()));
            }
            return Json(json!({"Items":values,"TotalRecordCount":values.len()})).into_response();
        }
        if path == "/Items/61617/ThumbnailSet" || path == "/Videos/61617/AdditionalParts" {
            return Json(json!({"Thumbnails":[],"Items":[]})).into_response();
        }
        if path == "/Users/user/Items/61617/SpecialFeatures" {
            return Json(json!([])).into_response();
        }
    }
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
    if method == axum::http::Method::POST
        && path == "/Items/SyncMediaInfo"
        && query.get("Id").is_some_and(|id| id == "61617")
    {
        let info: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(info.as_array().unwrap().len(), 1);
        assert!(emby_toolkit::media_info::recoverable(&info));
        let source = &info[0]["MediaSourceInfo"];
        let mut related = proxy.related.lock().await;
        related["MediaStreams"] = source["MediaStreams"].clone();
        related["MediaSources"][0]["MediaStreams"] = source["MediaStreams"].clone();
        related["MediaSources"][0]["Size"] = source["Size"].clone();
        related["MediaSources"][0]["RunTimeTicks"] = source["RunTimeTicks"].clone();
        related["Size"] = source["Size"].clone();
        related["RunTimeTicks"] = source["RunTimeTicks"].clone();
        related["Chapters"] = info[0]["Chapters"].clone();
        return Json(info).into_response();
    }
    if method == axum::http::Method::POST && path == "/Items/61617/Refresh" {
        return StatusCode::NO_CONTENT.into_response();
    }
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn selected_movie_reorganizes_its_related_versions_and_scrapes_without_touching_another_movie()
 {
    let f = Fixture::new(false).await;
    f.single_audio("eng", "旧英语").await;
    f.host.item.lock().await["ProviderIds"] = json!({"Tmdb":"113094"});
    let size = f.original[0]["MediaSourceInfo"]["Size"].as_u64().unwrap();
    let related_path = f.directory.path().join("61617.strm");
    tokio::fs::write(&related_path, "/cd2/61617.mkv")
        .await
        .unwrap();
    let source = json!({"Id":"mediasource_61617","ItemId":"61617","Path":"/cd2/61617.mkv","Container":"mkv","Size":size,"RunTimeTicks":50788670000_u64,"MediaStreams":f.host.item.lock().await["MediaStreams"]});
    let related = Arc::new(tokio::sync::Mutex::new(
        json!({"Id":"61617","Guid":native_guid(&related_path,"Movie"),"Type":"Movie","Name":"关联版本","Path":related_path,"PartCount":1,"Size":size,"RunTimeTicks":50788670000_u64,"MediaStreams":source["MediaStreams"],"MediaSources":[source],"Chapters":[],"ProviderIds":{"Tmdb":"113094"}}),
    ));
    let related_info = json!([{"MediaSourceInfo":source,"Chapters":[]}]);
    tokio::fs::write(
        f.directory.path().join("61617-mediainfo.json"),
        related_info.to_string(),
    )
    .await
    .unwrap();
    for id in ["61616", "61617"] {
        let path = f.directory.path().join(format!("{id}.strm"));
        let sha = format!("{id}-sha");
        sqlx::query("INSERT INTO p115_filesystem_cache(id,parent_id,name,local_path,sha1,pick_code,size) VALUES($1,'0',$2,$3,$4,$5,$6)")
            .bind(id).bind(format!("{id}.mkv")).bind(path.to_string_lossy().as_ref()).bind(&sha).bind(format!("pc{id}")).bind(size as i64).execute(&f.state.db).await.unwrap();
        let raw = json!({"format":{"format_name":"matroska,webm","size":size.to_string(),"duration":"5078.867"},"streams":[{"index":0,"codec_type":"video","codec_name":"h264","width":1920,"height":1080},{"index":1,"codec_type":"audio","codec_name":"aac","channels":2,"sample_rate":"48000","tags":{"language":"jpn","title":"日语"},"disposition":{"default":1}}],"chapters":[]});
        sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,raw_ffprobe_json) VALUES($1,$2)")
            .bind(&sha)
            .bind(raw)
            .execute(&f.state.db)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE media_metadata SET emby_item_ids_json=$1,asset_details_json=$2,file_sha1_json=$3,file_pickcode_json=$4 WHERE tmdb_id='113094' AND item_type='Movie'")
        .bind(json!(["61616","61617"])).bind(json!([{"emby_item_id":"61616","path":f.directory.path().join("61616.strm"),"size_bytes":size},{"emby_item_id":"61617","path":related_path,"size_bytes":size}])).bind(json!(["61616-sha","61617-sha"])).bind(json!(["pc61616","pc61617"])).execute(&f.state.db).await.unwrap();
    let other = f.directory.path().join("other.strm");
    tokio::fs::write(&other, "unrelated playback")
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
    let sync_ids = Arc::new(tokio::sync::Mutex::new(vec![]));
    let proxy = ReorganizeProxy {
        base,
        original: f.host.clone(),
        related: related.clone(),
        writes: writes.clone(),
        sync_ids: sync_ids.clone(),
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
    f.state.save_config(&json!({"emby_server_url":base,"tmdb_api_base_url":base,"tmdb_api_key":"fixture","monitor_sha1_pc_search":true,"douban_enable_online_api":false,"ai_api_key":"","item_delay":0})).await.unwrap();
    let (_, accepted) =
        super::reorganize(State(f.state.clone()), Json(json!({"item_ids":["61616"]})))
            .await
            .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let result = f
                .state
                .jobs
                .result(accepted["task_id"].as_str().unwrap())
                .await
                .unwrap();
            if result["is_running"] == false {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
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
    let mut imports = sync_ids.lock().await.clone();
    imports.sort();
    let nfo = tokio::fs::read_to_string(f.directory.path().join("61616.nfo"))
        .await
        .unwrap_or_default();
    server.abort();
    f.close().await;
    assert_eq!(outcome["result"]["updated"], 1, "{outcome}");
    assert_eq!(outcome["result"]["failed"], 0, "{outcome}");
    assert!(
        matches!(
            outcome["state"].as_str(),
            Some("completed" | "completed_with_warnings")
        ),
        "{outcome}"
    );
    assert_eq!(primary_language, "jpn");
    assert_eq!(related_language, "jpn");
    assert_eq!(
        imports,
        vec!["61616", "61617"],
        "each selected native version must use a nonempty import"
    );
    assert_eq!(actual_title.as_deref(), Some("重新整理后电影"));
    assert!(nfo.contains("重新整理后电影"), "{nfo}");
    assert_eq!(actual_other, original_other);
    assert_eq!(other_bytes, b"unrelated playback");
    assert_eq!(other_info_bytes, b"unrelated JSON");
    assert!(
        !mutations
            .iter()
            .any(|path| path.contains("ClearMediaInfo") || path.contains("9001")),
        "{mutations:?}"
    );
}
