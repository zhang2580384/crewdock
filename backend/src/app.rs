use crate::{config::Config, crypto::Crypto};
use anyhow::{Context, Result};
use redis::Client as RedisClient;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Notify;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub redis: RedisClient,
    pub config: Config,
    pub configured_file_root: PathBuf,
    pub crypto: Arc<Crypto>,
    pub project_cleanup_notify: Arc<Notify>,
    pub file_storage_notify: Arc<Notify>,
}

impl AppState {
    pub async fn connect(mut config: Config) -> Result<Self> {
        let configured_file_root = config.file_root.clone();
        let db = PgPoolOptions::new()
            .max_connections(12)
            .connect(&config.database_url)
            .await
            .context("connect postgres")?;
        let redis = RedisClient::open(config.redis_url.clone()).context("connect redis client")?;
        tokio::fs::create_dir_all(&config.file_root)
            .await
            .context("create file root")?;
        config.file_root = tokio::fs::canonicalize(&config.file_root)
            .await
            .context("canonicalize file root")?;
        let crypto = Arc::new(Crypto::new(config.master_key));
        Ok(Self {
            db,
            redis,
            config,
            configured_file_root,
            crypto,
            project_cleanup_notify: Arc::new(Notify::new()),
            file_storage_notify: Arc::new(Notify::new()),
        })
    }
}
