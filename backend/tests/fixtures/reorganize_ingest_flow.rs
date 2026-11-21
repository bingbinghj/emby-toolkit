async fn reorganize_fixture_metadata(Path(path): Path<String>) -> Json<Value> {
    let mut data = json!({"id":101,"original_language":"en","overview":"重新入库简介",
        "genres":[],"credits":{"cast":[],"crew":[]},"images":{"posters":[]},
        "external_ids":{},"production_companies":[],"keywords":{"results":[]}});
    match path.as_str() {
        "tv/101" => {
            data["name"] = json!("重新入库电视剧");
            data["original_name"] = json!("Reorganize Series");
            data["first_air_date"] = json!("2005-01-01");
            data["seasons"] = json!([{"id":1001,"season_number":1}]);
        }
        "tv/101/season/1" => {
            data["id"] = json!(1001);
            data["name"] = json!("第一季");
            data["season_number"] = json!(1);
            data["episodes"] = json!((1..=2).map(|number| json!({"id":1010+number,"season_number":1,"episode_number":number,
                "name":"重新入库分集","overview":"重新入库分集简介","air_date":"2005-01-01",
                "runtime":2,"credits":{"cast":[],"crew":[]}})).collect::<Vec<_>>());
        }
        _ => data = json!({"results":[],"credits":{"cast":[],"crew":[]}}),
    }
    Json(data)
}

fn metadata_only_mock_controls(item: &Value, fields: &str) -> Value {
    let mut item = item.clone();
    if fields.split(',').any(|field| field == "LockData") && item["LockData"].is_null() {
        item["LockData"] = json!(false);
    }
    if fields.split(',').any(|field| field == "DisplayOrder")
        && item["Type"] == "Series"
        && item["DisplayOrder"].is_null()
    {
        item["DisplayOrder"] = json!("Aired");
    }
    item
}

