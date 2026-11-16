/// Two real native episodes share a presentation key; only the empty new file is notified.
async fn gaia_empty_version_fixture() -> (Fixture, Value, Value, Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut fixture = Fixture::new(&["complete", "complete"], false, false).await;
    fixture.configure_series_chain().await;
    fixture.configure_cloud_wash().await;
    let cloud = fixture
        .cloud_version("10001", &"A".repeat(40), "hevc")
        .await;
    let (_, _, jpeg_bytes, bif_bytes) = fixture.wash_previews("102").await;
    let old_json = std::fs::read(
        std::path::Path::new(fixture.files[1].as_str().unwrap())
            .with_file_name("102-mediainfo.json"),
    )
    .unwrap();
    let guid = uuid::Uuid::new_v4().simple().to_string();
    let metadata = fixture._directory.path().join("metadata");
    std::fs::create_dir_all(metadata.join("library").join(&guid[..2]).join(&guid)).unwrap();
    let mut items = fixture.host.items.lock().await;
    let key = items["101"]["PresentationUniqueKey"].clone();
    let old = items.get_mut("102").unwrap();
    old["Type"] = json!("Episode");
    old["SeriesId"] = json!("301");
    old["ParentId"] = json!("302");
    old["ParentIndexNumber"] = json!(1);
    old["IndexNumber"] = json!(1);
    old["ProviderIds"] = json!({"Tmdb":"1011"});
    old["PresentationUniqueKey"] = key;
    let old = old.clone();
    let mut grouping = old["MediaSources"][0].clone();
    grouping["Type"] = json!("Grouping");
    let new = items.get_mut("101").unwrap();
    new["Guid"] = json!(guid);
    new["Size"] = json!(0);
    new["RunTimeTicks"] = Value::Null;
    new["MediaStreams"] = json!([]);
    new["Chapters"] = json!([]);
    let mut source = new["MediaSources"][0].clone();
    source["Type"] = json!("Default");
    source["Container"] = json!("strm");
    source["Size"] = json!(0);
    source["RunTimeTicks"] = Value::Null;
    source["MediaStreams"] = json!([]);
    source["Chapters"] = json!([]);
    new["MediaSources"] = json!([grouping, source]);
    drop(items);
    sqlx::query("UPDATE media_metadata SET emby_item_ids_json=$1,asset_details_json=$2 WHERE tmdb_id='1011' AND item_type='Episode'")
        .bind(json!(["102"]))
        .bind(json!([{"emby_item_id":"102","path":old["Path"],"size_bytes":old["Size"]}]))
        .execute(&fixture.state.db).await.unwrap();
    let mut event = fixture.cloud_event(cloud, "gaia-single-empty-version");
    event["files"] = json!([fixture.files[0]]);
    event["media_type"] = json!("series");
    (fixture, event, old, old_json, jpeg_bytes, bif_bytes)
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_gaia_empty_new_episode_syncs_only_its_id_and_preserves_old_version() {
    let (fixture, event, old, old_json, jpeg_bytes, bif_bytes) = gaia_empty_version_fixture().await;
    let result = fixture.run_moviepilot_event(event).await;
    let own = fixture.host.items.lock().await["101"].clone();
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["updated"], 3, "{result}");
    assert_eq!(*fixture.host.sync_imports.lock().await, vec!["101"]);
    assert!(
        fixture.host.file_imports.lock().await.is_empty(),
        "no expanded file refresh"
    );
    assert!(
        !fixture
            .host
            .refresh_queries
            .lock()
            .await
            .iter()
            .any(|(id, _)| id == "102"),
        "old version cannot be refreshed"
    );
    assert_eq!(own["Size"], 1000);
    assert_eq!(own["MediaStreams"][0]["Codec"], "hevc");
    assert_eq!(fixture.host.items.lock().await["102"], old);
    assert_eq!(
        std::fs::read(
            std::path::Path::new(fixture.files[1].as_str().unwrap())
                .with_file_name("102-mediainfo.json")
        )
        .unwrap(),
        old_json
    );
    assert_eq!(
        std::fs::read(
            fixture
                .host
                .chapter_image_paths
                .lock()
                .await
                .get(&("102".into(), 0))
                .unwrap()
        )
        .unwrap(),
        jpeg_bytes
    );
    assert_eq!(
        std::fs::read(fixture.host.wash_bif_paths.lock().await.get("102").unwrap()).unwrap(),
        bif_bytes
    );
    assert_eq!(
        media_recovery::status(State(fixture.state.clone()))
            .await
            .unwrap()
            .0["pending"],
        false
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; existing untitled media must use the normal MoviePilot entry"]
async fn moviepilot_existing_untitled_episode_enters_ingestion_without_a_media_edit_scope_check() {
    let (fixture, event, old, old_json, _, _) = gaia_empty_version_fixture().await;
    let sha = "A".repeat(40);
    let mut raw: Value =
        sqlx::query_scalar("SELECT raw_ffprobe_json FROM p115_mediainfo_cache WHERE sha1=$1")
            .bind(&sha)
            .fetch_one(&fixture.state.db)
            .await
            .unwrap();
    raw["streams"][1]["tags"]
        .as_object_mut()
        .unwrap()
        .remove("title");
    sqlx::query("UPDATE p115_mediainfo_cache SET raw_ffprobe_json=$1 WHERE sha1=$2")
        .bind(&raw)
        .bind(&sha)
        .execute(&fixture.state.db)
        .await
        .unwrap();
    let mut info = emby_toolkit::probe::build(&raw, &json!({"fn":"101.mkv"}), &json!([])).unwrap();
    info[0]["MediaSourceInfo"]["Path"] = json!("/cd2/101.mkv");
    {
        let mut items = fixture.host.items.lock().await;
        let item = items.get_mut("101").unwrap();
        let mut source = info[0]["MediaSourceInfo"].clone();
        source["Id"] = item["MediaSources"][1]["Id"].clone();
        source["ItemId"] = json!("101");
        source["Type"] = json!("Default");
        for stream in source["MediaStreams"].as_array_mut().unwrap() {
            if stream["Title"] == "" {
                stream.as_object_mut().unwrap().remove("Title");
            }
        }
        item["Size"] = source["Size"].clone();
        item["RunTimeTicks"] = source["RunTimeTicks"].clone();
        item["MediaStreams"] = source["MediaStreams"].clone();
        item["Chapters"] = info[0]["Chapters"].clone();
        item["MediaSources"][1] = source;
    }
    std::fs::write(fixture.sidecar_path(), serde_json::to_vec(&info).unwrap()).unwrap();
    fixture.state.save_config(&json!({"emby_metadata_path":fixture._directory.path().join("not-mounted-edit-directory")})).await.unwrap();
    let result = fixture.run_moviepilot_event(event).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert!(fixture.host.sync_imports.lock().await.is_empty());
    assert!(fixture.host.file_imports.lock().await.is_empty());
    assert_eq!(fixture.host.items.lock().await["102"], old);
    assert_eq!(
        std::fs::read(
            std::path::Path::new(fixture.files[1].as_str().unwrap())
                .with_file_name("102-mediainfo.json")
        )
        .unwrap(),
        old_json
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_empty_sync_accepts_native_location_and_playback_fields_without_changing_requested_info()
 {
    let (fixture, event, old, old_json, _, _) = gaia_empty_version_fixture().await;
    let sha = "A".repeat(40);
    // cloud_version stores current-SHA raw ffprobe data; the actual ingest
    // rebuilds canonical media info from it, including IsRemote=true.
    let raw: Value =
        sqlx::query_scalar("SELECT raw_ffprobe_json FROM p115_mediainfo_cache WHERE sha1=$1")
            .bind(&sha)
            .fetch_one(&fixture.state.db)
            .await
            .unwrap();
    assert!(
        raw["streams"]
            .as_array()
            .is_some_and(|streams| !streams.is_empty())
    );
    fixture
        .host
        .enrich_sync_native
        .store(true, Ordering::SeqCst);
    let result = fixture.run_moviepilot_event(event).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["updated"], 3, "{result}");
    assert_eq!(*fixture.host.sync_imports.lock().await, vec!["101"]);
    assert!(fixture.host.file_imports.lock().await.is_empty());
    let native = fixture.host.items.lock().await["101"].clone();
    let source = native["MediaSources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["ItemId"] == "101")
        .unwrap();
    assert_eq!(source["IsRemote"], false);
    assert_eq!(source["MimeType"], "video/x-matroska");
    assert!(!source["DefaultAudioStreamIndex"].is_null());
    assert_eq!(source["Chapters"], native["Chapters"]);
    let saved: Value =
        serde_json::from_slice(&std::fs::read(fixture.sidecar_path()).unwrap()).unwrap();
    assert_eq!(saved[0]["MediaSourceInfo"]["IsRemote"], true);
    // This helper runs the ingest business function directly, without the
    // webhook history wrapper. Verify its actual unique episode binding.
    let admitted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM media_metadata WHERE item_type='Episode' AND emby_item_ids_json @> $1::jsonb",
    )
    .bind(json!(["101"]))
    .fetch_one(&fixture.state.db)
    .await
    .unwrap();
    assert_eq!(
        admitted, 1,
        "one uniquely bound episode, with its required series/season metadata"
    );
    assert_eq!(fixture.host.items.lock().await["102"], old);
    assert_eq!(
        std::fs::read(
            std::path::Path::new(fixture.files[1].as_str().unwrap())
                .with_file_name("102-mediainfo.json")
        )
        .unwrap(),
        old_json
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_gaia_field_warning_never_clears_related_versions_or_blocks_metadata_ingestion() {
    let (fixture, event, old, old_json, _, _) = gaia_empty_version_fixture().await;
    fixture.host.skip_sync_import.store(true, Ordering::SeqCst);
    let result = fixture.run_moviepilot_event(event).await;
    assert_eq!(result["state"], "completed_with_warnings", "{result}");
    assert!(
        result["result"]["warnings"].as_u64().unwrap_or(0) > 0,
        "{result}"
    );
    assert_eq!(*fixture.host.sync_imports.lock().await, vec!["101"]);
    assert!(fixture.host.file_imports.lock().await.is_empty());
    assert_eq!(fixture.host.items.lock().await["102"], old);
    assert_eq!(
        std::fs::read(
            std::path::Path::new(fixture.files[1].as_str().unwrap())
                .with_file_name("102-mediainfo.json")
        )
        .unwrap(),
        old_json
    );
    assert_eq!(
        media_recovery::status(State(fixture.state.clone()))
            .await
            .unwrap()
            .0["pending"],
        true
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_gaia_existing_markers_refuse_before_new_recovery_directory_or_native_write() {
    let (fixture, event, old, old_json, _, _) = gaia_empty_version_fixture().await;
    {
        let mut native = fixture.host.items.lock().await;
        let new = native.get_mut("101").unwrap();
        let chapters = json!([{"MarkerType":"IntroStart","StartPositionTicks":0}]);
        new["Chapters"] = chapters.clone();
        new["MediaSources"][1]["Chapters"] = chapters;
    }
    let before = std::fs::read(fixture.sidecar_path()).unwrap();
    let result = fixture.run_moviepilot_event(event).await;
    assert_eq!(result["state"], "failed", "{result}");
    assert!(fixture.host.sync_imports.lock().await.is_empty());
    assert!(fixture.host.file_imports.lock().await.is_empty());
    assert_eq!(std::fs::read(fixture.sidecar_path()).unwrap(), before);
    let root = fixture.state.data_dir.join("backups/media-info-wash");
    assert!(
        !root.exists() || std::fs::read_dir(root).unwrap().next().is_none(),
        "refused preparation has no recovery operation"
    );
    assert_eq!(fixture.host.items.lock().await["102"], old);
    assert_eq!(
        std::fs::read(
            std::path::Path::new(fixture.files[1].as_str().unwrap())
                .with_file_name("102-mediainfo.json")
        )
        .unwrap(),
        old_json
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_empty_single_movie_imports_by_id_without_clear_or_file_refresh() {
    let mut fixture = Fixture::new(&["complete"], false, false).await;
    fixture.configure_cloud_wash().await;
    let identity = fixture
        .cloud_version("10001", &"A".repeat(40), "hevc")
        .await;
    let guid = uuid::Uuid::new_v4().simple().to_string();
    let root = fixture._directory.path().join("metadata");
    std::fs::create_dir_all(root.join("library").join(&guid[..2]).join(&guid)).unwrap();
    fixture
        .state
        .save_config(&json!({"emby_metadata_path":root}))
        .await
        .unwrap();
    {
        let mut items = fixture.host.items.lock().await;
        let new = items.get_mut("101").unwrap();
        new["Guid"] = json!(guid);
        new["Size"] = json!(0);
        new["RunTimeTicks"] = Value::Null;
        new["MediaStreams"] = json!([]);
        new["Chapters"] = json!([]);
        let source = &mut new["MediaSources"][0];
        source["Container"] = json!("strm");
        source["Size"] = json!(0);
        source["RunTimeTicks"] = Value::Null;
        source["MediaStreams"] = json!([]);
        source["Chapters"] = json!([]);
    }
    let result = fixture
        .run_moviepilot_event(fixture.cloud_event(identity, "empty-single-movie"))
        .await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["updated"], 1, "{result}");
    assert_eq!(*fixture.host.sync_imports.lock().await, vec!["101"]);
    assert!(fixture.host.file_imports.lock().await.is_empty());
    assert_eq!(fixture.host.items.lock().await["101"]["Size"], 1000);
    assert_eq!(
        fixture.host.items.lock().await["101"]["MediaStreams"][0]["Codec"],
        "hevc"
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_empty_single_version_episode_imports_by_id_without_file_refresh() {
    let (fixture, event, old, _, _, _) = gaia_empty_version_fixture().await;
    {
        let mut items = fixture.host.items.lock().await;
        let own = items["101"]["MediaSources"][1].clone();
        items.get_mut("101").unwrap()["MediaSources"] = json!([own]);
        items.get_mut("102").unwrap()["PresentationUniqueKey"] = json!("unrelated-episode");
    }
    let old_streams = old["MediaStreams"].clone();
    let result = fixture.run_moviepilot_event(event).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["updated"], 3, "{result}");
    assert_eq!(*fixture.host.sync_imports.lock().await, vec!["101"]);
    assert!(fixture.host.file_imports.lock().await.is_empty());
    assert_eq!(
        fixture.host.items.lock().await["102"]["MediaStreams"],
        old_streams
    );
    fixture.close().await;
}

async fn gaia_failed_empty_sync_fixture() -> (Fixture, Value, Value, Vec<u8>) {
    let (fixture, event, old, old_json, _, _) = gaia_empty_version_fixture().await;
    let new_path = fixture.files[0].clone();
    sqlx::query("UPDATE media_metadata SET emby_item_ids_json=$1,asset_details_json=$2,file_sha1_json=$3,file_pickcode_json=$4 WHERE tmdb_id='1011' AND item_type='Episode'")
        .bind(json!(["102","101"]))
        .bind(json!([{"emby_item_id":"102","path":old["Path"],"size_bytes":1000},
            {"emby_item_id":"101","path":new_path,"size_bytes":1000}]))
        .bind(json!(["1".repeat(40),"A".repeat(40)]))
        .bind(json!(["pc102","10001"]))
        .execute(&fixture.state.db).await.unwrap();
    fixture
        .host
        .fail_final_sync_readback
        .store(true, Ordering::SeqCst);
    let failed = fixture.run_moviepilot_event(event.clone()).await;
    assert_eq!(failed["state"], "failed", "{failed}");
    assert_eq!(*fixture.host.sync_imports.lock().await, vec!["101"]);
    let pending: bool = sqlx::query_scalar("SELECT COALESCE((asset->>'moviepilot_media_pending')::boolean,false) FROM media_metadata CROSS JOIN LATERAL jsonb_array_elements(asset_details_json) asset WHERE tmdb_id='1011' AND item_type='Episode' AND asset->>'emby_item_id'='101'")
        .fetch_one(&fixture.state.db).await.unwrap();
    assert!(
        pending,
        "native content alone cannot publish an unverified marker"
    );
    assert_eq!(fixture.host.items.lock().await["101"]["Size"], 1000);
    let exported: Value =
        serde_json::from_slice(&std::fs::read(fixture.sidecar_path()).unwrap()).unwrap();
    for field in ["Id", "ItemId", "Path", "Name"] {
        assert!(
            exported[0]["MediaSourceInfo"][field].is_null(),
            "{exported}"
        );
    }
    let root = fixture.state.data_dir.join("backups/media-info-wash");
    assert!(
        std::fs::read_dir(root).unwrap().any(|entry| entry
            .unwrap()
            .path()
            .join("ingest-sync-export.json")
            .is_file()),
        "the actual export is recorded before final native readback can fail"
    );
    (fixture, event, old, old_json)
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_empty_sync_retry_after_restart_verifies_own_pending_binding_without_reimport() {
    let (mut fixture, event, old, old_json) = gaia_failed_empty_sync_fixture().await;
    fixture.state.jobs.shutdown().await;
    fixture.state.jobs = Arc::new(
        emby_toolkit::jobs::JobManager::open(fixture.state.db.clone())
            .await
            .unwrap(),
    );
    fixture
        .host
        .fail_final_sync_readback
        .store(false, Ordering::SeqCst);
    let mut retry = event;
    retry["batch_key"] = json!("gaia-retry-after-process-restart");
    let result = fixture.run_moviepilot_event(retry).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["updated"], 3, "{result}");
    assert_eq!(
        *fixture.host.sync_imports.lock().await,
        vec!["101"],
        "verified native import is not rerun"
    );
    assert!(fixture.host.file_imports.lock().await.is_empty());
    assert_eq!(fixture.host.items.lock().await["102"], old);
    assert_eq!(
        std::fs::read(
            std::path::Path::new(fixture.files[1].as_str().unwrap())
                .with_file_name("102-mediainfo.json")
        )
        .unwrap(),
        old_json
    );
    let pending: bool = sqlx::query_scalar("SELECT COALESCE((asset->>'moviepilot_media_pending')::boolean,false) FROM media_metadata CROSS JOIN LATERAL jsonb_array_elements(asset_details_json) asset WHERE tmdb_id='1011' AND item_type='Episode' AND asset->>'emby_item_id'='101'")
        .fetch_one(&fixture.state.db).await.unwrap();
    assert!(!pending);
    assert_eq!(
        media_recovery::status(State(fixture.state.clone()))
            .await
            .unwrap()
            .0["pending"],
        false
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_empty_sync_restart_rejects_changed_export_or_unknown_native_fields() {
    for fault in [
        "audio_title",
        "unknown_null",
        "native_attribute",
        "native_stream",
    ] {
        let (mut fixture, mut event, old, old_json) = gaia_failed_empty_sync_fixture().await;
        let path = fixture.sidecar_path();
        let mut info: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        match fault {
            "audio_title" => {
                info[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("用户修改的音轨")
            }
            "unknown_null" => info[0]["MediaSourceInfo"]["unverified_native_tag"] = Value::Null,
            "native_attribute" => {
                let mut items = fixture.host.items.lock().await;
                let source = items.get_mut("101").unwrap()["MediaSources"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|source| source["ItemId"] == "101")
                    .unwrap();
                source["unverified_native_tag"] = json!(true);
            }
            "native_stream" => {
                fixture.host.items.lock().await.get_mut("101").unwrap()["MediaStreams"][0]["unverified_native_tag"] =
                    Value::Null;
            }
            _ => unreachable!(),
        }
        if !matches!(fault, "native_attribute" | "native_stream") {
            std::fs::write(&path, serde_json::to_vec(&info).unwrap()).unwrap();
        }
        let edited = std::fs::read(&path).unwrap();
        fixture.state.jobs.shutdown().await;
        fixture.state.jobs = Arc::new(
            emby_toolkit::jobs::JobManager::open(fixture.state.db.clone())
                .await
                .unwrap(),
        );
        fixture
            .host
            .fail_final_sync_readback
            .store(false, Ordering::SeqCst);
        event["batch_key"] = json!(format!("gaia-restart-refuses-{fault}"));
        let result = fixture.run_moviepilot_event(event).await;
        assert_eq!(result["state"], "failed", "{fault}: {result}");
        assert_eq!(result["result_status"], 409, "{fault}: {result}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            edited,
            "{fault}: keep current JSON"
        );
        let pending: bool = sqlx::query_scalar("SELECT COALESCE((asset->>'moviepilot_media_pending')::boolean,false) FROM media_metadata CROSS JOIN LATERAL jsonb_array_elements(asset_details_json) asset WHERE tmdb_id='1011' AND item_type='Episode' AND asset->>'emby_item_id'='101'")
            .fetch_one(&fixture.state.db).await.unwrap();
        assert!(
            pending,
            "{fault}: failed recovery cannot publish a media marker"
        );
        assert_eq!(
            *fixture.host.sync_imports.lock().await,
            vec!["101"],
            "{fault}"
        );
        assert!(fixture.host.file_imports.lock().await.is_empty());
        assert_eq!(fixture.host.items.lock().await["102"], old, "{fault}");
        assert_eq!(
            std::fs::read(
                std::path::Path::new(fixture.files[1].as_str().unwrap())
                    .with_file_name("102-mediainfo.json")
            )
            .unwrap(),
            old_json,
            "{fault}"
        );
        assert_eq!(
            media_recovery::status(State(fixture.state.clone()))
                .await
                .unwrap()
                .0["pending"],
            true,
            "{fault}"
        );
        fixture.close().await;
    }
}
