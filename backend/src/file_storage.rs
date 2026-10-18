use crate::{
    access,
    app::AppState,
    audit::log_event_in_transaction,
    auth::{self, CurrentUser},
    error::{ApiError, ApiResult},
    models::SystemRole,
    response::{ApiEnvelope, ok},
};
use axum::{
    Json,
    extract::{Path as AxumPath, State},
};
use serde::Serialize;
use serde_json::json;
use sqlx::FromRow;
use std::{
    path::{Component, Path, PathBuf},
    time::Duration,
};
use tokio::sync::watch;
use uuid::Uuid;

const BATCH_SIZE: i64 = 16;
const SCAN_INTERVAL: Duration = Duration::from_secs(30);
const LEASE_SECONDS: i64 = 5 * 60;
const JOB_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CONSECUTIVE_FAILURES: i32 = 8;
const RECORD_FILE_STORAGE_FAILURE_SQL: &str = r#"
    UPDATE file_storage_jobs
    SET attempts = $4, last_error = $5,
        next_attempt_at = now() + ($6::double precision * interval '1 second'),
        consecutive_failures = $7,
        terminal_at = CASE WHEN $8 THEN now() ELSE NULL END,
        lease_owner = NULL, lease_until = NULL, updated_at = now()
    WHERE id = $1 AND organization_id = $2 AND lease_owner = $3
"#;
const RETRY_FILE_STORAGE_JOB_SQL: &str = r#"
    UPDATE file_storage_jobs
    SET consecutive_failures = 0, terminal_at = NULL,
        next_attempt_at = now(), updated_at = now()
    WHERE id = $1 AND organization_id = $2
      AND terminal_at IS NOT NULL
      AND lease_owner IS NULL
      AND (
        kind = 'delete'
        OR (kind = 'upload_cleanup' AND operation_state = 'cancelled')
      )
    RETURNING id
"#;

#[derive(Serialize)]
pub struct RetryFileStorageJobResponse {
    pub retried: bool,
}

#[derive(FromRow)]
struct JobRef {
    id: Uuid,
    organization_id: Uuid,
}

