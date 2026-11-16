// Replays the observed E200 failure: no active local transaction, a database
// pending flag, and a previous JSON whose read-only HDR label was not imported.
async fn legacy_range_proxy(
    State((host, upstream, client)): State<(Host, String, reqwest::Client)>,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    if let Some(path) = path.strip_prefix("/tmdb/") {
        let Json(mut metadata) = reorganize_fixture_metadata(Path(path.to_string())).await;
        if path == "tv/101/season/1" {
            metadata["episodes"][0]["episode_number"] = json!(200);
        }
        return Json(metadata).into_response();
    }
    let method = request.method().clone();
    let uri = request.uri().path_and_query().unwrap().as_str().to_string();
    let content_type = request.headers().get("content-type").cloned();
    let body = axum::body::to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
    let mut forwarded = client.request(method.clone(), format!("{upstream}{uri}")).body(body);
    if let Some(content_type) = content_type {
        forwarded = forwarded.header("content-type", content_type);
    }
    let response = forwarded.send().await.unwrap();
    let status = response.status();
    let content_type = response.headers().get("content-type").cloned();
    let mut body = response.bytes().await.unwrap().to_vec();
    let sync = path == "/Items/SyncMediaInfo" && uri.contains("Id=101");
    if method == reqwest::Method::POST && status.is_success()
        && (sync || path == "/Items/101/Refresh"
        && uri.contains("ReplaceThumbnailImages="))
    {
        let missing = *host.resource_fault.lock().await == "ignore-hdr-classification";
        let mut items = host.items.lock().await;
        let item = items.get_mut("101").unwrap();
        for streams in ["MediaStreams", "MediaSources"] {
            let streams = if streams == "MediaSources" {
                &mut item["MediaSources"][0]["MediaStreams"]
            } else {
                &mut item["MediaStreams"]
            };
            for stream in streams.as_array_mut().unwrap() {
                if stream["Type"] != "Video" { continue; }
                if missing {
                    stream["ExtendedVideoType"] = json!("None");
                    stream["ExtendedVideoSubType"] = json!("None");
                }
                // Actual 4.10.2 getter behavior, rather than copying the input
                // VideoRange or the input read-only subtype description.
                stream["VideoRange"] = match stream["ExtendedVideoType"].as_str().unwrap() {
                    "None" => json!("SDR"), "Hdr10" => json!("HDR 10"),
                    kind => panic!("unexpected native type in E200 replay: {kind}"),
                };
                stream["ExtendedVideoSubTypeDescription"] = match stream["ExtendedVideoSubType"].as_str().unwrap() {
                    "None" => json!("None"), "Hdr10" => json!("HDR 10"),
                    kind => panic!("unexpected native subtype in E200 replay: {kind}"),
                };
            }
        }
        if sync {
            let mut exported: Value = serde_json::from_slice(&body).unwrap();
            exported[0]["MediaSourceInfo"]["MediaStreams"] = item["MediaStreams"].clone();
            body = serde_json::to_vec(&exported).unwrap();
            std::fs::write(fixture_sidecar_path(item), &body).unwrap();
        }
    }
    let mut response = Response::builder().status(status);
    if let Some(content_type) = content_type { response = response.header("content-type", content_type); }
    response.body(Body::from(body)).unwrap()
}

fn fixture_sidecar_path(item: &Value) -> std::path::PathBuf {
    let path = std::path::Path::new(item["Path"].as_str().unwrap());
    path.with_file_name(format!(
        "{}-mediainfo.json",
        path.file_stem().unwrap().to_string_lossy()
    ))
}

