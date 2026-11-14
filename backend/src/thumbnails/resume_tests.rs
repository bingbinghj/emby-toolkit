use super::*;
use crate::{auth::SessionSigner, jobs::JobManager, logs::LogStore};
use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post},
};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::RwLock;

const ANCHOR: &str = "2026-11-14 00:00:00.000 Info App: previous execution";
const EXECUTE: &str =
    "2026-11-14 00:00:01.000 Info App: VideoThumbnailExtract - Scheduled Task Execute";
const COUNT: &str = "2026-11-14 00:00:02.000 Info App: VideoThumbnailExtract - Number of items: 0";
const COMPLETE: &str =
    "2026-11-14 00:00:03.000 Info App: VideoThumbnailExtract - Scheduled Task Complete";

struct Host {
    native: Arc<RwLock<Value>>,
    logs: Arc<RwLock<Vec<String>>>,
    config: Arc<RwLock<Value>>,
    config_writes: Arc<AtomicUsize>,
    starts: Arc<AtomicUsize>,
    stops: Arc<AtomicUsize>,
    reads: Arc<AtomicUsize>,
    base: String,
    server: tokio::task::JoinHandle<()>,
}
impl Host {
    async fn new(mode: &'static str) -> Self {
        let native = Arc::new(RwLock::new(
            json!({"Id":if mode=="changed" {"other"}else{"extract"},
                "Key":"VideoThumbnailExtractTask","State":if matches!(mode,"running"|"foreign") {"Running"}else{"Idle"},
                "LastExecutionResult":{"StartTimeUtc":if matches!(mode,"completed"|"foreign") {"new"}else{"old"},"Status":"Completed"}}),
        ));
        let logs = Arc::new(RwLock::new(if mode == "completed" {
            vec![COMPLETE.into(), COUNT.into(), EXECUTE.into(), ANCHOR.into()]
        } else {
            vec![ANCHOR.into()]
        }));
        let starts = Arc::new(AtomicUsize::new(0));
        let config = Arc::new(RwLock::new(
            json!({"ExclusiveControlFeatures":"CatchAllAllow",
            "PersistMediaInfoMode":"Extract","CustomEncoderPath":"/usr/bin/ffmpeg","UserSetting":"保留当前修改"}),
        ));
        let config_writes = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));
        let tasks = native.clone();
        let lines = logs.clone();
        let requests = reads.clone();
        let events = logs.clone();
        let submitted = starts.clone();
        let execution = native.clone();
        let cancelled = stops.clone();
        let host_logs = logs.clone();
        let settings = config.clone();
        let saved_settings = config.clone();
        let settings_writes = config_writes.clone();
        let app = Router::new()
            .route("/web/ConfigurationPages",get(||async{Json(json!([
                {"Name":"MediaInfoExtractPageView","Href":"UI/Index.html?PageId=fixture:MediaInfoExtractPageView"}
            ]))}))
            .route("/UI/View",get(move||{let settings=settings.clone();async move{
                Json(json!({"EditObjectContainer":{"Object":settings.read().await.clone()}}))
            }}))
            .route("/UI/Command",post(move|Json(body):Json<Value>|{
                let saved_settings=saved_settings.clone();let settings_writes=settings_writes.clone();async move{
                    *saved_settings.write().await=serde_json::from_str(body["Data"].as_str().unwrap()).unwrap();
                    settings_writes.fetch_add(1,Ordering::SeqCst);StatusCode::NO_CONTENT
                }
            }))
            .route("/Users/AuthenticateByName",post(||async{Json(json!({
                "User":{"Id":"fixture-admin","Policy":{"IsAdministrator":true}},"AccessToken":"fixture-token"
            }))}))
            .route("/System/Info",get(||async{Json(json!({"Id":"fixture-server","ProgramDataPath":"/config","CachePath":"/cache"}))}))
            .route("/ScheduledTasks", get(move || {
                let tasks=tasks.clone(); let lines=lines.clone(); let requests=requests.clone();
                async move {
                    if mode=="running" && requests.fetch_add(1,Ordering::SeqCst)>0 {
                        *lines.write().await=vec![COMPLETE.into(),COUNT.into(),EXECUTE.into(),ANCHOR.into()];
                        tasks.write().await["State"]=json!("Idle");
                        tasks.write().await["LastExecutionResult"]=json!({"StartTimeUtc":"new","Status":"Completed"});
                    } else if mode!="running" {requests.fetch_add(1,Ordering::SeqCst);}
                    Json(json!([tasks.read().await.clone()]))
                }
            }))
            .route("/System/Logs/embyserver.txt/Lines",get(move || {
                let events=events.clone();async move{Json(json!({"Items":events.read().await.clone()}))}
            }))
            .route("/ScheduledTasks/Running/extract",post(move || {
                let execution=execution.clone();let logs=logs.clone();
                let attempt=submitted.fetch_add(1,Ordering::SeqCst)+1;
                async move {
                    let mut lines=logs.write().await;
                    let prior=lines.clone();
                    *lines=vec![format!("2026-11-14 00:00:{:02}.000 Info App: VideoThumbnailExtract - Number of items: 0",attempt*10+2),
                        format!("2026-11-14 00:00:{:02}.000 Info App: VideoThumbnailExtract - Scheduled Task Execute",attempt*10+1)];
                    if mode!="hold" {lines.insert(0,format!("2026-11-14 00:00:{:02}.000 Info App: VideoThumbnailExtract - Scheduled Task Complete",attempt*10+3));}
                    lines.extend(prior);
                    execution.write().await["State"]=json!(if mode=="hold" {"Running"}else{"Idle"});
                    if mode!="hold" {execution.write().await["LastExecutionResult"]=json!({"StartTimeUtc":format!("run-{attempt}"),"Status":"Completed"});}
                    StatusCode::NO_CONTENT
                }
            }).delete(move || {let cancelled=cancelled.clone();async move{
                cancelled.fetch_add(1,Ordering::SeqCst);StatusCode::NO_CONTENT
            }}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            native,
            logs: host_logs,
            config,
            config_writes,
            starts,
            stops,
            reads,
            base,
            server,
        }
    }
}