#[derive(FromRow)]
struct ClaimedJob {
    project_id: Uuid,
    file_asset_id: Option<Uuid>,
    kind: String,
    source_path: String,
    destination_path: Option<String>,
    attempts: i32,
    consecutive_failures: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UploadCleanupDisposition {
    ClearIntentOnly,
    RemoveArtifacts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FailureTransition {
    attempts: i32,
    consecutive_failures: i32,
    terminal: bool,
    backoff_seconds: i64,
}

fn upload_cleanup_disposition(file_asset_exists: bool) -> UploadCleanupDisposition {
    if file_asset_exists {
        UploadCleanupDisposition::ClearIntentOnly
    } else {
        UploadCleanupDisposition::RemoveArtifacts
    }
}

fn backoff_seconds(attempts: i32) -> i64 {
    let exponent = attempts.saturating_sub(1).clamp(0, 7) as u32;
    (30_i64.saturating_mul(1_i64 << exponent)).min(3600)
}

fn reaches_terminal_failure_limit(consecutive_failures: i32) -> bool {
    consecutive_failures >= MAX_CONSECUTIVE_FAILURES
}

fn failure_transition(attempts: i32, consecutive_failures: i32) -> FailureTransition {
    let attempts = attempts.saturating_add(1);
    let consecutive_failures = consecutive_failures.saturating_add(1);
    FailureTransition {
        attempts,
        consecutive_failures,
        terminal: reaches_terminal_failure_limit(consecutive_failures),
        backoff_seconds: backoff_seconds(consecutive_failures),
    }
}

pub(crate) async fn resolve_storage_path(state: &AppState, value: &str) -> ApiResult<PathBuf> {
    let cwd = std::env::current_dir().map_err(|error| ApiError::Internal(error.into()))?;
    let path = resolve_storage_path_with_cwd(
        &state.config.file_root,
        &state.configured_file_root,
        &cwd,
        value,
    )?;
    ensure_no_link_components(&state.config.file_root, &path).await?;
    Ok(path)
}

fn resolve_storage_path_with_cwd(
    root: &Path,
    configured_root: &Path,
    cwd: &Path,
    value: &str,
) -> ApiResult<PathBuf> {
    let path = PathBuf::from(value);
    let candidates = if path.is_absolute() {
        vec![path]
    } else {
        let mut candidates = vec![cwd.join(&path)];
        if let Ok(relative) = path.strip_prefix(configured_root) {
            candidates.push(root.join(relative));
        }
        candidates.push(root.join(&path));
        candidates
    };
    for candidate in candidates {
        if let Some(candidate) = lexical_normalize(&candidate)
            && candidate.starts_with(root)
            && candidate != root
        {
            return Ok(candidate);
        }
    }
    Err(ApiError::BadRequest(
        "file path is outside file root".to_string(),
    ))
}

fn lexical_normalize(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.file_name().is_none() || !normalized.pop() {
                    return None;
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    Some(normalized)
}

async fn ensure_no_link_components(root: &Path, path: &Path) -> ApiResult<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| ApiError::BadRequest("file path is outside file root".to_string()))?;
    let mut current = root.to_path_buf();
    reject_link_metadata(&current).await?;
    for component in relative.components() {
        current.push(component.as_os_str());
        match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) if metadata_is_link_or_reparse(&metadata) => {
                return Err(ApiError::BadRequest(
                    "file path contains a symlink or reparse point".to_string(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(ApiError::Internal(error.into())),
        }
    }
    Ok(())
}

async fn reject_link_metadata(path: &Path) -> ApiResult<()> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    if metadata_is_link_or_reparse(&metadata) {
        return Err(ApiError::BadRequest(
            "file path contains a symlink or reparse point".to_string(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn metadata_is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn metadata_is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    windows_attributes_are_reparse(metadata.file_attributes())
}

#[cfg(windows)]
fn windows_attributes_are_reparse(attributes: u32) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(any(unix, windows)))]
fn metadata_is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

async fn claim_job(
    state: &AppState,
    organization_id: Uuid,
    id: Uuid,
    owner: Uuid,
) -> ApiResult<Option<ClaimedJob>> {
    sqlx::query_as::<_, ClaimedJob>(
        r#"
        UPDATE file_storage_jobs
        SET lease_owner = $3,
            lease_until = now() + ($4::double precision * interval '1 second'),
            updated_at = now()
        WHERE id = $1 AND organization_id = $2
          AND next_attempt_at <= now()
          AND (lease_until IS NULL OR lease_until <= now())
          AND terminal_at IS NULL
        RETURNING project_id, file_asset_id, kind, source_path, destination_path,
                  attempts, consecutive_failures
        "#,
    )
    .bind(id)
    .bind(organization_id)
    .bind(owner)
    .bind(LEASE_SECONDS as f64)
    .fetch_optional(&state.db)
    .await
    .map_err(Into::into)
}

async fn execute_claimed_job(
    state: &AppState,
    organization_id: Uuid,
    job: &ClaimedJob,
) -> ApiResult<()> {
    let source = resolve_storage_path(state, &job.source_path).await?;
    match job.kind.as_str() {
        "delete" => remove_file_if_present(&source).await,
        "upload_cleanup" => {
            let file_asset_id = job.file_asset_id.ok_or_else(|| {
                ApiError::BadRequest("upload cleanup asset is required".to_string())
            })?;
            let destination = resolve_storage_path(
                state,
                job.destination_path.as_deref().ok_or_else(|| {
                    ApiError::BadRequest("upload cleanup destination is required".to_string())
                })?,
            )
            .await?;
            let committed: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                  SELECT 1
                  FROM file_assets file
                  JOIN projects project ON project.id = file.project_id
                  WHERE file.id = $1 AND file.project_id = $2
                    AND file.stored_path = $3 AND project.organization_id = $4
                )
                "#,
            )
            .bind(file_asset_id)
            .bind(job.project_id)
            .bind(&job.destination_path)
            .bind(organization_id)
            .fetch_one(&state.db)
            .await?;
            match upload_cleanup_disposition(committed) {
                UploadCleanupDisposition::ClearIntentOnly => Ok(()),
                UploadCleanupDisposition::RemoveArtifacts => {
                    remove_file_if_present(&source).await?;
                    remove_file_if_present(&destination).await
                }
            }
        }
        _ => Err(ApiError::BadRequest(
            "unsupported file storage job".to_string(),
        )),
    }
}

async fn finish_claimed_job(
    state: &AppState,
    organization_id: Uuid,
    id: Uuid,
    owner: Uuid,
) -> ApiResult<()> {
    let deleted = sqlx::query(
        "DELETE FROM file_storage_jobs WHERE id = $1 AND organization_id = $2 AND lease_owner = $3",
    )
    .bind(id)
    .bind(organization_id)
    .bind(owner)
    .execute(&state.db)
    .await?;
    if deleted.rows_affected() > 0 {
        state.project_cleanup_notify.notify_one();
    }
    Ok(())
}

async fn record_claimed_failure(
    state: &AppState,
    organization_id: Uuid,
    id: Uuid,
    owner: Uuid,
    job: &ClaimedJob,
    error: &str,
) -> ApiResult<()> {
    let transition = failure_transition(job.attempts, job.consecutive_failures);
    let updated = sqlx::query(RECORD_FILE_STORAGE_FAILURE_SQL)
        .bind(id)
        .bind(organization_id)
        .bind(owner)
        .bind(transition.attempts)
        .bind(error)
        .bind(transition.backoff_seconds as f64)
        .bind(transition.consecutive_failures)
        .bind(transition.terminal)
        .execute(&state.db)
        .await?;
    if updated.rows_affected() > 0 {
        tracing::warn!(%error, job_id = %id, attempts = transition.attempts, "file storage job failed");
        if transition.terminal {
            tracing::error!(%error, job_id = %id, attempts = transition.attempts, "file storage job moved to terminal state");
        }
    }
    Ok(())
}

async fn attempt_job(state: &AppState, organization_id: Uuid, id: Uuid) -> ApiResult<()> {
    let owner = Uuid::new_v4();
    let Some(job) = claim_job(state, organization_id, id, owner).await? else {
        return Ok(());
    };
    match tokio::time::timeout(
        JOB_TIMEOUT,
        execute_claimed_job(state, organization_id, &job),
    )
    .await
    {
        Ok(Ok(())) => finish_claimed_job(state, organization_id, id, owner).await,
        Ok(Err(error)) => {
            record_claimed_failure(state, organization_id, id, owner, &job, &error.to_string())
                .await
        }
        Err(_) => {
            record_claimed_failure(
                state,
                organization_id,
                id,
                owner,
                &job,
                "file storage job timed out after 30 seconds",
            )
            .await
        }
    }
}

async fn remove_file_if_present(path: &Path) -> ApiResult<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ApiError::Internal(error.into())),
    }
}

