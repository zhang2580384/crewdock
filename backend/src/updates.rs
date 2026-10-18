use crate::{
    app::AppState,
    audit::log_event,
    auth::CurrentUser,
    error::{ApiError, ApiResult},
    models::SystemRole,
    response::{ApiEnvelope, ok},
};
use axum::{Json, extract::State};
use chrono::Utc;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{io::ErrorKind, path::Path};
use uuid::Uuid;

const STATUS_FILE: &str = "status.json";
const REQUEST_FILE: &str = "request.json";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RunnerStatus {
    state: String,
    operation: Option<String>,
    latest_version: Option<String>,
    target_version: Option<String>,
    message: Option<String>,
    error: Option<String>,
    progress: Option<u8>,
    checked_at: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatusResponse {
    current_version: String,
    latest_version: Option<String>,
    target_version: Option<String>,
    update_available: bool,
    updater_configured: bool,
    auto_check_enabled: bool,
    can_check: bool,
    can_apply: bool,
    state: String,
    operation: Option<String>,
    message: String,
    error: Option<String>,
    progress: u8,
    checked_at: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ApplyUpdateRequest {
    target_version: String,
}

#[derive(Debug, Serialize)]
struct RunnerRequest {
    schema_version: u8,
    request_id: Uuid,
    action: &'static str,
    target_version: Option<String>,
    requested_by: Uuid,
    requested_at: String,
}

pub async fn get_status(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<UpdateStatusResponse>>> {
    ensure_system_admin(&current)?;
    Ok(ok(read_status(&state).await?))
}

pub async fn check_for_updates(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<UpdateStatusResponse>>> {
    ensure_system_admin(&current)?;
    let request_id = enqueue(&state, &current, "check", None).await?;
    log_update_request(&state, &current, request_id, "check", None).await;
    Ok(ok(read_status(&state).await?))
}

pub async fn apply_update(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(payload): Json<ApplyUpdateRequest>,
) -> ApiResult<Json<ApiEnvelope<UpdateStatusResponse>>> {
    ensure_system_admin(&current)?;
    let target = payload.target_version.trim().to_string();
    strict_release_tag(&target).map_err(ApiError::BadRequest)?;

    let current_version = Version::parse(env!("CARGO_PKG_VERSION"))
        .expect("package version must be valid semantic version");
    let target_version = strict_release_tag(&target).map_err(ApiError::BadRequest)?;
    if target_version <= current_version {
        return Err(ApiError::BadRequest(
            "target update version must be newer than the running version".to_string(),
        ));
    }

    let runner = load_runner_status(configured_root(&state)?).await?;
    if runner.latest_version.as_deref() != Some(target.as_str()) {
        return Err(ApiError::Conflict(
            "target update is no longer the latest checked release".to_string(),
        ));
    }

    let request_id = enqueue(&state, &current, "apply", Some(target.clone())).await?;
    log_update_request(&state, &current, request_id, "apply", Some(&target)).await;
    Ok(ok(read_status(&state).await?))
}

async fn read_status(state: &AppState) -> ApiResult<UpdateStatusResponse> {
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .expect("package version must be valid semantic version");
    let current_tag = format!("v{current}");
    let Some(root) = state.config.update_control_root.as_deref() else {
        return Ok(unconfigured_status(current_tag));
    };
    if !directory_exists(root).await {
        return Ok(unconfigured_status(current_tag));
    }

    let runner = load_runner_status(root).await?;
    let request_pending = tokio::fs::try_exists(root.join(REQUEST_FILE))
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let busy = is_active_state(&runner.state);
    let latest_version = runner.latest_version.as_deref().and_then(|tag| {
        strict_release_tag(tag)
            .ok()
            .map(|version| (tag.to_string(), version))
    });
    let update_available = latest_version
        .as_ref()
        .is_some_and(|(_, version)| version > &current);
    let state_name = if request_pending && !busy {
        "queued".to_string()
    } else if runner.state.is_empty() {
        "idle".to_string()
    } else {
        runner.state.clone()
    };
    let can_check = !busy && !request_pending;

    Ok(UpdateStatusResponse {
        current_version: current_tag,
        latest_version: latest_version.map(|(tag, _)| tag),
        target_version: runner.target_version,
        update_available,
        updater_configured: true,
        auto_check_enabled: true,
        can_check,
        can_apply: can_check && update_available,
        state: state_name.clone(),
        operation: runner.operation,
        message: runner
            .message
            .unwrap_or_else(|| default_message(&state_name).to_string()),
        error: runner.error,
        progress: if state_name == "queued" {
            3
        } else {
            runner.progress.unwrap_or(0).min(100)
        },
        checked_at: runner.checked_at,
        started_at: runner.started_at,
        finished_at: runner.finished_at,
    })
}

fn unconfigured_status(current_version: String) -> UpdateStatusResponse {
    UpdateStatusResponse {
        current_version,
        latest_version: None,
        target_version: None,
        update_available: false,
        updater_configured: false,
        auto_check_enabled: false,
        can_check: false,
        can_apply: false,
        state: "not_configured".to_string(),
        operation: None,
        message: "主机更新服务尚未安装".to_string(),
        error: None,
        progress: 0,
        checked_at: None,
        started_at: None,
        finished_at: None,
    }
}

async fn enqueue(
    state: &AppState,
    current: &CurrentUser,
    action: &'static str,
    target_version: Option<String>,
) -> ApiResult<Uuid> {
    let root = configured_root(state)?;
    if !directory_exists(root).await {
        return Err(ApiError::Unavailable("update service"));
    }
    let runner = load_runner_status(root).await?;
    if is_active_state(&runner.state)
        || tokio::fs::try_exists(root.join(REQUEST_FILE))
            .await
            .map_err(|error| ApiError::Internal(error.into()))?
    {
        return Err(ApiError::Busy("update service"));
    }

    let request_id = Uuid::new_v4();
    let request = RunnerRequest {
        schema_version: 1,
        request_id,
        action,
        target_version,
        requested_by: current.id,
        requested_at: Utc::now().to_rfc3339(),
    };
    let bytes =
        serde_json::to_vec_pretty(&request).map_err(|error| ApiError::Internal(error.into()))?;
    let temporary = root.join(format!(".request-{request_id}.tmp"));
    let destination = root.join(REQUEST_FILE);
    tokio::fs::write(&temporary, bytes)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let link_result = tokio::fs::hard_link(&temporary, &destination).await;
    let _ = tokio::fs::remove_file(&temporary).await;
    match link_result {
        Ok(()) => Ok(request_id),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            Err(ApiError::Busy("update service"))
        }
        Err(error) => Err(ApiError::Internal(error.into())),
    }
}

async fn load_runner_status(root: &Path) -> ApiResult<RunnerStatus> {
    match tokio::fs::read(root.join(STATUS_FILE)).await {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
            ApiError::Internal(anyhow::anyhow!("invalid updater status: {error}"))
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(RunnerStatus::default()),
        Err(error) => Err(ApiError::Internal(error.into())),
    }
}

async fn directory_exists(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
}

fn configured_root(state: &AppState) -> ApiResult<&Path> {
    state
        .config
        .update_control_root
        .as_deref()
        .ok_or(ApiError::Unavailable("update service"))
}

fn ensure_system_admin(current: &CurrentUser) -> ApiResult<()> {
    if current.system_role != SystemRole::SystemAdmin {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

async fn log_update_request(
    state: &AppState,
    current: &CurrentUser,
    request_id: Uuid,
    action: &str,
    target_version: Option<&str>,
) {
    if let Err(error) = log_event(
        state,
        current.organization_id,
        Some(current.id),
        "system_update_requested",
        "system_update",
        None,
        json!({
            "request_id": request_id,
            "operation": action,
            "target_version": target_version,
        }),
    )
    .await
    {
        tracing::warn!(%error, %request_id, "update request queued but audit logging failed");
    }
}

fn strict_release_tag(value: &str) -> Result<Version, String> {
    let raw = value
        .strip_prefix('v')
        .ok_or_else(|| "update version must use vX.Y.Z".to_string())?;
    let parts: Vec<&str> = raw.split('.').collect();
    if parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty()
                || !part.bytes().all(|byte| byte.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
        })
    {
        return Err("update version must use vX.Y.Z".to_string());
    }
    let numbers = parts
        .iter()
        .map(|part| part.parse::<u64>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "update version is out of range".to_string())?;
    Ok(Version::new(numbers[0], numbers[1], numbers[2]))
}

fn is_active_state(value: &str) -> bool {
    matches!(
        value,
        "checking"
            | "backing_up"
            | "downloading"
            | "building"
            | "deploying"
            | "verifying"
            | "rolling_back"
    )
}

fn default_message(state: &str) -> &'static str {
    match state {
        "queued" => "更新任务已进入主机队列",
        "idle" => "尚未检查更新",
        _ => "更新服务已就绪",
    }
}

#[cfg(test)]
mod tests {
    use super::{is_active_state, strict_release_tag};
    use semver::Version;

    #[test]
    fn release_tags_are_strict_and_shell_safe() {
        assert_eq!(
            strict_release_tag("v1.12.3").unwrap(),
            Version::new(1, 12, 3)
        );
        for invalid in [
            "1.2.3",
            "v1.2",
            "v01.2.3",
            "v1.2.3-rc.1",
            "v1.2.3;reboot",
            "v1.2.3\nnext",
        ] {
            assert!(strict_release_tag(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn only_mutating_runner_phases_block_new_requests() {
        assert!(is_active_state("building"));
        assert!(is_active_state("rolling_back"));
        assert!(!is_active_state("available"));
        assert!(!is_active_state("failed"));
    }
}