async fn metadata_only_mock_post(
    State(host): State<Host>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    if *host.resource_fault.lock().await == "metadata-post-fail" {
        return StatusCode::BAD_REQUEST;
    }
    let mut items = host.items.lock().await;
    let item = items.get_mut(&id).unwrap();
    assert_eq!(
        body["Id"], id,
        "the update request must retain the selected native Id"
    );
    assert_eq!(
        body["LockData"],
        item.get("LockData").cloned().unwrap_or(json!(false)),
        "changing Series lock status can affect children"
    );
    if item["Type"] == "Series" {
        assert_eq!(
            body["DisplayOrder"],
            item.get("DisplayOrder").cloned().unwrap_or(json!("Aired")),
            "changing display order can clear child provider IDs"
        );
    }
    for field in [
        "Name",
        "OriginalTitle",
        "Overview",
        "LockData",
        "LockedFields",
        "DisplayOrder",
        "Status",
        "ForcedSortName",
        "CriticRating",
        "CommunityRating",
        "IndexNumber",
        "ParentIndexNumber",
        "SortParentIndexNumber",
        "SortIndexNumber",
        "Tags",
        "TagItems",
        "Genres",
        "GenreItems",
        "Taglines",
        "Studios",
        "People",
        "DateCreated",
        "EndDate",
        "PremiereDate",
        "ProductionYear",
        "OfficialRating",
        "CustomRating",
        "ProductionLocations",
        "PreferredMetadataCountryCode",
        "PreferredMetadataLanguage",
        "RunTimeTicks",
        "ProviderIds",
        "Video3DFormat",
    ] {
        if let Some(value) = body.get(field) {
            item[field] = value.clone();
        }
    }
    // 原生接口仅采用人物姓名、角色和类型，不写媒体流/源/章节。
    if let Some(people) = item["People"].as_array_mut() {
        for person in people {
            person
                .as_object_mut()
                .unwrap()
                .retain(|key, _| ["Name", "Role", "Type"].contains(&key.as_str()));
        }
    }
    if *host.resource_fault.lock().await == "metadata-normalized" {
        item["ForcedSortName"] = json!("原生排序名");
        item["People"] = json!([{"Name":"原生规范化演员","Type":"Actor"}]);
    }
    drop(items);
    host.stage_order
        .lock()
        .await
        .push(format!("metadata-update:{id}"));
    if id == "101" {
        let fault = host.resource_fault.lock().await.clone();
        match fault.as_str() {
            "metadata-associated" => {
                host.items.lock().await.get_mut("102").unwrap()["Chapters"] = json!([])
            }
            "metadata-cloud-swap" => {
                if let Some(next) = host.cloud_swap_after_import.lock().await.take() {
                    *host.cloud_files.lock().await = vec![next];
                }
            }
            "metadata-new-bif" | "metadata-crash-resource" => {
                let old = host.items.lock().await["102"].clone();
                let path = std::path::Path::new(old["Path"].as_str().unwrap());
                let file = path.with_file_name(format!(
                    "{}-640-8.bif",
                    path.file_stem().unwrap().to_string_lossy()
                ));
                std::fs::write(file, bif(&jpeg([0, 0, 255]))).unwrap();
                if fault == "metadata-crash-resource" {
                    host.metadata_hook_reached.notify_one();
                    host.metadata_hook_release.notified().await;
                }
            }
            "metadata-new-chapter" | "metadata-new-chapter-root" => {
                let old = host.chapter_image_paths.lock().await[&("102".into(), 0)].clone();
                let file = old.with_file_name("new-hook.jpg");
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(file, jpeg([0, 0, 255])).unwrap();
            }
            _ => (),
        }
    }
    StatusCode::NO_CONTENT
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; native DTO normalization stays localhost"]
async fn metadata_normalization_does_not_fail_completed_ingestion_or_modify_media_resources() {
    let (fixture, event, old, _, _, _) = gaia_empty_version_fixture().await;
    let original = fixture.host.items.lock().await["101"].clone();
    *fixture.host.resource_fault.lock().await = "metadata-normalized".into();
    let result = fixture.run_moviepilot_event(event).await;
    assert_eq!(result["state"], "completed", "{result}");
    let current = fixture.host.items.lock().await["101"].clone();
    assert_eq!(current["ForcedSortName"], "原生排序名");
    assert_eq!(current["Path"], original["Path"]);
    assert_eq!(current["Size"], 1000);
    assert_eq!(current["MediaStreams"][0]["Codec"], "hevc");
    assert_eq!(fixture.host.items.lock().await["102"], old);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; metadata hook and all native requests stay localhost"]
async fn metadata_only_update_detects_an_associated_version_changed_by_a_post_hook() {
    let (fixture, event, old, _, _, _) = gaia_empty_version_fixture().await;
    fixture
        .host
        .resource_fault
        .lock()
        .await
        .clone_from(&"metadata-associated".to_string());
    let result = fixture.run_moviepilot_event(event).await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_ne!(
        fixture.host.items.lock().await["102"]["Chapters"],
        old["Chapters"],
        "must reach the real metadata POST before injecting its side effect"
    );
    assert!(
        result["result"]["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|error| error
                .as_str()
                .is_some_and(|error| error.contains("关联版本"))),
        "{result}"
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; obsolete local directories and native metadata stay localhost"]
async fn metadata_only_ingest_keeps_obsolete_native_version_when_its_local_directory_is_missing() {
    let (fixture, event, old, _, jpeg_bytes, _) = gaia_empty_version_fixture().await;
    let old_path = std::path::PathBuf::from(old["Path"].as_str().unwrap());
    let directory = old_path.parent().unwrap();
    assert_ne!(
        directory,
        std::path::Path::new(fixture.files[0].as_str().unwrap())
            .parent()
            .unwrap()
    );
    std::fs::remove_dir_all(directory).unwrap();
    assert!(!directory.exists());
    assert_eq!(fixture.host.items.lock().await["102"], old);
    let chapter = fixture.host.chapter_image_paths.lock().await[&("102".into(), 0)].clone();
    assert_eq!(std::fs::read(&chapter).unwrap(), jpeg_bytes);
    let result = fixture.run_moviepilot_event(event).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert!(fixture.host.sync_imports.lock().await.is_empty());
    assert_eq!(fixture.host.items.lock().await["101"]["Size"], 1000);
    assert_eq!(
        fixture.host.items.lock().await["101"]["MediaStreams"][0]["Codec"],
        "hevc"
    );
    assert_eq!(fixture.host.items.lock().await["102"], old);
    assert!(
        !directory.exists(),
        "no resource directory is recreated for the obsolete version"
    );
    assert_eq!(
        std::fs::read(chapter).unwrap(),
        jpeg_bytes,
        "remaining original chapter bytes are preserved"
    );
    let stages = fixture.host.stage_order.lock().await.clone();
    assert!(
        stages.iter().any(|stage| stage == "metadata-update:101"),
        "must actually reach targeted metadata update: {result}"
    );
    assert!(!stages.iter().any(|stage| stage == "metadata-update:102"));
    assert!(
        !fixture
            .host
            .refresh_queries
            .lock()
            .await
            .iter()
            .any(|(id, _)| id == "102")
    );
    assert!(fixture.host.file_imports.lock().await.is_empty());
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; generated BIF and chapter files and native metadata stay localhost"]
async fn metadata_only_ingest_rejects_new_associated_resource_files_even_when_native_and_original_bytes_are_unchanged()
 {
    for hook in [
        "metadata-new-bif",
        "metadata-new-chapter",
        "metadata-new-chapter-root",
    ] {
        let (fixture, event, old, old_json, jpeg_bytes, bif_bytes) =
            gaia_empty_version_fixture().await;
        let old_path = std::path::PathBuf::from(old["Path"].as_str().unwrap());
        let original_chapter =
            fixture.host.chapter_image_paths.lock().await[&("102".into(), 0)].clone();
        let original_bif = fixture.host.wash_bif_paths.lock().await["102"].clone();
        let created = if hook == "metadata-new-bif" {
            old_path.with_file_name(format!(
                "{}-640-8.bif",
                old_path.file_stem().unwrap().to_string_lossy()
            ))
        } else {
            original_chapter.with_file_name("new-hook.jpg")
        };
        if hook == "metadata-new-chapter-root" {
            std::fs::remove_dir_all(original_chapter.parent().unwrap()).unwrap();
        }
        assert!(!created.exists());
        *fixture.host.resource_fault.lock().await = hook.into();
        let result = fixture.run_moviepilot_event(event).await;
        assert!(
            created.exists(),
            "must reach the actual metadata POST file-producing hook: {hook}: {result}"
        );
        assert_eq!(
            fixture.host.items.lock().await["102"],
            old,
            "hook only creates a resource, not a native DTO change"
        );
        assert_eq!(
            std::fs::read(old_path.with_file_name("102-mediainfo.json")).unwrap(),
            old_json
        );
        assert_eq!(std::fs::read(original_bif).unwrap(), bif_bytes);
        if hook != "metadata-new-chapter-root" {
            assert_eq!(std::fs::read(original_chapter).unwrap(), jpeg_bytes);
        }
        assert_eq!(result["state"], "failed", "{hook}: {result}");
        assert!(
            result["result"]["failures"]
                .as_array()
                .unwrap()
                .iter()
                .any(|error| error
                    .as_str()
                    .is_some_and(|error| error.contains("关联版本") && error.contains("资源清单"))),
            "{hook}: {result}"
        );
        fixture.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; same-job metadata POST barrier and restart stay localhost"]
async fn metadata_only_restart_keeps_original_related_resource_snapshot_after_a_post_hook_creates_a_file()
 {
    let (mut fixture, mut event, old, old_json, _, old_bif) = gaia_empty_version_fixture().await;
    event["source"] = json!("moviepilot");
    let old_path = std::path::Path::new(old["Path"].as_str().unwrap());
    let created = old_path.with_file_name(format!(
        "{}-640-8.bif",
        old_path.file_stem().unwrap().to_string_lossy()
    ));
    assert!(!created.exists());
    *fixture.host.resource_fault.lock().await = "metadata-crash-resource".into();
    let spec = emby_toolkit::jobs::JobSpec::new("webhook", json!({"event":event}));
    let work = fixture.state.clone();
    let task = fixture
        .state
        .jobs
        .submit_persistent("元数据原资源断点验证", spec, move |context| async move {
            webhook::process_moviepilot(&work, &event, &context).await
        })
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(60),
        fixture.host.metadata_hook_reached.notified(),
    )
    .await
    .expect("must reach the actual single-ID metadata POST and file-producing hook");
    assert!(created.exists());
    assert_eq!(fixture.host.items.lock().await["102"], old);
    let original:Value=sqlx::query_scalar("SELECT value_json FROM durable_job_steps WHERE job_id=$1 AND step_key='plans:related-resource-origin:301'")
        .bind(&task).fetch_one(&fixture.state.db).await.unwrap();
    assert_eq!(original["scope"]["selected_ids"], json!(["101"]));
    assert!(
        !original["files"]
            .as_object()
            .unwrap()
            .contains_key(created.to_str().unwrap()),
        "guard must precede the new resource"
    );
    assert!(
        original["directories"]
            .as_array()
            .unwrap()
            .iter()
            .all(|directory| !directory["members"]
                .as_array()
                .is_some_and(|members| members
                    .iter()
                    .any(|file| file == created.to_str().unwrap())))
    );
    let manager = fixture.state.jobs.clone();
    let stopping = tokio::spawn(async move {
        manager.shutdown().await;
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if fixture.state.jobs.status().await["state"] == "interrupted" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the old manager must record interruption before the response barrier is released");
    *fixture.host.resource_fault.lock().await = String::new();
    fixture.host.metadata_hook_release.notify_one();
    tokio::time::timeout(Duration::from_secs(60), stopping)
        .await
        .unwrap()
        .unwrap();
    let phase_done:i64=sqlx::query_scalar("SELECT COUNT(*) FROM durable_job_steps WHERE job_id=$1 AND (step_key LIKE 'completed:moviepilot_finalized:%' OR step_key='completed:__job_result__')")
        .bind(&task).fetch_one(&fixture.state.db).await.unwrap();
    assert_eq!(
        phase_done, 0,
        "the crash precedes final guard verification and job settlement"
    );
    let mut restarted = AppState::new(fixture.state.db.clone(), fixture.state.data_dir.clone())
        .await
        .unwrap();
    restarted.p115 = fixture.state.p115.clone();
    fixture.state = restarted.clone();
    tokio::time::timeout(
        Duration::from_secs(60),
        restarted.jobs.resume_pending(
            restarted.clone(),
            tokio_util::sync::CancellationToken::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let result = restarted.jobs.result(&task).await.unwrap();
    println!(
        "same-job resume diagnostic: {}",
        json!({"task_id":result["task_id"],"state":result["state"],
        "phase":result["phase"],"result_status":result["result_status"],"error":result["result"]["error"],
        "failures":result["result"]["failures"],"message":result["message"],"resume_error":result["resume_error"]})
    );
    assert_eq!(result["state"], "failed", "{result}");
    let causes = result["result"]["error"]
        .as_str()
        .into_iter()
        .chain(
            result["result"]["failures"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        )
        .collect::<Vec<_>>();
    assert!(
        causes.iter().any(|error| error.contains("资源清单")),
        "resume must reject the original related-resource snapshot: {result}"
    );
    let after:Value=sqlx::query_scalar("SELECT value_json FROM durable_job_steps WHERE job_id=$1 AND step_key='plans:related-resource-origin:301'")
        .bind(&task).fetch_one(&fixture.state.db).await.unwrap();
    assert_eq!(
        after, original,
        "restart must not accept the new file as a replacement baseline"
    );
    assert_eq!(
        fixture
            .host
            .stage_order
            .lock()
            .await
            .iter()
            .filter(|stage| stage.as_str() == "metadata-update:101")
            .count(),
        1,
        "the completed metadata POST must not be repeated"
    );
    assert_eq!(fixture.host.items.lock().await["102"], old);
    assert_eq!(
        std::fs::read(old_path.with_file_name("102-mediainfo.json")).unwrap(),
        old_json
    );
    assert_eq!(
        std::fs::read(fixture.host.wash_bif_paths.lock().await["102"].clone()).unwrap(),
        old_bif
    );
    assert!(
        created.exists(),
        "the unverified resource is retained, not deleted or silently accepted"
    );
    let notices: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_outbox")
        .fetch_one(&fixture.state.db)
        .await
        .unwrap();
    assert_eq!(
        notices, 0,
        "no successful admission notification may be registered"
    );
    fixture.close().await;
}

async fn current_reorganize_fixture() -> Fixture {
    let mut fixture = Fixture::new(&["complete"], false, false).await;
    fixture.configure_cloud_wash().await;
    fixture
        .cloud_version("10001", &"A".repeat(40), "hevc")
        .await;
    sqlx::query("UPDATE p115_mediainfo_cache SET raw_ffprobe_json=jsonb_set(raw_ffprobe_json,'{format,duration}','\"120\"') WHERE sha1=$1")
        .bind("A".repeat(40)).execute(&fixture.state.db).await.unwrap();
    configure_ingest_metadata(&fixture, "101").await;
    fixture
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; reproduces Emby's root-only inventory on localhost"]
async fn metadata_only_ingest_finds_selected_episode_in_recursive_library_inventory() {
    let (fixture, event, old, _, _, _) = gaia_empty_version_fixture().await;
    let result = fixture.run_moviepilot_event(event).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(fixture.host.items.lock().await["102"], old);
    assert!(fixture.host.stage_order.lock().await.iter().any(|stage| stage == "metadata-update:101"));
    fixture.close().await;
}

async fn e200_db_marker_fixture() -> (Fixture, tokio::task::JoinHandle<()>, Value) {
    let fixture = current_reorganize_fixture().await;
    let metadata = prepare_reorganize_chain(&fixture, true).await;
    fixture.state.save_config(&json!({
        "auto_repair_truehd_core_on_ingest":false,
        "auto_extract_intro_fingerprint_on_ingest":false,
        "auto_extract_credits_on_ingest":false,
        "auto_extract_video_thumbnails_on_ingest":false
    })).await.unwrap();
    let chapters = json!([
        {"Name":"片头","MarkerType":"Chapter","StartPositionTicks":0},
        {"Name":"正片","MarkerType":"Chapter","StartPositionTicks":1240000000_u64},
        {"Name":"片尾","MarkerType":"Chapter","StartPositionTicks":10660000000_u64}
    ]);
    {
        let mut items = fixture.host.items.lock().await;
        let episode = items.get_mut("101").unwrap();
        episode["RunTimeTicks"] = json!(11560100000_u64);
        episode["MediaSources"][0]["RunTimeTicks"] = json!(11560100000_u64);
        episode["Chapters"] = chapters.clone();
        episode["MediaSources"][0]["Chapters"] = chapters.clone();
        let mut info: Value = serde_json::from_slice(&std::fs::read(fixture.sidecar_path()).unwrap()).unwrap();
        info[0]["Chapters"] = chapters.clone();
        info[0]["MediaSourceInfo"]["RunTimeTicks"] = json!(11560100000_u64);
        std::fs::write(fixture.sidecar_path(), serde_json::to_vec(&info).unwrap()).unwrap();
    }
    *fixture.host.resource_fault.lock().await = "native-db-markers".into();
    (fixture, metadata, chapters)
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; reproduces E200 database markers during native refresh on localhost"]
async fn reorganize_e200_native_database_markers_do_not_mean_original_chapter_loss() {
    let (fixture, metadata, chapters) = e200_db_marker_fixture().await;
    let row = public_reorganize_queue(&fixture).await;
    assert_eq!(row["status"], "completed", "{row}");
    assert_eq!(row["result"]["finalization"]["warnings"], 0, "{row}");
    let live = fixture.host.items.lock().await["101"].clone();
    assert!(emby_toolkit::media_info::chapters_restored(&chapters, &live["Chapters"]));
    assert!(live["Chapters"].as_array().unwrap().iter().any(|chapter| chapter["Name"] == "CreditsStart#DB"));
    assert_eq!(live["MediaSources"][0]["Container"], "mkv", "STRM remains the entry path, not the media container");
    let recovery = media_recovery::status(State(fixture.state.clone())).await.unwrap().0;
    assert_eq!(recovery["pending"], false, "{recovery}");
    metadata.abort();
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; native E200 refresh interruption and recovery stay localhost"]
async fn reorganize_e200_restart_keeps_database_markers_and_releases_previous_settings_guard() {
    let (mut fixture, metadata, chapters) = e200_db_marker_fixture().await;
    *fixture.host.resource_fault.lock().await = "native-db-markers-crash".into();
    let original_settings = fixture.host.configuration.lock().await.clone();
    let accepted = submit_reorganize(&fixture).await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let worker = tokio::spawn(emby_toolkit::media_organize_queue::run(
        fixture.state.clone(),
        cancel.clone(),
    ));
    tokio::time::timeout(
        Duration::from_secs(60),
        fixture.host.metadata_hook_reached.notified(),
    )
    .await
    .unwrap();
    let task = fixture.state.jobs.status().await["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    let saved: Value = sqlx::query_scalar("SELECT value_json FROM durable_job_steps WHERE job_id=$1 AND step_key LIKE 'plans:%:rescrape-recovery'")
        .bind(&task).fetch_one(&fixture.state.db).await.unwrap();
    let guard_path = std::path::PathBuf::from(saved.as_str().unwrap());
    assert_eq!(
        guard_path.parent(),
        Some(
            fixture
                .state
                .data_dir
                .join("backups/media-info-reload")
                .as_path()
        )
    );
    let legacy_copy = fixture._directory.path().join("old-version-guard");
    assert!(
        std::process::Command::new("cp")
            .arg("-a")
            .arg(&guard_path)
            .arg(&legacy_copy)
            .status()
            .unwrap()
            .success()
    );
    let manager = fixture.state.jobs.clone();
    let stopping = tokio::spawn(async move {
        manager.shutdown().await;
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while fixture.state.jobs.result(&task).await.unwrap()["state"] != "interrupted" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    *fixture.host.resource_fault.lock().await = String::new();
    fixture.host.metadata_hook_release.notify_one();
    tokio::time::timeout(Duration::from_secs(60), stopping)
        .await
        .unwrap()
        .unwrap();
    cancel.cancel();
    worker.await.unwrap();
    // The fixed live guard can finish during shutdown. Restore the exact
    // earlier guard snapshot to model a record left by the old strict check.
    if guard_path.exists() {
        std::fs::remove_dir_all(&guard_path).unwrap();
    }
    assert!(
        std::process::Command::new("cp")
            .arg("-a")
            .arg(&legacy_copy)
            .arg(&guard_path)
            .status()
            .unwrap()
            .success()
    );
    let before = fixture.host.items.lock().await["101"].clone();
    assert!(emby_toolkit::media_info::chapters_restored(
        &chapters,
        &before["Chapters"]
    ));
    assert_eq!(before["Chapters"].as_array().unwrap().len(), 6);
    let pending = media_recovery::status(State(fixture.state.clone()))
        .await
        .unwrap()
        .0;
    assert_eq!(
        pending["pending"], true,
        "interrupted refresh must retain its original recovery evidence: {pending}"
    );
    let info_runs = *fixture.host.info_runs.lock().await;
    let mut restarted = AppState::new(fixture.state.db.clone(), fixture.state.data_dir.clone())
        .await
        .unwrap();
    restarted.p115 = fixture.state.p115.clone();
    fixture.state = restarted.clone();
    tokio::time::timeout(
        Duration::from_secs(120),
        restarted.jobs.resume_pending(
            restarted.clone(),
            tokio_util::sync::CancellationToken::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let result = restarted.jobs.result(&task).await.unwrap();
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(
        *fixture.host.info_runs.lock().await,
        info_runs,
        "restart must reuse completed media extraction"
    );
    let row: Value =
        sqlx::query_scalar("SELECT to_jsonb(q) FROM media_organize_queue q WHERE id=$1")
            .bind(accepted["queue_ids"][0].as_str().unwrap())
            .fetch_one(&fixture.state.db)
            .await
            .unwrap();
    assert_eq!(row["status"], "completed", "{row}");
    assert_eq!(
        fixture.host.items.lock().await["101"]["Chapters"],
        before["Chapters"]
    );
    assert_eq!(*fixture.host.configuration.lock().await, original_settings);
    assert_eq!(
        media_recovery::status(State(fixture.state.clone()))
            .await
            .unwrap()
            .0["pending"],
        false
    );
    metadata.abort();
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; all media and metadata APIs stay localhost"]
async fn moviepilot_remote_media_without_etag_enters_native_ingestion_without_a_field_stage() {
    let fixture = current_reorganize_fixture().await;
    let metadata = prepare_reorganize_chain(&fixture, false).await;
    fixture.state.save_config(&json!({"monitor_sha1_pc_search":false,
        "auto_extract_intro_fingerprint_on_ingest":false,"auto_extract_credits_on_ingest":false,
        "auto_extract_video_thumbnails_on_ingest":false})).await.unwrap();
    // An ordinary HTTP source is not required to support HEAD or ETag.
    let source = "http://127.0.0.1:9/native-fixture.mkv";
    std::fs::write(fixture.files[0].as_str().unwrap(), source).unwrap();
    fixture.host.items.lock().await.get_mut("101").unwrap()["MediaSources"][0]["Path"] = json!(source);
    let before = *fixture.host.info_runs.lock().await;
    let event = json!({"source":"moviepilot","media_type":"series","tmdb_id":"101",
        "files":[fixture.files[0]],"batch_key":"remote-no-etag"});
    let result = fixture.run_moviepilot_event(event).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(*fixture.host.info_runs.lock().await, before + 1);
    assert!(fixture.host.sync_imports.lock().await.is_empty());
    assert!(fixture.host.file_imports.lock().await.is_empty());
    assert!(!result["result"]["warning_details"].to_string().contains("字段补充"), "{result}");
    metadata.abort();
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; missing native technical fields stay localhost"]
async fn reorganize_missing_native_technical_fields_does_not_add_a_field_stage_or_reminders() {
    let fixture = current_reorganize_fixture().await;
    let metadata = prepare_reorganize_chain(&fixture, false).await;
    {
        let mut items = fixture.host.items.lock().await;
        let item = items.get_mut("101").unwrap();
        for stream in item["MediaStreams"].as_array_mut().unwrap() {
            for field in [
                "Title",
                "RefFrames",
                "AspectRatio",
                "ColorSpace",
                "ColorTransfer",
                "ColorPrimaries",
                "VideoRange",
            ] {
                stream.as_object_mut().unwrap().remove(field);
            }
        }
        item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
    }
    let before_json = std::fs::read(fixture.sidecar_path()).unwrap();
    let row = public_reorganize_queue(&fixture).await;
    assert_eq!(row["status"], "completed", "{row}");
    assert_eq!(row["result"]["finalization"]["metadata_refresh"]["completed"], true, "{row}");
    assert!(!row["result"]["warning_details"].to_string().contains("字段补充"), "{row}");
    assert!(fixture.host.sync_imports.lock().await.is_empty());
    assert!(fixture.host.file_imports.lock().await.is_empty());
    assert_eq!(std::fs::read(fixture.sidecar_path()).unwrap(), before_json);
    metadata.abort();
    fixture.close().await;
}

async fn configure_ingest_metadata(fixture: &Fixture, id: &str) {
    let guid = uuid::Uuid::new_v4().simple().to_string();
    let root = fixture._directory.path().join("metadata");
    std::fs::create_dir_all(root.join("library").join(&guid[..2]).join(&guid)).unwrap();
    fixture.host.items.lock().await.get_mut(id).unwrap()["Guid"] = json!(guid);
    fixture
        .state
        .save_config(&json!({"emby_metadata_path":root}))
        .await
        .unwrap();
}

async fn emby_untitled_admission(event_name: &str, pending: bool) {
    let fixture = current_reorganize_fixture().await;
    let metadata = prepare_reorganize_chain(&fixture, false).await;
    fixture
        .state
        .save_config(&json!({"p115_generate_mediainfo":true,
        "auto_fill_language_titles_on_ingest":true,"auto_extract_intro_fingerprint_on_ingest":false,
        "auto_extract_credits_on_ingest":false,"auto_extract_video_thumbnails_on_ingest":false,
        "monitor_sha1_pc_search":false}))
        .await
        .unwrap();
    let mut info: Value =
        serde_json::from_slice(&std::fs::read(fixture.sidecar_path()).unwrap()).unwrap();
    for stream in info[0]["MediaSourceInfo"]["MediaStreams"]
        .as_array_mut()
        .unwrap()
    {
        if matches!(stream["Type"].as_str(), Some("Video" | "Audio")) {
            stream["Title"] = json!("");
        }
    }
    {
        let mut items = fixture.host.items.lock().await;
        import_fixture_info(items.get_mut("101").unwrap(), &info);
    }
    std::fs::write(fixture.sidecar_path(), serde_json::to_vec(&info).unwrap()).unwrap();
    sqlx::query("UPDATE p115_mediainfo_cache SET mediainfo_json=$1 WHERE sha1=$2")
        .bind(&info)
        .bind("A".repeat(40))
        .execute(&fixture.state.db)
        .await
        .unwrap();
    sqlx::query("UPDATE media_metadata SET asset_details_json=jsonb_set(asset_details_json,'{0,moviepilot_media_pending}',$1::jsonb) WHERE tmdb_id='1011' AND item_type='Episode'")
        .bind(json!(pending)).execute(&fixture.state.db).await.unwrap();
    let event = json!({"id":"301","episode_ids":["101"],"event":event_name,
        "source":"emby","original":{"Id":"101","Type":"Episode"}});
    let work = fixture.state.clone();
    let task = fixture
        .state
        .jobs
        .submit("Emby 空标题入库回归", move |context| async move {
            Box::pin(webhook::process_emby(&work, &event, &context)).await
        })
        .await
        .unwrap();
    let result = fixture.wait_for_job(&task).await;
    let current = fixture.host.items.lock().await["101"].clone();
    let still_pending: bool = sqlx::query_scalar("SELECT COALESCE((asset_details_json->0->>'moviepilot_media_pending')::boolean,false) FROM media_metadata WHERE tmdb_id='1011' AND item_type='Episode'")
        .fetch_one(&fixture.state.db).await.unwrap();
    metadata.abort();
    fixture.close().await;
    assert_eq!(result["state"], "completed", "{event_name}: {result}");
    assert_eq!(
        result["result"]["finalization"]["metadata_refresh"]["completed"], true,
        "{result}"
    );
    assert_eq!(
        result["result"]["finalization"]["title_fill"]["completed"], true,
        "{result}"
    );
    assert!(
        current["MediaStreams"]
            .as_array()
            .unwrap()
            .iter()
            .any(|stream| {
                stream["Type"] == "Audio" && !stream["Title"].as_str().unwrap_or("").is_empty()
            }),
        "title filling follows admission: {current}"
    );
    assert!(
        !still_pending,
        "an Emby retry must finish the pending media stage"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; actual Emby item.add and title filling stay localhost"]
async fn emby_item_add_accepts_untitled_tracks_and_runs_title_filling() {
    emby_untitled_admission("item.add", false).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; actual Emby library.new resumes a pending local media stage"]
async fn emby_library_new_accepts_untitled_tracks_and_retries_pending_ingestion() {
    emby_untitled_admission("library.new", true).await;
}

async fn prepare_reorganize_chain(fixture: &Fixture, titles: bool) -> tokio::task::JoinHandle<()> {
    fixture.configure_series_chain().await;
    fixture
        .state
        .save_config(&json!({"auto_extract_intro_fingerprint_on_ingest":false,
        "auto_extract_credits_on_ingest":false,"auto_extract_video_thumbnails_on_ingest":false}))
        .await
        .unwrap();
    let initial = fixture.run_series(true).await;
    assert_eq!(
        initial["state"], "completed",
        "prepare current media through the real ingestion entry: {initial}"
    );
    // Native extraction leaves these already imported files unchanged.
    // Do not replay the fixture's initial sample over E200 or edited fields.
    fixture.host.native_generated_info.lock().await.clear();
    // The organizer receives previously ingested content with a valid marker.
    let mut info: Value = serde_json::from_slice(&std::fs::read(fixture.sidecar_path()).unwrap()).unwrap();
    let marker = json!({"Name":"片尾","MarkerType":"CreditsStart","StartPositionTicks":750_000_000_u64});
    if !info[0]["Chapters"].as_array().unwrap().iter().any(|chapter| chapter["MarkerType"]=="CreditsStart") {
        info[0]["Chapters"].as_array_mut().unwrap().push(marker.clone());
        fixture.host.items.lock().await.get_mut("101").unwrap()["Chapters"].as_array_mut().unwrap().push(marker);
        std::fs::write(fixture.sidecar_path(), serde_json::to_vec(&info).unwrap()).unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/tmdb", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/tmdb/{*path}", get(reorganize_fixture_metadata)),
        )
        .await
        .unwrap();
    });
    fixture
        .state
        .save_config(&json!({"tmdb_api_base_url":base,"tmdb_api_key":"fixture",
        "auto_fill_language_titles_on_ingest":titles,"monitor_sha1_pc_search":true,
        "auto_repair_truehd_core_on_ingest":true,"auto_extract_intro_fingerprint_on_ingest":true,
        "auto_extract_credits_on_ingest":true,"auto_extract_video_thumbnails_on_ingest":true,
        "delay_between_items_sec":0}))
        .await
        .unwrap();
    fixture.host.stage_order.lock().await.clear();
    fixture.host.native_starts.lock().await.clear();
    server
}

async fn submit_reorganize(fixture: &Fixture) -> Value {
    let session = fixture.state.sessions.sign(&emby_toolkit::auth::Session {
        user_id: "operator".into(),
        name: "Test Operator".into(),
        setup_mode: false,
        expires: chrono::Utc::now().timestamp() + 600,
    });
    let response = emby_toolkit::actions::router()
        .with_state(fixture.state.clone())
        .oneshot(
            Request::post("/api/actions/reorganize")
                .header("content-type", "application/json")
                .header("cookie", format!("etk_session={session}"))
                .body(Body::from(
                    json!({"item_ids":["101"],"mode":"reprocess"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let accepted: Value = serde_json::from_slice(&body).unwrap();
    accepted
}

async fn public_reorganize_queue(fixture: &Fixture) -> Value {
    let accepted = submit_reorganize(fixture).await;
    let id = accepted["queue_ids"][0].as_str().unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let worker = tokio::spawn(emby_toolkit::media_organize_queue::run(
        fixture.state.clone(),
        cancel.clone(),
    ));
    let row = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let row: Value =
                sqlx::query_scalar("SELECT to_jsonb(q) FROM media_organize_queue q WHERE id=$1")
                    .bind(id)
                    .fetch_one(&fixture.state.db)
                    .await
                    .unwrap();
            if !matches!(row["status"].as_str(), Some("queued" | "running")) {
                break row;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    worker.await.unwrap();
    row
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; all ingestion, metadata and native tasks stay localhost"]
async fn reorganize_public_queue_runs_complete_ingestion_and_configured_native_stages() {
    let fixture = current_reorganize_fixture().await;
    let metadata = prepare_reorganize_chain(&fixture, false).await;
    let row = public_reorganize_queue(&fixture).await;
    assert_eq!(row["status"], "completed", "{row}");
    let report = &row["result"];
    assert_eq!(
        report["finalization"]["metadata_refresh"]["completed"], true,
        "{row}"
    );
    assert_eq!(
        report["finalization"]["title_fill"]["skipped"], true,
        "{row}"
    );
    assert_eq!(
        report["finalization"]["audio_repair"]["scanned"], 1,
        "configured audio stage must actually scan the selected file: {row}"
    );
    assert!(report["finalization"]["fingerprints"].is_object(), "{row}");
    assert_eq!(
        *fixture.host.native_starts.lock().await,
        vec!["intro", "thumb"],
        "{row}"
    );
    let stages = fixture.host.stage_order.lock().await.clone();
    let position = |stage: &str| stages.iter().position(|value| value == stage).unwrap();
    assert!(
        position("metadata-refresh") < position("intro"),
        "{stages:?}"
    );
    assert!(position("intro") < position("credits-read"), "{stages:?}");
    assert!(position("credits-read") < position("thumb"), "{stages:?}");
    assert_eq!(
        report["automatic_extraction"]["intro"]["failed"], 0,
        "{row}"
    );
    assert_eq!(
        report["automatic_extraction"]["credits"]["failed"], 0,
        "{row}"
    );
    assert!(
        report["automatic_extraction"]["warnings"].as_u64().unwrap() > 0,
        "the deliberate native thumbnail refusal must remain a warning: {row}"
    );
    let marked: Value = sqlx::query_scalar("SELECT asset_details_json->0->'moviepilot_ingest_identity' FROM media_metadata WHERE tmdb_id='1011' AND item_type='Episode'")
        .fetch_one(&fixture.state.db).await.unwrap();
    assert_eq!(
        marked["id"], "101",
        "completion follows the original native ingestion stages: {row}"
    );
    let info: Value =
        serde_json::from_slice(&std::fs::read(fixture.sidecar_path()).unwrap()).unwrap();
    assert!(
        info[0]["Chapters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|chapter| chapter["MarkerType"] == "CreditsStart"),
        "final sidecar must preserve verified credits markers"
    );
    metadata.abort();
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; a fresh organizer attempt reruns native extraction and the complete ingestion workflow"]
async fn reorganize_previously_completed_media_reruns_media_import_and_configured_ingestion() {
    let fixture = current_reorganize_fixture().await;
    let metadata = prepare_reorganize_chain(&fixture, false).await;
    let before = *fixture.host.info_runs.lock().await;
    let result = public_reorganize_queue(&fixture).await;
    let after = *fixture.host.info_runs.lock().await;
    let starts = fixture.host.native_starts.lock().await.clone();
    metadata.abort();
    fixture.close().await;
    assert_eq!(result["status"], "completed", "{result}");
    assert_eq!(after, before + 1,
        "a new organizer request reruns the original native media extraction: {result}");
    assert_eq!(starts, vec!["intro", "thumb"]);
    assert_eq!(
        result["result"]["finalization"]["metadata_refresh"]["completed"],
        true
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; all ingestion and native requests stay localhost"]
async fn reorganize_public_queue_does_not_add_field_reminders_to_completed_ingestion()
 {
    let fixture = current_reorganize_fixture().await;
    let metadata = prepare_reorganize_chain(&fixture, false).await;
    sqlx::query("UPDATE media_metadata SET asset_details_json=jsonb_set(asset_details_json,'{0,moviepilot_ingest_identity}','null') WHERE tmdb_id='1011' AND item_type='Episode'")
        .execute(&fixture.state.db).await.unwrap();
    fixture
        .host
        .corrupt_size_on_native_start
        .store(true, Ordering::SeqCst);
    let row = public_reorganize_queue(&fixture).await;
    assert_eq!(row["status"], "completed", "{row}");
    assert!(!row["result"]["warning_details"].to_string().contains("字段补充"), "{row}");
    assert!(
        fixture
            .host
            .native_starts
            .lock()
            .await
            .iter()
            .any(|stage| stage == "intro"),
        "must reach the actual native entry before deliberate corruption: {row}"
    );
    let marked: Value = sqlx::query_scalar("SELECT asset_details_json->0->'moviepilot_ingest_identity' FROM media_metadata WHERE tmdb_id='1011' AND item_type='Episode'")
        .fetch_one(&fixture.state.db).await.unwrap();
    assert_eq!(marked["id"], "101", "{marked}");
    metadata.abort();
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; all ingestion, metadata and native requests stay localhost"]
async fn reorganize_public_queue_does_not_extract_when_actual_final_refresh_fails() {
    let fixture = current_reorganize_fixture().await;
    let metadata = prepare_reorganize_chain(&fixture, false).await;
    fixture
        .host
        .refresh_failures
        .lock()
        .await
        .push("101".into());
    let row = public_reorganize_queue(&fixture).await;
    assert_eq!(row["status"], "failed", "{row}");
    assert!(
        fixture
            .host
            .refresh_attempts
            .lock()
            .await
            .iter()
            .any(|(id, mode)| id == "101" && mode == "FullRefresh"),
        "{row}"
    );
    assert!(fixture.host.native_starts.lock().await.is_empty(), "{row}");
    metadata.abort();
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; unfinished reorder/recovery only checks existing localhost media"]
async fn reorganize_restart_reuses_verified_stages_and_cleans_uuid_recovery_without_repeating_native_work()
 {
    let fixture = current_reorganize_fixture().await;
    let metadata = prepare_reorganize_chain(&fixture, false).await;
    let row = public_reorganize_queue(&fixture).await;
    assert_eq!(row["status"], "completed", "{row}");
    // 队列先登记业务结果；停止原 manager，确保它不在模拟中断后继续登记终态。
    fixture.state.jobs.shutdown().await;
    let task = row["task_id"].as_str().unwrap();
    let original_native = fixture.host.items.lock().await["101"].clone();
    let original_sidecar = std::fs::read(fixture.sidecar_path()).unwrap();
    let original_starts = fixture.host.native_starts.lock().await.clone();
    let original_refreshes = fixture.host.refresh_queries.lock().await.clone();
    let original_imports = fixture.host.file_imports.lock().await.clone();
    let notices: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_outbox")
        .fetch_one(&fixture.state.db)
        .await
        .unwrap();
    let paths: Vec<Value> = sqlx::query_scalar("SELECT value_json FROM durable_job_steps WHERE job_id=$1 AND step_key LIKE 'plans:%:rescrape-recovery'")
        .bind(task).fetch_all(&fixture.state.db).await.unwrap();
    assert!(
        !paths.is_empty(),
        "the native guard must retain a recoverable UUID plan"
    );
    for path in &paths {
        let directory = std::path::Path::new(path.as_str().unwrap());
        assert!(
            uuid::Uuid::parse_str(directory.file_name().unwrap().to_str().unwrap()).is_ok(),
            "{path}"
        );
        assert!(
            !directory.exists(),
            "a successful guard must already have cleaned its resources: {path}"
        );
    }
    // A failed read-only capture on older releases leaves settings and partial
    // snapshots, but never publishes media-reload.json. This must not block
    // startup recovery or the next unrelated (e.g. music) job.
    let settings_only = std::path::Path::new(paths[0].as_str().unwrap());
    std::fs::create_dir_all(settings_only.join(format!("ingest-refresh-{}", uuid::Uuid::new_v4().simple()))).unwrap();
    let settings = fixture.host.configuration.lock().await.clone();
    std::fs::write(settings_only.join("original-settings.json"), serde_json::to_vec(&json!({"ExclusiveControlFeatures":settings["ExclusiveControlFeatures"]})).unwrap()).unwrap();
    let applied_features = settings["ExclusiveControlFeatures"].as_str().unwrap().split(',')
        .filter(|value| !value.is_empty() && !["CatchAllAllow", "ExtractOnFileChange", "IgnoreFileChange"].contains(value))
        .chain(std::iter::once("IgnoreFileChange")).collect::<Vec<_>>().join(",");
    std::fs::write(settings_only.join("applied-settings.json"), serde_json::to_vec(&json!({"ExclusiveControlFeatures":applied_features})).unwrap()).unwrap();
    fixture.host.configuration.lock().await["ExclusiveControlFeatures"] = json!(applied_features);
    // 模拟收尾已持久登记、但进程尚未来得及登记任务终态的中断。
    sqlx::query(
        "DELETE FROM durable_job_steps WHERE job_id=$1 AND step_key='completed:__job_result__'",
    )
    .bind(task)
    .execute(&fixture.state.db)
    .await
    .unwrap();
    sqlx::query("UPDATE durable_jobs SET state_json=state_json||'{\"state\":\"interrupted\",\"is_running\":false,\"cleanup_pending\":true,\"stop_requested\":false,\"pause_requested\":false}'::jsonb WHERE id=$1")
        .bind(task).execute(&fixture.state.db).await.unwrap();
    sqlx::query("UPDATE media_organize_queue SET status='running',result=NULL,completed_at=NULL WHERE id=$1")
        .bind(row["id"].as_str().unwrap()).execute(&fixture.state.db).await.unwrap();
    let mut restarted = AppState::new(fixture.state.db.clone(), fixture.state.data_dir.clone())
        .await
        .unwrap();
    restarted.p115 = fixture.state.p115.clone();
    tokio::time::timeout(
        Duration::from_secs(60),
        restarted.jobs.resume_pending(
            restarted.clone(),
            tokio_util::sync::CancellationToken::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let resumed: Value =
        sqlx::query_scalar("SELECT to_jsonb(q) FROM media_organize_queue q WHERE id=$1")
            .bind(row["id"].as_str().unwrap())
            .fetch_one(&fixture.state.db)
            .await
            .unwrap();
    assert!(!settings_only.exists(), "verified settings-only capture must be cleaned");
    assert_eq!(*fixture.host.configuration.lock().await, settings);
    assert_eq!(resumed["status"], "completed", "{resumed}");
    assert_eq!(resumed["task_id"], row["task_id"], "{resumed}");
    assert_eq!(fixture.host.items.lock().await["101"], original_native);
    assert_eq!(
        std::fs::read(fixture.sidecar_path()).unwrap(),
        original_sidecar
    );
    assert_eq!(*fixture.host.native_starts.lock().await, original_starts);
    assert_eq!(
        *fixture.host.refresh_queries.lock().await,
        original_refreshes
    );
    assert_eq!(*fixture.host.file_imports.lock().await, original_imports);
    let resumed_notices: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_outbox")
        .fetch_one(&fixture.state.db)
        .await
        .unwrap();
    assert_eq!(
        resumed_notices, notices,
        "resume must not register a duplicate notification"
    );
    let finished: Vec<String> = sqlx::query_scalar("SELECT step_key FROM durable_job_steps WHERE job_id=$1 AND step_key LIKE 'completed:moviepilot_finished:%'")
        .bind(task).fetch_all(&fixture.state.db).await.unwrap();
    assert_eq!(finished, vec!["completed:moviepilot_finished:101"]);
    metadata.abort();
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; same-series batch and native extraction stay localhost"]
async fn selected_episodes_share_one_ingestion_and_one_automatic_extraction_batch() {
    let fixture = current_reorganize_fixture().await;
    let metadata = prepare_reorganize_chain(&fixture, false).await;
    fixture.state.save_config(&json!({"monitor_sha1_pc_search":false})).await.unwrap();
    let first = fixture.host.items.lock().await["101"].clone();
    let directory = std::path::Path::new(first["Path"].as_str().unwrap()).parent().unwrap();
    let path = directory.join("102.strm");
    let playback = directory.join("102.mkv");
    std::fs::write(&playback, vec![1_u8; 1000]).unwrap();
    std::fs::write(&path, playback.to_string_lossy().as_bytes()).unwrap();
    let mut second = first.clone();
    second["Id"] = json!("102"); second["Path"] = json!(path); second["IndexNumber"] = json!(2);
    second["ProviderIds"] = json!({"Tmdb":"1012"}); second["PresentationUniqueKey"] = json!("key-102");
    second["Guid"] = json!(uuid::Uuid::new_v4().simple().to_string());
    second["MediaSources"][0]["Id"] = json!("mediasource_102");
    second["MediaSources"][0]["ItemId"] = json!("102");
    second["MediaSources"][0]["Path"] = json!(playback);
    let mut second_info: Value = serde_json::from_slice(&std::fs::read(fixture.sidecar_path()).unwrap()).unwrap();
    second_info[0]["MediaSourceInfo"]["Path"] = json!(playback);
    assert!(emby_toolkit::media_info::recoverable(&second_info));
    second_info[0]["Chapters"] = second["Chapters"].clone();
    std::fs::write(directory.join("102-mediainfo.json"), serde_json::to_vec(&second_info).unwrap()).unwrap();
    fixture.host.items.lock().await.insert("102".into(), second.clone());
    configure_ingest_metadata(&fixture, "102").await;
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,parent_series_tmdb_id,season_number,episode_number,in_library,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES('1012','Episode','第二集','101',1,2,true,$1,$2,$3,$4)")
        .bind(json!(["102"])).bind(json!([{"emby_item_id":"102","path":path,"size_bytes":1000}])).bind(json!(["B".repeat(40)])).bind(json!(["pc102"])).execute(&fixture.state.db).await.unwrap();
    sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2)")
        .bind("B".repeat(40)).bind(&second_info).execute(&fixture.state.db).await.unwrap();
    for id in ["101", "102"] {
        sqlx::query("INSERT INTO media_organize_history(id,source,source_identity,item_id,item_json,status) VALUES($1,'emby',$1,$1,$2,'success')")
            .bind(id).bind(json!({"item_id":id,"item_type":"Episode","series_id":"301"})).execute(&fixture.state.db).await.unwrap();
    }
    let session = fixture.state.sessions.sign(&emby_toolkit::auth::Session {
        user_id:"operator".into(),name:"Test".into(),setup_mode:false,expires:chrono::Utc::now().timestamp()+600,
    });
    let response = emby_toolkit::actions::router().with_state(fixture.state.clone()).oneshot(
        Request::post("/api/actions/reorganize").header("content-type","application/json").header("cookie",format!("etk_session={session}"))
        .body(Body::from(json!({"item_ids":["101","102"],"mode":"reprocess"}).to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(response.status(),StatusCode::ACCEPTED);
    let cancel = tokio_util::sync::CancellationToken::new();
    let worker = tokio::spawn(emby_toolkit::media_organize_queue::run(fixture.state.clone(),cancel.clone()));
    let rows: Vec<Value> = tokio::time::timeout(Duration::from_secs(90),async {
        loop {
            let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(q) FROM media_organize_queue q ORDER BY queue_order").fetch_all(&fixture.state.db).await.unwrap();
            if rows.iter().all(|row| !matches!(row["status"].as_str(),Some("queued"|"running"))) {break rows;}
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    cancel.cancel();worker.await.unwrap();
    assert_eq!(rows.len(),2);
    assert_eq!(rows[0]["task_id"],rows[1]["task_id"],"{rows:?}");
    assert!(rows.iter().all(|row|row["status"]=="completed"),"{rows:?}");
    assert_eq!(*fixture.host.native_starts.lock().await,vec!["intro","thumb"],"{rows:?}");
    assert_eq!(fixture.host.items.lock().await["102"]["MediaSources"],second["MediaSources"]);
    metadata.abort();fixture.close().await;
}