async fn run_batch(state: &AppState) -> ApiResult<()> {
    let jobs = sqlx::query_as::<_, JobRef>(
        r#"SELECT id, organization_id FROM file_storage_jobs
           WHERE next_attempt_at <= now() AND (lease_until IS NULL OR lease_until <= now())
             AND terminal_at IS NULL
           ORDER BY next_attempt_at, updated_at, id LIMIT $1"#,
    )
    .bind(BATCH_SIZE)
    .fetch_all(&state.db)
    .await?;
    for job in jobs {
        if let Err(error) = attempt_job(state, job.organization_id, job.id).await {
            tracing::error!(%error, job_id = %job.id, "file storage job transition failed");
        }
    }
    Ok(())
}

pub async fn run_worker(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut interval = tokio::time::interval(SCAN_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            changed = shutdown.changed() => if changed.is_err() || *shutdown.borrow() { break; },
            _ = state.file_storage_notify.notified() => if let Err(error) = run_batch(&state).await {
                tracing::error!(%error, "file storage worker wake scan failed");
            },
            _ = interval.tick() => if let Err(error) = run_batch(&state).await {
                tracing::error!(%error, "file storage worker scan failed");
            }
        }
    }
}

pub async fn retry_job(
    State(state): State<AppState>,
    current: CurrentUser,
    AxumPath(job_id): AxumPath<Uuid>,
) -> ApiResult<Json<ApiEnvelope<RetryFileStorageJobResponse>>> {
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    if actor.system_role != SystemRole::SystemAdmin {
        return Err(ApiError::Forbidden);
    }
    access::lock_organization_department_tree(&mut transaction, actor.organization_id).await?;
    let project_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT project_id FROM file_storage_jobs WHERE id = $1 AND organization_id = $2",
    )
    .bind(job_id)
    .bind(actor.organization_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let project_id = project_id.ok_or(ApiError::NotFound)?;
    access::lock_project_in_transaction_optional(&mut transaction, &actor, project_id, true)
        .await?;
    let retried: Option<Uuid> = sqlx::query_scalar(RETRY_FILE_STORAGE_JOB_SQL)
        .bind(job_id)
        .bind(actor.organization_id)
        .fetch_optional(&mut *transaction)
        .await?;
    retried.ok_or(ApiError::Busy("file storage job is not retryable"))?;
    sqlx::query(
        r#"
        UPDATE project_file_cleanup_jobs
        SET consecutive_failures = 0, terminal_at = NULL,
            next_attempt_at = now(), updated_at = now()
        WHERE project_id = $1 AND organization_id = $2
        "#,
    )
    .bind(project_id)
    .bind(actor.organization_id)
    .execute(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "file_storage_job.retry",
        "file_storage_job",
        Some(job_id),
        json!({ "project_id": project_id }),
    )
    .await?;
    transaction.commit().await?;
    state.file_storage_notify.notify_one();
    state.project_cleanup_notify.notify_one();
    Ok(ok(RetryFileStorageJobResponse { retried: true }))
}

