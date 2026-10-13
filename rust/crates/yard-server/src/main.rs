//! The shipyard web server: stores uploaded designs and serves the browser client.

mod app;
mod chat;
mod history;
mod importer;
mod models;
mod palette;
mod sessions;
mod smoothing;
mod store;
mod sync;

use anyhow::Context;
use std::net::SocketAddr;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = app::Config {
        starmade_dir: env_path("YARD_STARMADE_DIR", "starmade"),
        data_dir: env_path("YARD_DATA_DIR", "data"),
        static_dir: env_path("YARD_STATIC_DIR", "web/dist"),
        sync_token: std::env::var("YARD_SYNC_TOKEN").ok(),
        sync_quiet_secs: match std::env::var("YARD_SYNC_QUIET_SECS") {
            Ok(text) => text.parse().context("YARD_SYNC_QUIET_SECS must be a number of seconds")?,
            Err(_) => sync::QUIET_SECS,
        },
    };
    let bind: SocketAddr = std::env::var("YARD_BIND")
        .unwrap_or_else(|_| "127.0.0.1:4280".into())
        .parse()
        .context("YARD_BIND must be an address like 127.0.0.1:4280")?;

    let state = app::AppState::load(config).await?;
    let listener = tokio::net::TcpListener::bind(bind).await.with_context(|| format!("cannot listen on {bind}"))?;
    tracing::info!("shipyard listening on http://{bind}");

    // Edited designs are written to disk shortly after the editing pauses.
    let saver = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let state = saver.clone();
            let _ = tokio::task::spawn_blocking(move || state.flush(false)).await;
        }
    });

    let closing = state.clone();
    axum::serve(listener, app::router(state))
        .with_graceful_shutdown(async move {
            stop_signal().await;
            tracing::info!("stopping; saving open designs");
            let _ = tokio::task::spawn_blocking(move || closing.flush(true)).await;
        })
        .await?;
    Ok(())
}

/// Resolves on Ctrl-C or, on Unix, when the process is asked to terminate.
async fn stop_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => tokio::select! {
                _ = interrupt => {}
                _ = terminate.recv() => {}
            },
            Err(_) => interrupt.await,
        }
    }
    #[cfg(not(unix))]
    interrupt.await;
}

fn env_path(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(default))
}
