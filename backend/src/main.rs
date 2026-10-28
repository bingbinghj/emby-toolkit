use emby_toolkit::{
    AppState, db,
    logs::{LogFormat, LogWriter},
    router,
};
use std::{net::SocketAddr, path::PathBuf};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    if args
        .get(1)
        .is_some_and(|arg| ["media-archive", "restore-media-short-names"].contains(&arg.as_str()))
    {
        return emby_toolkit::media_archive::run(&args[1..]);
    }
    if args.get(1).is_some_and(|s| s == "--clear-activity-log") {
        let path = args
            .get(2)
            .map(String::as_str)
            .unwrap_or("/config/data/activitylog.db");
        println!(
            "{}",
            emby_toolkit::activity::clear_database(std::path::Path::new(path))?
        );
        return Ok(());
    }
    let data_dir =
        PathBuf::from(std::env::var("APP_DATA_DIR").unwrap_or_else(|_| "local_data".into()));
    let db = db::connect(&data_dir).await?;
    let state = AppState::new(db, data_dir).await?;
    tracing_subscriber::fmt()
        .with_ansi(false)
        .event_format(LogFormat)
        .with_writer(LogWriter(state.logs.clone()))
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,emby_toolkit=trace".into()),
        )
        .init();
    state.refresh_server_identity().await;
    let bind: SocketAddr = std::env::var("ETK_BIND")
        .unwrap_or_else(|_| "0.0.0.0:5257".into())
        .parse()?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let background = tokio_util::sync::CancellationToken::new();
    let bot = tokio::spawn(emby_toolkit::telegram_bot::run(
        state.clone(),
        background.clone(),
    ));
    let monitor = emby_toolkit::monitor::Service::new(state.clone());
    let watcher = tokio::spawn(monitor.run(background.clone()));
    let shutdown = background.clone();
    let jobs = state.jobs.clone();
    let shutdown_jobs = jobs.clone();
    tracing::info!("应用程序启动");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                let mut terminate =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("SIGTERM handler");
                tokio::select! { _ = tokio::signal::ctrl_c()=>{}, _ = terminate.recv()=>{} }
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("应用程序正在关闭，等待后台任务安全结束");
            shutdown.cancel();
            shutdown_jobs.shutdown().await;
        })
        .await?;
    background.cancel();
    jobs.shutdown().await;
    bot.await?;
    watcher.await?;
    tracing::info!("应用程序已关闭，后台任务已结束");
    Ok(())
}