#[cfg(test)]
mod tests {
    use super::{
        JOB_TIMEOUT, LEASE_SECONDS, MAX_CONSECUTIVE_FAILURES, RECORD_FILE_STORAGE_FAILURE_SQL,
        RETRY_FILE_STORAGE_JOB_SQL, UploadCleanupDisposition, backoff_seconds,
        ensure_no_link_components, failure_transition, reaches_terminal_failure_limit,
        resolve_storage_path_with_cwd, upload_cleanup_disposition,
    };
    use std::path::{Path, PathBuf};
    use uuid::Uuid;

    #[test]
    fn storage_paths_cannot_escape_file_root() {
        let cwd = unique_test_dir("storage-paths");
        let root = cwd.join("files");
        assert!(
            resolve_storage_path_with_cwd(
                &root,
                Path::new("data/files"),
                &cwd,
                root.join("project/file.bin").to_str().unwrap()
            )
            .is_ok()
        );
        assert!(
            resolve_storage_path_with_cwd(
                &root,
                Path::new("data/files"),
                &cwd,
                cwd.join("other/file.bin").to_str().unwrap()
            )
            .is_err()
        );
        assert!(
            resolve_storage_path_with_cwd(
                &root,
                Path::new("data/files"),
                &cwd,
                root.join("../secret").to_str().unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn storage_paths_support_legacy_cwd_and_new_root_relative_values() {
        let cwd = unique_test_dir("legacy-storage-paths");
        let root = cwd.join("data/files");
        let different_cwd = cwd.join("different-cwd");
        assert_eq!(
            resolve_storage_path_with_cwd(
                &root,
                Path::new("data/files"),
                &different_cwd,
                "data/files/project/legacy.bin",
            )
            .unwrap(),
            root.join("project/legacy.bin")
        );
        assert_eq!(
            resolve_storage_path_with_cwd(
                &root,
                Path::new("./data/files"),
                &different_cwd,
                "./data/files/project/legacy.bin",
            )
            .unwrap(),
            root.join("project/legacy.bin")
        );
        assert_eq!(
            resolve_storage_path_with_cwd(&root, Path::new("data/files"), &cwd, "project/new.bin")
                .unwrap(),
            root.join("project/new.bin")
        );
    }

    #[test]
    fn storage_retry_and_lease_are_bounded() {
        assert_eq!(backoff_seconds(1), 30);
        assert_eq!(backoff_seconds(20), 3600);
        assert!(LEASE_SECONDS > JOB_TIMEOUT.as_secs() as i64);
    }

    #[test]
    fn storage_failures_stop_at_terminal_limit() {
        assert!(!reaches_terminal_failure_limit(
            MAX_CONSECUTIVE_FAILURES - 1
        ));
        assert!(reaches_terminal_failure_limit(MAX_CONSECUTIVE_FAILURES));
        assert!(RETRY_FILE_STORAGE_JOB_SQL.contains("terminal_at = NULL"));
        assert!(RETRY_FILE_STORAGE_JOB_SQL.contains("consecutive_failures = 0"));
    }

    #[test]
    fn timeout_failure_transition_increments_and_enters_terminal_state() {
        let transition = failure_transition(12, MAX_CONSECUTIVE_FAILURES - 1);

        assert_eq!(transition.attempts, 13);
        assert_eq!(transition.consecutive_failures, MAX_CONSECUTIVE_FAILURES);
        assert!(transition.terminal);
        assert_eq!(transition.backoff_seconds, 3600);
        assert!(RECORD_FILE_STORAGE_FAILURE_SQL.contains("lease_owner = $3"));
        assert!(RECORD_FILE_STORAGE_FAILURE_SQL.contains("last_error = $5"));
    }

    #[test]
    fn manual_retry_only_accepts_unclaimed_terminal_jobs() {
        assert!(RETRY_FILE_STORAGE_JOB_SQL.contains("terminal_at IS NOT NULL"));
        assert!(RETRY_FILE_STORAGE_JOB_SQL.contains("lease_owner IS NULL"));
        assert!(RETRY_FILE_STORAGE_JOB_SQL.contains("operation_state = 'cancelled'"));
    }

    #[tokio::test]
    async fn nonexistent_target_checks_existing_parent_chain() {
        let root = unique_test_dir("normal-parent");
        let parent = root.join("project");
        tokio::fs::create_dir_all(&parent).await.unwrap();
        ensure_no_link_components(&root, &parent.join("missing.bin"))
            .await
            .unwrap();
        tokio::fs::remove_dir_all(&root).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_parent_is_rejected() {
        use std::os::unix::fs::symlink;

        let root = unique_test_dir("symlink-root");
        let outside = unique_test_dir("symlink-outside");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::create_dir_all(&outside).await.unwrap();
        symlink(&outside, root.join("linked")).unwrap();
        assert!(
            ensure_no_link_components(&root, &root.join("linked/file.bin"))
                .await
                .is_err()
        );
        tokio::fs::remove_dir_all(&root).await.unwrap();
        tokio::fs::remove_dir_all(&outside).await.unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_reparse_attribute_is_rejected() {
        use super::windows_attributes_are_reparse;

        assert!(windows_attributes_are_reparse(0x0400));
        assert!(windows_attributes_are_reparse(0x0410));
        assert!(!windows_attributes_are_reparse(0x0010));
    }

    #[test]
    fn committed_upload_only_clears_intent() {
        assert_eq!(
            upload_cleanup_disposition(true),
            UploadCleanupDisposition::ClearIntentOnly
        );
        assert_eq!(
            upload_cleanup_disposition(false),
            UploadCleanupDisposition::RemoveArtifacts
        );
    }

    #[test]
    fn storage_job_schema_has_owner_guarded_due_leases() {
        let migration = include_str!("../migrations/202607150005_access_foundation.sql");
        assert!(migration.contains("CREATE TABLE file_storage_jobs"));
        assert!(migration.contains("kind IN ('upload_cleanup', 'delete')"));
        assert!(!migration.contains("'finalize'"));
        assert!(migration.contains("lease_owner uuid"));
        assert!(migration.contains("lease_until timestamptz"));
        assert!(migration.contains("idx_file_storage_jobs_due"));
        assert!(migration.contains("idx_file_storage_jobs_project"));
        assert!(migration.contains("terminal_at timestamptz"));
        assert!(migration.contains("consecutive_failures integer"));
    }

    fn unique_test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("crewdock-{label}-{}", Uuid::new_v4()))
    }
}
