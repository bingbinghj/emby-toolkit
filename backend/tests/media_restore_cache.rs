use axum::{Json, Router, routing::post};
use emby_toolkit::{AppState, db, media_restore, p115};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

struct Fixture {
    state: AppState,
    root: sqlx::PgPool,
    schema: String,
    directory: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let root = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("restore_cache_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&root)
            .await
            .unwrap();
        let path = format!("SET search_path TO {schema}");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |connection, _| {
                let path = path.clone();
                Box::pin(async move {
                    sqlx::query(&path).execute(connection).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::new(pool, directory.path().into()).await.unwrap();
        Self {
            state,
            root,
            schema,
            directory,
        }
    }
    async fn seed(&self, info: &Value, raw: &Value) {
        sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json,raw_ffprobe_json) VALUES('CACHE-SHA',$1,$2)")
            .bind(info).bind(raw).execute(&self.state.db).await.unwrap();
    }
    async fn read(&self) -> Value {
        sqlx::query_scalar("SELECT to_jsonb(c) FROM p115_mediainfo_cache c WHERE sha1='CACHE-SHA'")
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }
    async fn restore(&self) -> Value {
        let state = self.state.clone();
        let id = self
            .state
            .jobs
            .submit("读取原媒体备份", move |context| async move {
                media_restore::info(
                    &state,
                    "CACHE-SHA",
                    "unused",
                    "episode.mkv",
                    false,
                    &context,
                )
                .await
            })
            .await
            .unwrap();
        ended(&self.state, &id).await
    }
    async fn close(self) {
        self.state.jobs.shutdown().await;
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.root)
            .await
            .unwrap();
        self.root.close().await;
    }
}
async fn ended(state: &AppState, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let task = state.jobs.result(id).await.unwrap();
            if task["is_running"] == false {
                break task;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("local operation must finish without waiting for the probe timeout")
}
fn raw_hdr() -> Value {
    json!({"format":{"size":"1000","duration":"2","format_name":"matroska"},
        "streams":[{"codec_type":"video","codec_name":"hevc","index":0,"width":3840,"height":2160,
            "pix_fmt":"yuv420p10le","color_space":"bt2020nc","color_transfer":"smpte2084","color_primaries":"bt2020"},
            {"codec_type":"audio","codec_name":"aac","index":1,"channels":2,"sample_rate":"48000","tags":{"language":"chi"}}],
        "chapters":[{"start_time":"0","tags":{"title":"原始章节"}}]})
}
fn old_hdr(raw: &Value) -> Value {
    let mut info =
        emby_toolkit::probe::build(raw, &json!({"fn":"episode.mkv"}), &json!([])).unwrap();
    let video = &mut info[0]["MediaSourceInfo"]["MediaStreams"][0];
    video["ExtendedVideoType"] = json!("None");
    video["ExtendedVideoSubType"] = json!("None");
    video["ExtendedVideoSubTypeDescription"] = json!("None");
    video["VideoRange"] = json!("HDR10");
    info[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("用户保存的国语备注");
    info[0]["MediaSourceInfo"]["MediaStreams"][1]["Language"] = json!("zho");
    info[0]["Chapters"] = json!([{"Name":"用户片头","MarkerType":"IntroStart","StartPositionTicks":0},
        {"Name":"用户片头结束","MarkerType":"IntroEnd","StartPositionTicks":10_000_000}]);
    info[0]["UnknownUserField"] = json!({"keep":"完整保留"});
    info
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; cached HDR repair never reads remote media"]
async fn reused_legacy_hdr_cache_repairs_only_classification_and_keeps_user_edits() {
    let f = Fixture::new().await;
    let raw = raw_hdr();
    let info = old_hdr(&raw);
    f.seed(&info, &raw).await;
    let cache_before = f.read().await;
    let task = f.restore().await;
    let cache_after = f.read().await;
    f.close().await;
    assert_eq!(task["state"], "completed", "{task}");
    let mut expected = info;
    let video = &mut expected[0]["MediaSourceInfo"]["MediaStreams"][0];
    for (field, value) in [
        ("ExtendedVideoType", "Hdr10"),
        ("ExtendedVideoSubType", "Hdr10"),
        ("VideoRange", "HDR 10"),
        ("ExtendedVideoSubTypeDescription", "HDR 10"),
    ] {
        video[field] = json!(value);
    }
    assert_eq!(
        task["result"], expected,
        "all other fields, including edits and unknown fields, must remain intact"
    );
    assert_eq!(
        cache_after, cache_before,
        "reading a repaired projection must not overwrite the saved cache"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; ambiguous old caches are not replaced or imported"]
async fn reused_legacy_hdr_cache_refuses_missing_or_mismatched_raw_evidence() {
    for fault in ["missing", "size", "colors", "codec", "index"] {
        let f = Fixture::new().await;
        let original_raw = raw_hdr();
        let info = old_hdr(&original_raw);
        let mut raw = original_raw;
        match fault {
            "missing" => raw = Value::Null,
            "size" => raw["format"]["size"] = json!("1001"),
            "colors" => raw["streams"][0]["color_transfer"] = json!("bt709"),
            "codec" => raw["streams"][0]["codec_name"] = json!("h264"),
            "index" => raw["streams"][0]["index"] = json!(4),
            _ => unreachable!(),
        }
        f.seed(&info, &raw).await;
        let cache_before = f.read().await;
        let task = f.restore().await;
        let cache_after = f.read().await;
        f.close().await;
        assert_eq!(task["state"], "failed", "{fault}: {task}");
        assert!(
            task["message"].as_str().unwrap().contains("原始探测"),
            "{fault}: {task}"
        );
        assert_eq!(
            cache_after, cache_before,
            "{fault}: existing data must not be replaced"
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; valid edited cache remains byte-equivalent JSON"]
async fn reused_consistent_video_cache_does_not_require_raw_or_rewrite_edits() {
    let f = Fixture::new().await;
    let mut info =
        emby_toolkit::probe::build(&raw_hdr(), &json!({"fn":"episode.mkv"}), &json!([])).unwrap();
    info[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("自定义音轨");
    info[0]["Chapters"] =
        json!([{"Name":"自定义章节","MarkerType":"Chapter","StartPositionTicks":0}]);
    f.seed(&info, &Value::Null).await;
    let cache_before = f.read().await;
    let task = f.restore().await;
    let cache_after = f.read().await;
    f.close().await;
    assert_eq!(task["state"], "completed", "{task}");
    assert_eq!(task["result"], info);
    assert_eq!(cache_after, cache_before);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; readonly subtitle repair preserves all other saved data"]
async fn reused_subtitle_cache_recomputes_only_proven_internal_getters() {
    let f = Fixture::new().await;
    let mut info =
        emby_toolkit::probe::build(&raw_hdr(), &json!({"fn":"episode.mkv"}), &json!([])).unwrap();
    let subtitle = json!({"Type":"Subtitle","Codec":"TTML","Index":100,"IsExternal":false,
        "Title":"用户字幕备注","Language":"chi","IsTextSubtitleStream":false,"SubtitleLocationType":"InternalStream",
        "UnknownSubtitleField":{"preserve":true}});
    info[0]["MediaSourceInfo"]["MediaStreams"]
        .as_array_mut()
        .unwrap()
        .push(subtitle);
    info[0]["Chapters"] =
        json!([{"Name":"用户章节","MarkerType":"Chapter","StartPositionTicks":0}]);
    f.seed(&info, &Value::Null).await;
    let before = f.read().await;
    let task = f.restore().await;
    let after = f.read().await;
    let mut expected = info.clone();
    expected[0]["MediaSourceInfo"]["MediaStreams"][2]["IsTextSubtitleStream"] = json!(true);
    expected[0]["MediaSourceInfo"]["MediaStreams"][2]["SupportsExternalStream"] = json!(true);
    expected[0]["MediaSourceInfo"]["MediaStreams"][2]["SubtitleLocationType"] =
        json!("VideoSideData");
    assert_eq!(task["state"], "completed", "{task}");
    assert_eq!(task["result"], expected);
    assert_eq!(after, before, "projection must not overwrite the cache");
    for (field, value) in [
        ("IsExternal", json!(true)),
        ("IsExternal", Value::Null),
        ("Index", json!("100")),
        ("Index", json!(100.5)),
        ("Index", json!(4_294_967_296_i64)),
        ("Codec", json!(123)),
        ("Codec", Value::Null),
        ("Codec", json!("")),
    ] {
        let mut invalid = info.clone();
        invalid[0]["MediaSourceInfo"]["MediaStreams"][2][field] = value;
        sqlx::query("UPDATE p115_mediainfo_cache SET mediainfo_json=$1 WHERE sha1='CACHE-SHA'")
            .bind(&invalid)
            .execute(&f.state.db)
            .await
            .unwrap();
        let before = f.read().await;
        let task = f.restore().await;
        assert_eq!(task["state"], "completed", "{field}: {task}");
        assert_eq!(
            task["result"], invalid,
            "invalid or external foundations must not be guessed"
        );
        assert_eq!(f.read().await, before);
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; capability projection preserves editable track values"]
async fn reused_internal_subtitle_cache_repairs_capability_without_rewriting_track_edits() {
    for (codec, capability) in [
        ("ASS", true),
        ("SUBRIP", true),
        ("TTML", true),
        ("PGSSUB", false),
    ] {
        let f = Fixture::new().await;
        let mut info =
            emby_toolkit::probe::build(&raw_hdr(), &json!({"fn":"episode.mkv"}), &json!([]))
                .unwrap();
        info[0]["MediaSourceInfo"]["MediaStreams"][1]["ChannelLayout"] = json!("用户保留的布局");
        info[0]["MediaSourceInfo"]["MediaStreams"]
            .as_array_mut()
            .unwrap()
            .push(json!({
            "Type":"Subtitle","Codec":codec,"Index":2,"IsExternal":false,
            "IsTextSubtitleStream":capability,"SubtitleLocationType":"InternalStream",
            "SupportsExternalStream":!capability,"IsHearingImpaired":true,
            "Title":"用户字幕标题","Language":"chi","UserExtension":{"keep":true}}));
        f.seed(&info, &Value::Null).await;
        let before = f.read().await;
        let task = f.restore().await;
        let after = f.read().await;
        f.close().await;
        let mut expected = info;
        expected[0]["MediaSourceInfo"]["MediaStreams"][2]["SupportsExternalStream"] =
            json!(capability);
        assert_eq!(task["state"], "completed", "{codec}: {task}");
        assert_eq!(
            task["result"], expected,
            "{codec}: editable values must remain intact"
        );
        assert_eq!(
            after, before,
            "{codec}: repaired projection must not overwrite cache"
        );
    }
}

#[cfg(unix)]
struct PathGuard(Option<std::ffi::OsString>);
#[cfg(unix)]
impl Drop for PathGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(path) = &self.0 {
                std::env::set_var("PATH", path);
            } else {
                std::env::remove_var("PATH");
            }
        }
    }
}
#[cfg(unix)]
fn fake_ffprobe(directory: &std::path::Path, blocked: bool) -> PathGuard {
    use std::os::unix::fs::PermissionsExt;
    let bin = directory.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let executable = bin.join("ffprobe");
    let output = if blocked {
        "exec /bin/sleep 30\n".to_string()
    } else {
        format!("cat <<'RAW'\n{}\nRAW\n", raw_hdr())
    };
    std::fs::write(
        &executable,
        format!(
            "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\n{output}",
            directory.join("probe.started").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let previous = std::env::var_os("PATH");
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(
        previous.as_deref().unwrap_or_default(),
    ));
    unsafe {
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
    }
    PathGuard(previous)
}

#[cfg(unix)]
async fn cloud_probe_fixture(
    blocked: bool,
) -> (Fixture, tokio::task::JoinHandle<()>, PathGuard, String) {
    let mut f = Fixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let url = format!("{base}/unused.mkv");
    let app = Router::new().route(
        "/app/chrome/downurl",
        post(move || {
            let url = url.clone();
            async move { Json(json!({"state":true,"data":{"url":url}})) }
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    f.state.p115 = Arc::new(p115::Service::with_endpoints(p115::Endpoints {
        openapi: base,
        ..Default::default()
    }));
    db::save_setting(
        &f.state.db,
        "p115_auth_tokens",
        &json!({"cookie":"fixture"}),
    )
    .await
    .unwrap();
    let guard = fake_ffprobe(f.directory.path(), blocked);
    let state = f.state.clone();
    let id = f
        .state
        .jobs
        .submit("提取本地测试媒体", move |context| async move {
            media_restore::info(&state, "PROBE-SHA", "12345", "episode.mkv", true, &context).await
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !f.directory.path().join("probe.started").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fake ffprobe must start through the actual cloud probe route");
    (f, server, guard, id)
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires disposable PostgreSQL; run --test-threads=1 for isolated fake ffprobe PATH"]
async fn cancelling_cloud_probe_kills_the_child_without_waiting_or_writing_cache() {
    let (f, server, guard, id) = cloud_probe_fixture(true).await;
    let pid = std::fs::read_to_string(f.directory.path().join("probe.started")).unwrap();
    f.state.jobs.control(Some(&id), "stop").await.unwrap();
    let ended_result = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let task = f.state.jobs.result(&id).await.unwrap();
            if task["is_running"] == false {
                break task;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    if ended_result.is_err() {
        // A failing baseline must not leave its deliberately blocked child alive.
        tokio::process::Command::new("/bin/kill")
            .args(["-KILL", &pid])
            .status()
            .await
            .unwrap();
    }
    let task = ended(&f.state, &id).await;
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM p115_mediainfo_cache")
        .fetch_one(&f.state.db)
        .await
        .unwrap();
    let child_stopped = tokio::time::timeout(Duration::from_secs(2), async {
        while std::path::Path::new(&format!("/proc/{pid}")).exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok();
    drop(guard);
    server.abort();
    f.close().await;
    assert!(
        ended_result.is_ok(),
        "cancel must finish promptly rather than waiting for 30/45-second probe timeouts"
    );
    assert_eq!(task["state"], "cancelled", "{task}");
    assert_eq!(rows, 0);
    assert!(
        child_stopped,
        "kill_on_drop must terminate the same ffprobe PID"
    );
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires disposable PostgreSQL; run --test-threads=1 for isolated fake ffprobe PATH"]
async fn successful_cloud_probe_keeps_its_result_and_persists_raw_evidence() {
    let (f, server, guard, id) = cloud_probe_fixture(false).await;
    let task = ended(&f.state, &id).await;
    let raw: Value = sqlx::query_scalar(
        "SELECT raw_ffprobe_json FROM p115_mediainfo_cache WHERE sha1='PROBE-SHA'",
    )
    .fetch_one(&f.state.db)
    .await
    .unwrap();
    drop(guard);
    server.abort();
    f.close().await;
    assert_eq!(task["state"], "completed", "{task}");
    assert_eq!(
        task["result"][0]["MediaSourceInfo"]["MediaStreams"][0]["VideoRange"],
        "HDR 10"
    );
    assert_eq!(raw, raw_hdr());
}
