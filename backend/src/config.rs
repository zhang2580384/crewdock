use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{env, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub public_base_url: String,
    pub icon_private_origins: Vec<String>,
    pub trusted_proxy_token: Option<String>,
    pub database_url: String,
    pub redis_url: String,
    pub file_root: PathBuf,
    pub master_key: [u8; 32],
    pub session_secret: String,
    pub session_ttl_days: i64,
    pub session_refresh_hours: i64,
    pub libreoffice_bin: String,
    pub pdftoppm_bin: String,
    pub update_control_root: Option<PathBuf>,
    pub bootstrap_email: String,
    pub bootstrap_password: String,
    pub bootstrap_name: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let master_key_raw = required_env("CREWDOCK_MASTER_KEY")?;
        let decoded = STANDARD
            .decode(master_key_raw.as_bytes())
            .context("CREWDOCK_MASTER_KEY must be base64")?;
        let master_key: [u8; 32] = decoded
            .try_into()
            .map_err(|_| anyhow::anyhow!("CREWDOCK_MASTER_KEY must decode to exactly 32 bytes"))?;
        anyhow::ensure!(
            master_key.iter().any(|byte| *byte != 0),
            "CREWDOCK_MASTER_KEY must be randomly generated; an all-zero key is not allowed"
        );
        let session_ttl_days = bounded_i64_env("CREWDOCK_SESSION_TTL_DAYS", 90, 1, 3650)?;
        let session_refresh_hours =
            bounded_i64_env("CREWDOCK_SESSION_REFRESH_HOURS", 24, 1, 24 * 30)?;
        if session_refresh_hours >= session_ttl_days * 24 {
            anyhow::bail!(
                "CREWDOCK_SESSION_REFRESH_HOURS must be shorter than CREWDOCK_SESSION_TTL_DAYS"
            );
        }

        Ok(Self {
            icon_private_origins: crate::site_icons::parse_private_origins(&env_or(
                "CREWDOCK_ICON_PRIVATE_ORIGINS",
                "",
            ))
            .context("invalid CREWDOCK_ICON_PRIVATE_ORIGINS")?,
            trusted_proxy_token: optional_proxy_token()?,
            host: env_or("CREWDOCK_HOST", "0.0.0.0"),
            port: env_or("CREWDOCK_PORT", "16888")
                .parse()
                .context("invalid CREWDOCK_PORT")?,
            public_base_url: env_or("CREWDOCK_PUBLIC_BASE_URL", "http://127.0.0.1:16888")
                .trim_end_matches('/')
                .to_string(),
            database_url: required_env("CREWDOCK_DATABASE_URL")?,
            redis_url: env_or("CREWDOCK_REDIS_URL", "redis://127.0.0.1:6379/0"),
            file_root: PathBuf::from(env_or("CREWDOCK_FILE_ROOT", "./deploy/files")),
            master_key,
            session_secret: required_env("CREWDOCK_SESSION_SECRET")?,
            session_ttl_days,
            session_refresh_hours,
            libreoffice_bin: env_or("CREWDOCK_LIBREOFFICE_BIN", "libreoffice"),
            pdftoppm_bin: env_or("CREWDOCK_PDFTOPPM_BIN", "pdftoppm"),
            update_control_root: optional_path_env("CREWDOCK_UPDATE_CONTROL_ROOT"),
            bootstrap_email: required_env("CREWDOCK_BOOTSTRAP_EMAIL")?,
            bootstrap_password: required_env("CREWDOCK_BOOTSTRAP_PASSWORD")?,
            bootstrap_name: env_or("CREWDOCK_BOOTSTRAP_NAME", "System Admin"),
        })
    }
}

fn env_or(key: &str, fallback: &str) -> String {
    env::var(key).unwrap_or_else(|_| fallback.to_string())
}

fn required_env(key: &str) -> Result<String> {
    let value = env::var(key).with_context(|| format!("{key} is required"))?;
    if value.trim().is_empty() {
        anyhow::bail!("{key} must not be empty");
    }
    Ok(value)
}

fn optional_proxy_token() -> Result<Option<String>> {
    let token = env_or("CREWDOCK_TRUSTED_PROXY_TOKEN", "");
    if token.is_empty() {
        return Ok(None);
    }
    anyhow::ensure!(
        token.len() >= 32 && token.bytes().all(|b| b.is_ascii_alphanumeric()),
        "CREWDOCK_TRUSTED_PROXY_TOKEN must contain at least 32 alphanumeric characters"
    );
    Ok(Some(token))
}

fn optional_path_env(key: &str) -> Option<PathBuf> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn bounded_i64_env(key: &str, fallback: i64, min: i64, max: i64) -> Result<i64> {
    let value = env::var(key)
        .unwrap_or_else(|_| fallback.to_string())
        .parse::<i64>()
        .with_context(|| format!("{key} must be an integer"))?;
    if !(min..=max).contains(&value) {
        anyhow::bail!("{key} must be between {min} and {max}");
    }
    Ok(value)
}
