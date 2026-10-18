use crate::{app::AppState, error::ApiResult};
use sqlx::FromRow;
use std::time::Duration;
use tokio::sync::watch;
use uuid::Uuid;

const CLEANUP_BATCH_SIZE: i64 = 16;
const CLEANUP_SCAN_INTERVAL: Duration = Duration::from_secs(30);
const CLEANUP_LEASE_SECONDS: i64 = 5 * 60;
const CLEANUP_JOB_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CONSECUTIVE_FAILURES: i32 = 8;
const STORAGE_JOB_WAIT_SECONDS: i64 = 30;
const RECORD_PROJECT_CLEANUP_FAILURE_SQL: &str = r#"
    UPDATE project_file_cleanup_jobs
    SET attempts = $4,
        last_error = $5,
        next_attempt_at = now() + ($6::double precision * interval '1 second'),
        consecutive_failures = $7,
        terminal_at = CASE WHEN $8 THEN now() ELSE NULL END,
        lease_owner = NULL,
        lease_until = NULL,
        updated_at = now()
    WHERE project_id = $1 AND organization_id = $2 AND lease_owner = $3
"#;

#[derive(FromRow)]
struct CleanupJob {
    project_id: Uuid,
    organization_id: Uuid,
}

#[derive(FromRow)]
struct ClaimedCleanupJob {
    attempts: i32,
    consecutive_failures: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StorageDependencyState {
    Ready,
    Waiting,
    Terminal,
}

fn storage_dependency_state(has_jobs: bool, has_terminal_jobs: bool) -> StorageDependencyState {
    if has_terminal_jobs {
        StorageDependencyState::Terminal
    } else if has_jobs {
        StorageDependencyState::Waiting
    } else {
        StorageDependencyState::Ready
    }
}

fn cleanup_backoff_seconds(attempts_after_failure: i32) -> i64 {
    let exponent = attempts_after_failure.saturating_sub(1).clamp(0, 7) as u32;
    (30_i64.saturating_mul(1_i64 << exponent)).min(60 * 60)
}

fn reaches_terminal_failure_limit(consecutive_failures: i32) -> bool {
    consecutive_failures >= MAX_CONSECUTIVE_FAILURES
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FailureTransition {
    attempts: i32,
    consecutive_failures: i32,
    terminal: bool,
    backoff_seconds: i64,
}

fn failure_transition(attempts: i32, consecutive_failures: i32) -> FailureTransition {
    let attempts = attempts.saturating_add(1);
    let consecutive_failures = consecutive_failures.saturating_add(1);
    FailureTransition {
        attempts,
        consecutive_failures,
        terminal: reaches_terminal_failure_limit(consecutive_failures),
        backoff_seconds: cleanup_backoff_seconds(consecutive_failures),
    }
}

async fn claim_project_file_cleanup(
    state: &AppState,
    organization_id: Uuid,
    project_id: Uuid,
    lease_owner: Uuid,
) -> ApiResult<Option<ClaimedCleanupJob>> {
    sqlx::query_as::<_, ClaimedCleanupJob>(
        r#"
        UPDATE project_file_cleanup_jobs
        SET lease_owner = $3,
            lease_until = now() + ($4::double precision * interval '1 second'),
            updated_at = now()
        WHERE project_id = $1
          AND organization_id = $2
          AND (lease_until IS NULL OR lease_until <= now())
          AND next_attempt_at <= now()
          AND terminal_at IS NULL
        RETURNING attempts, consecutive_failures
        "#,
    )
    .bind(project_id)
    .bind(organization_id)
    .bind(lease_owner)
    .bind(CLEANUP_LEASE_SECONDS as f64)
    .fetch_optional(&state.db)
    .await
    .map_err(Into::into)
}

async fn execute_claimed_project_file_cleanup(
    state: &AppState,
    organization_id: Uuid,
    project_id: Uuid,
    lease_owner: Uuid,
    claimed: &ClaimedCleanupJob,
) -> ApiResult<bool> {
    let (has_storage_jobs, has_terminal_storage_jobs) = sqlx::query_as::<_, (bool, bool)>(
        r#"
        SELECT
          EXISTS (
            SELECT 1 FROM file_storage_jobs
            WHERE project_id = $1 AND organization_id = $2
          ),
          EXISTS (
            SELECT 1 FROM file_storage_jobs
            WHERE project_id = $1 AND organization_id = $2 AND terminal_at IS NOT NULL
          )
        "#,
    )
    .bind(project_id)
    .bind(organization_id)
    .fetch_one(&state.db)
    .await?;
    match storage_dependency_state(has_storage_jobs, has_terminal_storage_jobs) {
        StorageDependencyState::Ready => {}
        dependency => {
            let terminal = dependency == StorageDependencyState::Terminal;
            let updated = sqlx::query(
                r#"
                UPDATE project_file_cleanup_jobs
                SET next_attempt_at = now() + ($4::double precision * interval '1 second'),
                    last_error = CASE WHEN $5 THEN 'terminal file storage job requires manual retry' ELSE last_error END,
                    terminal_at = CASE WHEN $5 THEN now() ELSE NULL END,
                    lease_owner = NULL, lease_until = NULL, updated_at = now()
                WHERE project_id = $1 AND organization_id = $2 AND lease_owner = $3
                "#,
            )
            .bind(project_id)
            .bind(organization_id)
            .bind(lease_owner)
            .bind(STORAGE_JOB_WAIT_SECONDS as f64)
            .bind(terminal)
            .execute(&state.db)
            .await?;
            if terminal && updated.rows_affected() > 0 {
                tracing::error!(%project_id, "project cleanup blocked by terminal file storage job");
            }
            return Ok(true);
        }
    }

    let cleanup_result =
        match crate::file_storage::resolve_storage_path(state, &project_id.to_string()).await {
            Ok(project_dir) => match tokio::fs::remove_dir_all(project_dir).await {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.to_string()),
            },
            Err(error) => Err(error.to_string()),
        };
    match cleanup_result {
        Ok(()) => finish_claimed_cleanup(state, organization_id, project_id, lease_owner).await,
        Err(error) => {
            record_claimed_cleanup_failure(
                state,
                organization_id,
                project_id,
                lease_owner,
                claimed,
                &error,
            )
            .await
        }
    }
}

async fn record_claimed_cleanup_failure(
    state: &AppState,
    organization_id: Uuid,
    project_id: Uuid,
    lease_owner: Uuid,
    claimed: &ClaimedCleanupJob,
    error: &str,
) -> ApiResult<bool> {
    let transition = failure_transition(claimed.attempts, claimed.consecutive_failures);
    let updated = sqlx::query(RECORD_PROJECT_CLEANUP_FAILURE_SQL)
        .bind(project_id)
        .bind(organization_id)
        .bind(lease_owner)
        .bind(transition.attempts)
        .bind(error)
        .bind(transition.backoff_seconds as f64)
        .bind(transition.consecutive_failures)
        .bind(transition.terminal)
        .execute(&state.db)
        .await?;
    if updated.rows_affected() > 0 {
        tracing::warn!(%error, %project_id, attempts = transition.attempts, "project file cleanup failed");
        if transition.terminal {
            tracing::error!(%error, %project_id, attempts = transition.attempts, "project cleanup moved to terminal state");
        }
        Ok(true)
    } else {
        cleanup_job_still_exists(state, organization_id, project_id).await
    }
}

async fn attempt_project_file_cleanup(
    state: &AppState,
    organization_id: Uuid,
    project_id: Uuid,
) -> ApiResult<bool> {
    let lease_owner = Uuid::new_v4();
    let Some(claimed) =
        claim_project_file_cleanup(state, organization_id, project_id, lease_owner).await?
    else {
        return cleanup_job_still_exists(state, organization_id, project_id).await;
    };
    match tokio::time::timeout(
        CLEANUP_JOB_TIMEOUT,
        execute_claimed_project_file_cleanup(
            state,
            organization_id,
            project_id,
            lease_owner,
            &claimed,
        ),
    )
    .await
    {
        Ok(Ok(pending)) => Ok(pending),
        Ok(Err(error)) => {
            record_claimed_cleanup_failure(
                state,
                organization_id,
                project_id,
                lease_owner,
                &claimed,
                &error.to_string(),
            )
            .await
        }
        Err(_) => {
            record_claimed_cleanup_failure(
                state,
                organization_id,
                project_id,
                lease_owner,
                &claimed,
                "project cleanup job timed out after 30 seconds",
            )
            .await
        }
    }
}

async fn finish_claimed_cleanup(
    state: &AppState,
    organization_id: Uuid,
    project_id: Uuid,
    lease_owner: Uuid,
) -> ApiResult<bool> {
    let deleted = sqlx::query(
        r#"
        DELETE FROM project_file_cleanup_jobs
        WHERE project_id = $1 AND organization_id = $2 AND lease_owner = $3
        "#,
    )
    .bind(project_id)
    .bind(organization_id)
    .bind(lease_owner)
    .execute(&state.db)
    .await?;
    if deleted.rows_affected() > 0 {
        Ok(false)
    } else {
        cleanup_job_still_exists(state, organization_id, project_id).await
    }
}

async fn cleanup_job_still_exists(
    state: &AppState,
    organization_id: Uuid,
    project_id: Uuid,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
          SELECT 1 FROM project_file_cleanup_jobs
          WHERE project_id = $1 AND organization_id = $2
        )
        "#,
    )
    .bind(project_id)
    .bind(organization_id)
    .fetch_one(&state.db)
    .await?)
}

