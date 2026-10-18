mod access;
mod accounts;
mod app;
mod audit;
mod auth;
mod config;
mod crypto;
mod db;
mod departments;
mod email_templates;
mod error;
mod file_storage;
mod files;
mod login_rate_limit;
mod mail;
mod models;
mod onboarding;
mod project_cleanup;
mod projects;
mod proxy_identity;
mod public_url;
mod response;
mod routes;
mod seed;
mod settings;
mod share;
mod signing;
mod site_icons;
mod sites;
mod tickets;
mod updates;
mod vault;

use anyhow::Context;
use app::AppState;
use config::Config;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "crewdock_api=debug,tower_http=debug,axum=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env()?;
    let state = AppState::connect(config.clone()).await?;
    db::migrate(&state.db).await?;
    seed::bootstrap_admin(&state).await?;

    let (cleanup_shutdown_tx, cleanup_shutdown_rx) = tokio::sync::watch::channel(false);
    let cleanup_worker = tokio::spawn(project_cleanup::run_cleanup_worker(
        state.clone(),
        cleanup_shutdown_rx.clone(),
    ));
    let file_storage_worker = tokio::spawn(file_storage::run_worker(
        state.clone(),
        cleanup_shutdown_rx.clone(),
    ));
    let email_worker = tokio::spawn(mail::run_worker(state.clone(), cleanup_shutdown_rx));

    let app = routes::router(state).layer(TraceLayer::new_for_http());
    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .context("invalid bind address")?;
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "crewdock API listening");
    let shutdown_tx = cleanup_shutdown_tx.clone();
    let server_result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            let _ = shutdown_tx.send(true);
        })
        .await;
    let _ = cleanup_shutdown_tx.send(true);
    stop_worker("project cleanup", cleanup_worker).await;
    stop_worker("file storage", file_storage_worker).await;
    stop_worker("email outbox", email_worker).await;
    server_result?;
    Ok(())
}

async fn stop_worker(name: &'static str, mut worker: JoinHandle<()>) {
    match tokio::time::timeout(std::time::Duration::from_secs(5), &mut worker).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::error!(%error, worker = name, "background worker failed"),
        Err(_) => {
            tracing::warn!(
                worker = name,
                "background worker shutdown timed out; aborting"
            );
            worker.abort();
            let _ = worker.await;
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown signal received");
}