fn fixture(root: &std::path::Path, base: &str) -> AppState {
    AppState {
        db: sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap(),
        config: Arc::new(RwLock::new(
            json!({"emby_server_url":base,"emby_api_key":"fixture"}),
        )),
        http: reqwest::Client::new(),
        sessions: Arc::new(SessionSigner::load(root).unwrap()),
        recovery_tokens: Arc::new(RwLock::new(HashMap::new())),
        data_dir: root.into(),
        notifications: Arc::default(),
        webhooks: Arc::default(),
        ai: Arc::default(),
        jobs: Arc::new(JobManager::default()),
        logs: LogStore::open(&root.join("logs")).unwrap(),
        p115: Arc::default(),
        strm: Arc::default(),
        douban: Arc::default(),
        docker: Arc::new(crate::docker_host::Service { docker: None }),
    }
}
fn saved_run() -> NativeRun {
    NativeRun {
        task_id: "extract".into(),
        previous: json!("old"),
        cursor: LogCursor {
            anchor: Some(ANCHOR.into()),
        },
        trace: ExtractionLog::default(),
        observed_start: true,
        execution_start: None,
        execution_status: None,
        start_error: None,
    }
}
async fn result(manager: &JobManager, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let state = manager.result(id).await.unwrap();
            if state["is_running"] == false {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn resumed_native_execution_waits_or_reads_its_completion_without_submitting_again() {
    for mode in ["running", "completed"] {
        let host = Host::new(mode).await;
        let root = tempfile::tempdir().unwrap();
        let state = fixture(root.path(), &host.base);
        let work = state.clone();
        let id = state
            .jobs
            .submit("恢复缩略图原生执行", move |context| async move {
                context
                    .save_plan(&native_key(5, 50, None), &json!(saved_run()))
                    .await?;
                let paths = run_with_trace(
                    &work,
                    Duration::from_secs(2),
                    5,
                    50,
                    None,
                    &context,
                    &mut ExtractionLog::default(),
                )
                .await?;
                Ok(json!({"paths":paths}))
            })
            .await
            .unwrap();
        let completed = result(&state.jobs, &id).await;
        assert_eq!(completed["state"], "completed", "{mode}: {completed}");
        assert_eq!(
            host.starts.load(Ordering::SeqCst),
            0,
            "{mode}: existing native execution was replayed"
        );
        host.server.abort();
    }
}

#[tokio::test]
async fn native_completion_is_scoped_to_each_media_batch() {
    let host = Host::new("fresh").await;
    let root = tempfile::tempdir().unwrap();
    let state = fixture(root.path(), &host.base);
    let work = state.clone();
    let id = state
        .jobs
        .submit("分批缩略图执行", move |context| async move {
            for scope in ["media-A", "media-B", "media-A"] {
                run_with_trace(
                    &work,
                    Duration::from_secs(2),
                    5,
                    50,
                    None,
                    &context.scoped(scope),
                    &mut ExtractionLog::default(),
                )
                .await?;
            }
            Ok(json!({"message":"已核对两批独立执行"}))
        })
        .await
        .unwrap();
    let completed = result(&state.jobs, &id).await;
    assert_eq!(completed["state"], "completed", "{completed}");
    assert_eq!(
        host.starts.load(Ordering::SeqCst),
        2,
        "a different batch must not reuse the first batch, while its own completed retry must"
    );
    host.server.abort();
}

#[tokio::test]
async fn changed_native_task_identity_does_not_replay_an_old_submission() {
    let host = Host::new("changed").await;
    let root = tempfile::tempdir().unwrap();
    let state = fixture(root.path(), &host.base);
    let work = state.clone();
    let id = state
        .jobs
        .submit("核对原生任务身份", move |context| async move {
            context
                .save_plan(&native_key(5, 50, None), &json!(saved_run()))
                .await?;
            run_with_trace(
                &work,
                Duration::from_secs(2),
                5,
                50,
                None,
                &context,
                &mut ExtractionLog::default(),
            )
            .await?;
            Ok(json!({}))
        })
        .await
        .unwrap();
    let completed = result(&state.jobs, &id).await;
    assert_eq!(completed["state"], "failed");
    assert!(completed["message"].as_str().unwrap().contains("身份变化"));
    assert_eq!(host.starts.load(Ordering::SeqCst), 0);
    host.server.abort();
}

#[tokio::test]
async fn confirmed_plan_before_final_checkpoint_survives_a_later_native_execution() {
    let host = Host::new("changed").await;
    let root = tempfile::tempdir().unwrap();
    let state = fixture(root.path(), &host.base);
    let work = state.clone();
    let id = state
        .jobs
        .submit("保留已确认的本次执行", move |context| async move {
            let mut run = saved_run();
            run.trace
                .feed(&[EXECUTE.into(), COUNT.into(), COMPLETE.into()]);
            run.execution_start = Some(json!("owned-run"));
            run.execution_status = Some("Completed".into());
            context
                .save_plan(&native_key(5, 50, None), &json!(run))
                .await?;
            run_with_trace(
                &work,
                Duration::from_secs(2),
                5,
                50,
                None,
                &context,
                &mut ExtractionLog::default(),
            )
            .await?;
            Ok(json!({"message":"沿用崩溃前已确认的执行结果"}))
        })
        .await
        .unwrap();
    let completed = result(&state.jobs, &id).await;
    assert_eq!(completed["state"], "completed", "{completed}");
    assert_eq!(
        host.reads.load(Ordering::SeqCst),
        0,
        "another execution must not replace an already verified persisted plan"
    );
    assert_eq!(host.starts.load(Ordering::SeqCst), 0);
    host.server.abort();
}

#[tokio::test]
async fn confirmed_intro_plan_does_not_require_native_registration_after_restart() {
    let host = Host::new("changed").await;
    let root = tempfile::tempdir().unwrap();
    let state = fixture(root.path(), &host.base);
    let work = state.clone();
    let id = state
        .jobs
        .submit("恢复已完成的片头声纹阶段", move |context| async move {
            let mut trace = IntroLog::default();
            trace.feed(&[
                "IntroFingerprintExtract - Scheduled Task Execute".into(),
                "IntroFingerprintExtract - Number of episodes: 0".into(),
                "IntroFingerprintExtract - Number of seasons: 0".into(),
                "IntroFingerprintExtract - Scheduled Task Complete".into(),
            ]);
            let saved = IntroRun {
                task_id: "old-intro".into(),
                previous: json!("old"),
                cursor: LogCursor { anchor: None },
                trace,
                native_completed: true,
            };
            context.save_plan("intro-extraction", &json!(saved)).await?;
            extract_intro_verified(&work, &context).await
        })
        .await
        .unwrap();
    let completed = result(&state.jobs, &id).await;
    assert_eq!(completed["state"], "completed", "{completed}");
    assert_eq!(completed["result"]["native_ended"], true);
    assert_eq!(host.reads.load(Ordering::SeqCst), 0);
    assert_eq!(host.starts.load(Ordering::SeqCst), 0);
    host.server.abort();
}

#[tokio::test]
async fn incomplete_target_plan_cannot_turn_missing_images_into_a_successful_resume() {
    let host = Host::new("completed").await;
    let root = tempfile::tempdir().unwrap();
    let state = fixture(root.path(), &host.base);
    let work = state.clone();
    let id=state.jobs.submit("拒绝缺少原目标的恢复记录",move|context|async move{
        context.save_plan("thumbnail-extraction",&json!({"report":{"state":"failed","failed":1},
            "paths":["/media/old.strm"],"targets":{"old":[{"Id":"old","Path":"/media/old.strm"},{}]}})).await?;
        let extraction=extract_with_timeout(&work,Duration::from_secs(2),&context).await?;
        Ok(extraction.report)
    }).await.unwrap();
    let completed = result(&state.jobs, &id).await;
    assert_eq!(completed["state"], "failed");
    assert!(
        completed["message"]
            .as_str()
            .unwrap()
            .contains("目标记录无效")
    );
    assert_eq!(host.starts.load(Ordering::SeqCst), 0);
    host.server.abort();
}

#[tokio::test]
async fn replaced_native_execution_is_rejected_without_stopping_the_other_task() {
    let host = Host::new("foreign").await;
    let root = tempfile::tempdir().unwrap();
    let state = fixture(root.path(), &host.base);
    let work = state.clone();
    let id = state
        .jobs
        .submit("不停止其它原生执行", move |context| async move {
            let mut run = saved_run();
            run.execution_start = Some(json!("owned-run"));
            context
                .save_plan(&native_key(5, 50, None), &json!(run))
                .await?;
            run_with_trace(
                &work,
                Duration::from_secs(2),
                5,
                50,
                None,
                &context,
                &mut ExtractionLog::default(),
            )
            .await?;
            Ok(json!({}))
        })
        .await
        .unwrap();
    let completed = result(&state.jobs, &id).await;
    assert_eq!(completed["state"], "failed");
    assert!(
        completed["message"]
            .as_str()
            .unwrap()
            .contains("执行身份已变化")
    );
    assert_eq!(host.stops.load(Ordering::SeqCst), 0);
    assert_eq!(host.starts.load(Ordering::SeqCst), 0);
    host.server.abort();
}

#[tokio::test]
async fn late_owned_completion_logs_do_not_wait_for_a_subsequent_running_task() {
    let host = Host::new("foreign").await;
    *host.logs.write().await = vec![COMPLETE.into(), COUNT.into(), EXECUTE.into(), ANCHOR.into()];
    let root = tempfile::tempdir().unwrap();
    let state = fixture(root.path(), &host.base);
    let work = state.clone();
    let id = state
        .jobs
        .submit("只等待本次执行日志", move |context| async move {
            let mut run = saved_run();
            run.execution_start = Some(json!("owned-run"));
            run.execution_status = Some("Completed".into());
            context
                .save_plan(&native_key(5, 50, None), &json!(run))
                .await?;
            run_with_trace(
                &work,
                Duration::from_secs(2),
                5,
                50,
                None,
                &context,
                &mut ExtractionLog::default(),
            )
            .await?;
            Ok(json!({"message":"已确认本次完整日志，未等待另一次原生任务"}))
        })
        .await
        .unwrap();
    let completed = result(&state.jobs, &id).await;
    assert_eq!(completed["state"], "completed", "{completed}");
    assert_eq!(host.native.read().await["State"], "Running");
    assert_eq!(host.starts.load(Ordering::SeqCst), 0);
    assert_eq!(host.stops.load(Ordering::SeqCst), 0);
    host.server.abort();
}

#[test]
fn same_id_path_and_size_do_not_allow_old_images_to_verify_a_new_video() {
    let before = json!({"Id":"1","Path":"/media/same.strm","MediaSources":[{"ItemId":"1","Path":"/video/same.mkv","Size":2048,
        "MediaStreams":[{"Type":"Video","Codec":"h264","Width":1920,"Height":1080}]}]});
    let mut after = before.clone();
    after["MediaSources"][0]["MediaStreams"][0]["Width"] = json!(3840);
    assert!(!same_target(&before, &after));
    assert!(same_target(&before, &before));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; native API is localhost"]
async fn native_thumbnail_task_survives_real_job_manager_reopen_without_another_start() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("thumbnail_resume_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = format!("SET search_path TO {schema}");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(move |connection, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&search).execute(connection).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut state = AppState::new(pool.clone(), root.path().into())
        .await
        .unwrap();
    state.docker = Arc::new(crate::docker_host::Service { docker: None });
    let host = Host::new("hold").await;
    *state.config.write().await = json!({"emby_server_url":host.base,"emby_api_key":"fixture"});
    let work = state.clone();
    let owner = json!({"kind":"thumbnail-test","queue_id":"fixture"});
    let (send_context, receive_context) = tokio::sync::oneshot::channel();
    let id = state
        .jobs
        .submit_owned(
            "可恢复缩略图任务",
            owner.clone(),
            None,
            move |context| async move {
                let _ = send_context.send(context.clone());
                run_with_trace(
                    &work,
                    Duration::from_secs(30),
                    5,
                    50,
                    None,
                    &context,
                    &mut ExtractionLog::default(),
                )
                .await?;
                Ok(json!({"message":"本次执行与日志均已验收"}))
            },
        )
        .await
        .unwrap();
    let context = receive_context.await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if context
                .plan(&native_key(5, 50, None))
                .await
                .unwrap()
                .is_some_and(|checkpoint| checkpoint["trace"]["started"] == true)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), state.jobs.shutdown())
        .await
        .unwrap();
    assert_eq!(
        state.jobs.result(&id).await.unwrap()["state"],
        "interrupted"
    );
    assert_eq!(
        host.stops.load(Ordering::SeqCst),
        0,
        "restart must preserve the native task, rather than cancel it"
    );
    host.native.write().await["State"] = json!("Idle");
    host.native.write().await["LastExecutionResult"] =
        json!({"StartTimeUtc":"run-1","Status":"Completed"});
    host.logs.write().await.insert(
        0,
        "2026-11-14 00:00:13.000 Info App: VideoThumbnailExtract - Scheduled Task Complete".into(),
    );
    state.jobs = Arc::new(JobManager::open(pool.clone()).await.unwrap());
    let work = state.clone();
    let restored = state
        .jobs
        .submit_owned(
            "可恢复缩略图任务",
            owner,
            Some(&id),
            move |context| async move {
                assert!(context.is_resuming());
                run_with_trace(
                    &work,
                    Duration::from_secs(2),
                    5,
                    50,
                    None,
                    &context,
                    &mut ExtractionLog::default(),
                )
                .await?;
                Ok(json!({"message":"恢复后沿用原生执行"}))
            },
        )
        .await
        .unwrap();
    assert_eq!(restored, id, "resume must keep its original task identity");
    let completed = result(&state.jobs, &restored).await;
    assert_eq!(completed["state"], "completed", "{completed}");
    assert_eq!(
        host.starts.load(Ordering::SeqCst),
        1,
        "reopen must not submit an already completed native execution again"
    );
    assert!(host.reads.load(Ordering::SeqCst) > 0);
    state.jobs.shutdown().await;
    host.server.abort();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; native and configuration APIs are localhost"]
async fn paused_scoped_thumbnail_restart_restores_encoder_before_waiting_for_resume() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("thumbnail_paused_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = format!("SET search_path TO {schema}");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(move |connection, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&search).execute(connection).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut state = AppState::new(pool.clone(), root.path().into())
        .await
        .unwrap();
    state.docker = Arc::new(crate::docker_host::Service { docker: None });
    let host = Host::new("completed").await;
    *state.config.write().await = json!({"emby_server_url":host.base,"emby_api_key":"fixture",
        "emby_admin_user":"fixture-admin","emby_admin_pass":"fixture-password"});
    let folder = format!("etk-thumbnail-{}", uuid::Uuid::new_v4().simple());
    let script = format!("/cache/temp/{folder}/ffmpeg");
    host.config.write().await["CustomEncoderPath"] = json!(script);
    let owner = json!({"kind":"thumbnail-test","queue_id":"paused"});
    let (send_context, receive_context) = tokio::sync::oneshot::channel();
    let saved_script = script.clone();
    let id = state
        .jobs
        .submit_owned(
            "暂停的分批缩略图任务",
            owner.clone(),
            None,
            move |context| async move {
                let scoped = context.scoped("moviepilot:batch-a:thumbnail-repair");
                let plan = FallbackPlan {
                    original_encoder: "/usr/bin/ffmpeg".into(),
                    system_id: "fixture-server".into(),
                    container_id: "fixture-container".into(),
                    directory: "/cache/temp".into(),
                    folder,
                    script: saved_script.clone(),
                    rules: vec![],
                    cursor: LogCursor {
                        anchor: Some(ANCHOR.into()),
                    },
                    uploaded: true,
                    backend_verified: true,
                };
                scoped.save_plan("thumbnail-fallback", &json!(plan)).await?;
                scoped
                    .save_plan(
                        &native_key(65, 95, Some(&saved_script)),
                        &json!(saved_run()),
                    )
                    .await?;
                let _ = send_context.send(context.clone());
                loop {
                    context.checkpoint().await?;
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                #[allow(unreachable_code)]
                Ok(json!({}))
            },
        )
        .await
        .unwrap();
    let _ = receive_context.await.unwrap();
    state.jobs.control(Some(&id), "pause").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if state.jobs.result(&id).await.unwrap()["state"] == "paused" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    state.jobs.shutdown().await;
    assert_eq!(state.jobs.result(&id).await.unwrap()["state"], "paused");
    state.jobs = Arc::new(JobManager::open(pool.clone()).await.unwrap());
    tokio::time::timeout(
        Duration::from_secs(5),
        state
            .jobs
            .resume_pending(state.clone(), tokio_util::sync::CancellationToken::new()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        host.config.read().await["CustomEncoderPath"],
        "/usr/bin/ffmpeg"
    );
    assert_eq!(host.config.read().await["UserSetting"], "保留当前修改");
    assert_eq!(host.config_writes.load(Ordering::SeqCst), 1);
    assert_eq!(host.starts.load(Ordering::SeqCst), 0);
    let executed = Arc::new(AtomicUsize::new(0));
    let run = executed.clone();
    let work = state.clone();
    let restored = state
        .jobs
        .submit_owned(
            "暂停的分批缩略图任务",
            owner,
            Some(&id),
            move |context| async move {
                run.fetch_add(1, Ordering::SeqCst);
                run_with_trace(
                    &work,
                    Duration::from_secs(2),
                    65,
                    95,
                    Some(&script),
                    &context.scoped("moviepilot:batch-a:thumbnail-repair"),
                    &mut ExtractionLog::default(),
                )
                .await?;
                Ok(json!({"message":"保留暂停意图后继续原任务"}))
            },
        )
        .await
        .unwrap();
    assert_eq!(restored, id);
    assert_eq!(state.jobs.result(&id).await.unwrap()["state"], "paused");
    assert_eq!(executed.load(Ordering::SeqCst), 0);
    state.jobs.control(Some(&id), "resume").await.unwrap();
    let completed = result(&state.jobs, &id).await;
    assert_eq!(completed["state"], "completed", "{completed}");
    assert_eq!(executed.load(Ordering::SeqCst), 1);
    assert_eq!(
        host.starts.load(Ordering::SeqCst),
        0,
        "root cleanup must preserve a scoped completed native run"
    );
    state.jobs.shutdown().await;
    host.server.abort();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