async fn run_due_batch(state: &AppState) -> ApiResult<()> {
    let jobs = sqlx::query_as::<_, CleanupJob>(
        r#"
        SELECT project_id, organization_id
        FROM project_file_cleanup_jobs
        WHERE next_attempt_at <= now()
          AND (lease_until IS NULL OR lease_until <= now())
          AND terminal_at IS NULL
        ORDER BY next_attempt_at, updated_at, project_id
        LIMIT $1
        "#,
    )
    .bind(CLEANUP_BATCH_SIZE)
    .fetch_all(&state.db)
    .await?;

    for job in jobs {
        if let Err(error) =
            attempt_project_file_cleanup(state, job.organization_id, job.project_id).await
        {
            tracing::error!(%error, project_id = %job.project_id, "cleanup worker transition failed");
        }
    }
    Ok(())
}

pub async fn run_cleanup_worker(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut interval = tokio::time::interval(CLEANUP_SCAN_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            _ = state.project_cleanup_notify.notified() => {
                if let Err(error) = run_due_batch(&state).await {
                    tracing::error!(%error, "cleanup worker wake scan failed");
                }
            }
            _ = interval.tick() => {
                if let Err(error) = run_due_batch(&state).await {
                    tracing::error!(%error, "cleanup worker scan failed");
                }
            }
        }
    }
    tracing::info!("project file cleanup worker stopped");
}