async fn legacy_database_pending_range_fixture() -> (Fixture, tokio::task::JoinHandle<()>) {
    let mut fixture = Fixture::new(&["complete", "complete"], false, false).await;
    // The initial complete-ingest preparation also needs TMDb. Configure its
    // local mock before that preparation, rather than only after it returns.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bootstrap_base = format!("http://{}/tmdb", listener.local_addr().unwrap());
    let bootstrap = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/tmdb/{*path}", get(reorganize_fixture_metadata)),
        )
            .await.unwrap();
    });
    fixture.state.save_config(&json!({"tmdb_api_base_url":bootstrap_base,"tmdb_api_key":"fixture"}))
        .await.unwrap();
    // Prepare only the selected episode. The neighbor is a preservation
    // sentinel and must not enter this initial MoviePilot files event.
    let neighbor_files = fixture.files.split_off(1);
    let metadata = prepare_reorganize_chain(&fixture, false).await;
    fixture.files.extend(neighbor_files);
    bootstrap.abort();
    fixture.configure_cloud_wash().await;
    configure_ingest_metadata(&fixture, "101").await;
    let sha = "A".repeat(40);
    fixture.cloud_version("10001", &sha, "hevc").await;
    let raw = json!({"format":{"format_name":"matroska,webm","size":"1286350287","duration":"1156.01"},
        "streams":[{"index":0,"codec_type":"video","codec_name":"hevc","width":3840,"height":2160,
            "pix_fmt":"yuv420p10le","profile":"Main 10","level":153,"bit_rate":"8902001",
            "avg_frame_rate":"25/1","color_space":"bt2020nc","color_transfer":"smpte2084",
            "color_primaries":"bt2020","disposition":{"default":1}},
            {"index":1,"codec_type":"audio","codec_name":"aac","profile":"LC","channels":2,
             "channel_layout":"stereo","sample_rate":"48000","tags":{"language":"chi"},"disposition":{"default":1}}],
        "chapters":[{"start_time":"0","tags":{"title":"片头"}},
            {"start_time":"124","tags":{"title":"正片"}},{"start_time":"1066","tags":{"title":"片尾"}}]});
    sqlx::query("UPDATE p115_mediainfo_cache SET raw_ffprobe_json=$1 WHERE sha1=$2")
        .bind(&raw).bind(&sha).execute(&fixture.state.db).await.unwrap();
    fixture.host.cloud_files.lock().await[0]["size"] = json!(1_286_350_287_u64);
    let mut previous = emby_toolkit::probe::build(&raw, &json!({"fn":"101.mkv"}), &json!([])).unwrap();
    previous[0]["MediaSourceInfo"]["Path"] = json!("/cd2/101.mkv");
    let video = &mut previous[0]["MediaSourceInfo"]["MediaStreams"][0];
    video["VideoRange"] = json!("HDR10");
    video["ExtendedVideoType"] = json!("None");
    video["ExtendedVideoSubType"] = json!("None");
    video["ExtendedVideoSubTypeDescription"] = json!("None");
    std::fs::write(fixture.sidecar_path(), serde_json::to_vec(&previous).unwrap(),
    ).unwrap();
    {
        let mut items = fixture.host.items.lock().await;
        let item = items.get_mut("101").unwrap();
        item["IndexNumber"] = json!(200);
        import_fixture_info(item, &previous);
        item["MediaStreams"][0]["VideoRange"] = json!("SDR");
        item["MediaSources"][0]["MediaStreams"][0]["VideoRange"] = json!("SDR");
        let neighbor = items.get_mut("102").unwrap();
        neighbor["Type"] = json!("Episode");
        neighbor["SeriesId"] = json!("301");
        neighbor["ParentId"] = json!("302");
        neighbor["ParentIndexNumber"] = json!(1);
        neighbor["IndexNumber"] = json!(199);
    }
    sqlx::query("UPDATE media_metadata SET episode_number=200,asset_details_json=jsonb_set(asset_details_json,'{0,moviepilot_media_pending}','true'::jsonb),file_sha1_json=$1,file_pickcode_json=$2 WHERE item_type='Episode' AND tmdb_id='1011'")
        .bind(json!([sha])).bind(json!(["10001"])).execute(&fixture.state.db).await.unwrap();
    fixture.wash_previews("102").await;
    let status = media_recovery::status(State(fixture.state.clone())).await.unwrap().0;
    assert!(status["records"].as_array().unwrap().iter().any(|record| record["id"] == "wash-pending:101"));
    assert!(!status["records"].as_array().unwrap().iter().any(|record| {
        record["kind"] == "wash-pending" && record["id"].as_str().unwrap().starts_with("wash:")
    }), "replay uses the observed database pending flag, without active UUID wash records: {status}");
    let upstream = fixture.state.config.read().await["emby_server_url"].as_str().unwrap().to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new().route("/{*path}", axum::routing::any(legacy_range_proxy))
        .with_state((fixture.host.clone(), upstream, reqwest::Client::new()));
    let proxy = tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
    let original_server = std::mem::replace(&mut fixture.server, proxy);
    fixture.state.save_config(&json!({"emby_server_url":base,"tmdb_api_base_url":format!("{base}/tmdb")})).await.unwrap();
    metadata.abort();
    (fixture, original_server)
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; observed E200 HDR import and native getter stay localhost"]
async fn reorganize_database_pending_hdr_difference_does_not_add_a_field_stage_or_touch_neighbor() {
    let (fixture, upstream) = legacy_database_pending_range_fixture().await;
    let neighbor = fixture.host.items.lock().await["102"].clone();
    let neighbor_json = std::fs::read(std::path::Path::new(fixture.files[1].as_str().unwrap()).with_file_name("102-mediainfo.json"),
    ).unwrap();
    let neighbor_bif = std::fs::read(&fixture.host.wash_bif_paths.lock().await["102"]).unwrap();
    let result = public_reorganize_queue(&fixture).await;
    let native = fixture.host.items.lock().await["101"].clone();
    let saved: Value = serde_json::from_slice(&std::fs::read(fixture.sidecar_path()).unwrap()).unwrap();
    let pending: bool = sqlx::query_scalar("SELECT COALESCE((asset_details_json->0->>'moviepilot_media_pending')::boolean,false) FROM media_metadata WHERE item_type='Episode' AND tmdb_id='1011'")
        .fetch_one(&fixture.state.db).await.unwrap();
    let imports = fixture.host.sync_imports.lock().await.clone();
    let unchanged_neighbor = fixture.host.items.lock().await["102"].clone();
    let unchanged_json = std::fs::read(std::path::Path::new(fixture.files[1].as_str().unwrap()).with_file_name("102-mediainfo.json"),
    ).unwrap();
    let unchanged_bif = std::fs::read(&fixture.host.wash_bif_paths.lock().await["102"]).unwrap();
    fixture.close().await; upstream.abort();
    assert_eq!(result["status"], "completed", "{result}");
    assert!(!pending, "the obsolete field gate is retired after the original ingestion flow");
    assert!(imports.is_empty(), "ingestion must not add a media-edit import: {imports:?}");
    assert_eq!(native["MediaStreams"][0]["VideoRange"], "SDR");
    assert_eq!(saved[0]["MediaSourceInfo"]["MediaStreams"][0]["VideoRange"], "HDR10");
    assert_eq!(native["MediaStreams"][0]["ExtendedVideoType"], "None");
    assert_eq!(native["IndexNumber"], 200);
    assert_eq!(native["MediaStreams"].as_array().unwrap().len(), 2);
    assert_eq!(native["Chapters"].as_array().unwrap().len(), 3);
    assert_eq!((unchanged_neighbor, unchanged_json, unchanged_bif), (neighbor, neighbor_json, neighbor_bif));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; untitled E200 native export must reach configured title filling"]
async fn reorganize_untitled_native_tracks_run_title_fill_and_configured_followup_stages() {
    let (fixture, upstream) = legacy_database_pending_range_fixture().await;
    fixture
        .host
        .native_empty_titles
        .store(true, Ordering::SeqCst);
    fixture
        .state
        .save_config(&json!({"auto_fill_language_titles_on_ingest":true,
            "auto_extract_intro_fingerprint_on_ingest":true,"auto_extract_credits_on_ingest":true,
            "auto_extract_video_thumbnails_on_ingest":true}))
        .await
        .unwrap();
    let neighbor = fixture.host.items.lock().await["102"].clone();
    let result = public_reorganize_queue(&fixture).await;
    let native = fixture.host.items.lock().await["101"].clone();
    let after_neighbor = fixture.host.items.lock().await["102"].clone();
    let imports = fixture.host.sync_imports.lock().await.clone();
    let starts = fixture.host.native_starts.lock().await.clone();
    fixture.close().await;
    upstream.abort();
    assert_eq!(result["status"], "completed", "{result}");
    assert_eq!(
        result["result"]["finalization"]["title_fill"]["completed"], true,
        "{result}"
    );
    assert_ne!(
        result["result"]["finalization"]["title_fill"]["skipped"], true,
        "{result}"
    );
    assert_eq!(
        starts,
        vec!["intro", "thumb"],
        "configured native stages follow title filling: {result}"
    );
    assert!(imports.iter().all(|id| id == "101"), "configured title and credits processing must stay on the selected file: {imports:?}");
    assert_eq!(native["IndexNumber"], 200);
    assert!(native["MediaStreams"].as_array().unwrap().iter().any(|stream| stream["Type"] == "Audio" && !stream["Title"].as_str().unwrap_or("").is_empty()));
    assert_eq!(after_neighbor, neighbor);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; missing native HDR fields are not an ingestion gate"]
async fn reorganize_database_pending_hdr_does_not_add_field_reminders_or_block_ingestion() {
    let (fixture, upstream) = legacy_database_pending_range_fixture().await;
    *fixture.host.resource_fault.lock().await = "ignore-hdr-classification".into();
    let result = public_reorganize_queue(&fixture).await;
    let pending: bool = sqlx::query_scalar("SELECT COALESCE((asset_details_json->0->>'moviepilot_media_pending')::boolean,false) FROM media_metadata WHERE item_type='Episode' AND tmdb_id='1011'")
        .fetch_one(&fixture.state.db).await.unwrap();
    let native = fixture.host.items.lock().await["101"].clone();
    let imports = fixture.host.sync_imports.lock().await.clone();
    fixture.close().await; upstream.abort();
    assert_eq!(result["status"], "completed", "{result}");
    assert!(result["result"]["failures"].as_array().unwrap().is_empty(), "{result}");
    assert_eq!(result["result"]["warnings"].as_u64().unwrap_or(0), 0, "{result}");
    assert_eq!(result["result"]["finalization"]["metadata_refresh"]["completed"], true, "{result}");
    assert!(imports.is_empty(), "no extra field import: {imports:?}");
    assert!(!pending, "an obsolete field gate must not remain after ingestion");
    assert!(!result["result"]["warning_details"].to_string().contains("字段补充"), "{result}");
    assert_eq!(native["MediaStreams"][0]["ExtendedVideoType"], "None");
    assert_eq!(native["MediaStreams"][0]["VideoRange"], "SDR");
}
