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
    assert!(fixture.host.sync_imports.lock().await.is_empty());
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
async fn moviepilot_gaia_skipped_field_import_does_not_add_a_stage_or_warning() {
    let (fixture, event, old, old_json, _, _) = gaia_empty_version_fixture().await;
    fixture.host.skip_sync_import.store(true, Ordering::SeqCst);
    let result = fixture.run_moviepilot_event(event).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert!(
        result["result"]["warnings"].as_u64().unwrap_or(0) == 0,
        "{result}"
    );
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
async fn moviepilot_gaia_existing_markers_do_not_add_an_ingestion_admission_check() {
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
    assert_eq!(result["state"], "completed", "{result}");
    assert!(fixture.host.sync_imports.lock().await.is_empty());
    assert!(fixture.host.file_imports.lock().await.is_empty());
    assert_eq!(std::fs::read(fixture.sidecar_path()).unwrap(), before);
    let root = fixture.state.data_dir.join("backups/media-info-wash");
    assert!(
        !root.exists() || std::fs::read_dir(root).unwrap().next().is_none(),
        "existing markers do not require a field-import recovery operation"
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
    assert!(fixture.host.sync_imports.lock().await.is_empty());
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
    assert!(fixture.host.sync_imports.lock().await.is_empty());
    assert!(fixture.host.file_imports.lock().await.is_empty());
    assert_eq!(
        fixture.host.items.lock().await["102"]["MediaStreams"],
        old_streams
    );
    fixture.close().await;
}