#[cfg(test)]
mod tests {
    use super::{
        CLEANUP_JOB_TIMEOUT, CLEANUP_LEASE_SECONDS, CLEANUP_SCAN_INTERVAL,
        MAX_CONSECUTIVE_FAILURES, RECORD_PROJECT_CLEANUP_FAILURE_SQL, StorageDependencyState,
        cleanup_backoff_seconds, failure_transition, reaches_terminal_failure_limit,
        storage_dependency_state,
    };

    #[test]
    fn cleanup_backoff_grows_and_caps_at_one_hour() {
        assert_eq!(cleanup_backoff_seconds(1), 30);
        assert_eq!(cleanup_backoff_seconds(2), 60);
        assert_eq!(cleanup_backoff_seconds(4), 240);
        assert_eq!(cleanup_backoff_seconds(20), 3600);
    }

    #[test]
    fn cleanup_lease_has_a_bounded_reclaim_time() {
        assert!(CLEANUP_LEASE_SECONDS > CLEANUP_SCAN_INTERVAL.as_secs() as i64);
        assert!(CLEANUP_LEASE_SECONDS > CLEANUP_JOB_TIMEOUT.as_secs() as i64);
    }

    #[test]
    fn cleanup_failures_stop_at_terminal_limit() {
        assert!(!reaches_terminal_failure_limit(
            MAX_CONSECUTIVE_FAILURES - 1
        ));
        assert!(reaches_terminal_failure_limit(MAX_CONSECUTIVE_FAILURES));
    }

    #[test]
    fn timeout_failure_transition_increments_and_enters_terminal_state() {
        let transition = failure_transition(4, MAX_CONSECUTIVE_FAILURES - 1);

        assert_eq!(transition.attempts, 5);
        assert_eq!(transition.consecutive_failures, MAX_CONSECUTIVE_FAILURES);
        assert!(transition.terminal);
        assert_eq!(transition.backoff_seconds, 3600);
        assert!(RECORD_PROJECT_CLEANUP_FAILURE_SQL.contains("lease_owner = $3"));
        assert!(RECORD_PROJECT_CLEANUP_FAILURE_SQL.contains("last_error = $5"));
    }

    #[test]
    fn project_cleanup_waits_for_storage_jobs_and_stops_for_terminal_dependency() {
        assert_eq!(
            storage_dependency_state(false, false),
            StorageDependencyState::Ready
        );
        assert_eq!(
            storage_dependency_state(true, false),
            StorageDependencyState::Waiting
        );
        assert_eq!(
            storage_dependency_state(true, true),
            StorageDependencyState::Terminal
        );
    }

    #[test]
    fn cleanup_job_schema_contains_cancellation_safe_lease_fields() {
        let migration = include_str!("../migrations/202607150005_access_foundation.sql");

        assert!(migration.contains("lease_owner uuid"));
        assert!(migration.contains("lease_until timestamptz"));
        assert!(migration.contains("next_attempt_at timestamptz"));
        assert!(migration.contains("terminal_at timestamptz"));
        assert!(migration.contains("consecutive_failures integer"));
        assert!(migration.contains("(lease_owner IS NULL) = (lease_until IS NULL)"));
    }
}
