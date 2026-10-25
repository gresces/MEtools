//! ME personal data server: HTTP API, self-describing command proxy, and dashboard.

pub mod auth;
pub mod config;
pub mod dashboard;
pub mod data;
pub mod import;
pub mod json;
pub mod routes;
pub mod state;

use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::net::TcpListener;

use config::Config;
use state::AppState;

/// Start the HTTP server and block until shutdown.
pub async fn serve(config: Config) -> Result<()> {
    let store = Arc::new(me_store::SqliteStore::open_existing(&config.db_path)?);
    let bind = config.bind;
    let db_path = config.db_path.clone();
    let auth = if config.auth_required() {
        "password"
    } else {
        "disabled"
    };

    let state = Arc::new(AppState::new(config, store));
    let app = routes::app(state);

    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("could not bind {bind}"))?;

    tracing::info!(
        %bind,
        db = %db_path.display(),
        auth,
        "me-server listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received");
}
