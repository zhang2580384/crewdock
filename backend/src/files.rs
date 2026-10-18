use crate::{
    access,
    app::AppState,
    audit::log_event_in_transaction,
    auth::{self, CurrentUser},
    error::{ApiError, ApiResult},
    file_storage::resolve_storage_path,
    models::FileAsset,
    response::{ApiEnvelope, ok},
    share,
};
use axum::{
    Json,
    body::Body,
    extract::{Multipart, Path, State},
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
use tokio::{fs::OpenOptions, io::AsyncWriteExt};
use uuid::Uuid;

const UPLOAD_IO_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const STAGING_CLEANUP_DELAY_SECONDS: i64 = 30 * 60;
const LIST_FILES_SQL: &str = r#"
    SELECT file.id, file.project_id, file.filename, file.stored_path,
           file.content_type, file.size_bytes, file.uploaded_by, file.created_at
    FROM file_assets file
    JOIN projects project ON project.id = file.project_id
    WHERE file.project_id = $1 AND project.organization_id = $2
      AND project.archived_at IS NULL
    ORDER BY file.created_at DESC
"#;
const INSERT_DELETE_FILE_JOB_SQL: &str = r#"
    INSERT INTO file_storage_jobs
        (id, organization_id, project_id, kind, source_path, next_attempt_at)
    VALUES ($1, $2, $3, 'delete', $4, now())
"#;

#[derive(Serialize)]
pub struct DeleteFileResponse {
    pub deleted: bool,
}

#[derive(Deserialize)]
pub struct UpdateFileRequest {
    pub filename: String,
}

fn upload_storage_values(project_id: Uuid, file_id: Uuid, filename: &str) -> (String, String) {
    (
        format!("{project_id}/.{file_id}-{filename}.uploading"),
        format!("{project_id}/{file_id}-{filename}"),
    )
}

pub async fn list_files(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<Vec<FileAsset>>>> {
    access::project_access(&state, &current, project_id).await?;
    let rows = sqlx::query_as::<_, FileAsset>(LIST_FILES_SQL)
        .bind(project_id)
        .bind(current.organization_id)
        .fetch_all(&state.db)
        .await?;
    Ok(ok(rows))
}

pub async fn upload_file(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
    mut multipart: Multipart,
) -> ApiResult<Json<ApiEnvelope<FileAsset>>> {
    let field = multipart
        .next_field()
        .await
        .map_err(|err| ApiError::BadRequest(err.to_string()))?
        .ok_or_else(|| ApiError::BadRequest("file field is required".to_string()))?;
    let filename = field
        .file_name()
        .map(safe_filename)
        .unwrap_or_else(|| "upload.bin".to_string());
    let content_type = field.content_type().map(ToOwned::to_owned);
    let id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let operation_owner = Uuid::new_v4();
    let project_dir = state.config.file_root.join(project_id.to_string());
    let (temporary_value, stored_value) = upload_storage_values(project_id, id, &filename);

    let mut transaction = state.db.begin().await?;
    let (actor, project_access) =
        access::lock_active_project_write_context(&mut transaction, &current, project_id).await?;
    if !project_access.role.can_write() {
        return Err(ApiError::Forbidden);
    }
    sqlx::query(
        r#"
        INSERT INTO file_storage_jobs
            (id, organization_id, project_id, file_asset_id, kind,
             operation_owner, operation_state, source_path, destination_path, next_attempt_at)
        VALUES
            ($1, $2, $3, $4, 'upload_cleanup', $5, 'receiving', $6, $7,
             now() + ($8::double precision * interval '1 second'))
        "#,
    )
    .bind(job_id)
    .bind(actor.organization_id)
    .bind(project_id)
    .bind(id)
    .bind(operation_owner)
    .bind(&temporary_value)
    .bind(&stored_value)
    .bind(STAGING_CLEANUP_DELAY_SECONDS as f64)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    let io_result = tokio::time::timeout(UPLOAD_IO_TIMEOUT, async {
        match tokio::fs::create_dir(&project_dir).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(ApiError::Internal(error.into())),
        }
        let temporary_path = resolve_storage_path(&state, &temporary_value).await?;
        let stored_path = resolve_storage_path(&state, &stored_value).await?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .await
            .map_err(|error| ApiError::Internal(error.into()))?;
        let mut stream = field.into_stream();
        let mut size = 0_i64;
        while let Some(chunk) = stream
            .try_next()
            .await
            .map_err(|error| ApiError::BadRequest(error.to_string()))?
        {
            size = size.saturating_add(chunk.len() as i64);
            file.write_all(&chunk)
                .await
                .map_err(|error| ApiError::Internal(error.into()))?;
        }
        file.flush()
            .await
            .map_err(|error| ApiError::Internal(error.into()))?;
        drop(file);
        resolve_storage_path(&state, &temporary_value).await?;
        match tokio::fs::symlink_metadata(&stored_path).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(ApiError::Internal(error.into())),
            Ok(_) => {
                return Err(ApiError::BadRequest(
                    "file destination already exists".to_string(),
                ));
            }
        }
        tokio::fs::rename(&temporary_path, &stored_path)
            .await
            .map_err(|error| ApiError::Internal(error.into()))?;
        resolve_storage_path(&state, &stored_value).await?;
        Ok::<i64, ApiError>(size)
    })
    .await;
    let size = match io_result {
        Ok(Ok(size)) => size,
        Ok(Err(error)) => {
            schedule_storage_job_now(&state, current.organization_id, job_id).await;
            return Err(error);
        }
        Err(_) => {
            schedule_storage_job_now(&state, current.organization_id, job_id).await;
            return Err(ApiError::BadRequest(
                "file upload I/O timed out".to_string(),
            ));
        }
    };

    let completion: ApiResult<FileAsset> = async {
        let mut transaction = state.db.begin().await?;
        let (actor, project_access) =
            access::lock_active_project_write_context(&mut transaction, &current, project_id)
                .await?;
        if !project_access.role.can_write() {
            return Err(ApiError::Forbidden);
        }
        let intent_locked: Option<Uuid> = sqlx::query_scalar(
            r#"
            SELECT id
            FROM file_storage_jobs
            WHERE id = $1 AND organization_id = $2 AND project_id = $3
              AND file_asset_id = $4 AND kind = 'upload_cleanup'
              AND operation_owner = $5 AND operation_state = 'receiving'
              AND lease_owner IS NULL AND terminal_at IS NULL
            FOR UPDATE
            "#,
        )
        .bind(job_id)
        .bind(actor.organization_id)
        .bind(project_id)
        .bind(id)
        .bind(operation_owner)
        .fetch_optional(&mut *transaction)
        .await?;
        if intent_locked.is_none() {
            return Err(ApiError::Busy("upload cleanup intent"));
        }
        let row = sqlx::query_as::<_, FileAsset>(
            r#"
            INSERT INTO file_assets
                (id, project_id, filename, stored_path, content_type, size_bytes, uploaded_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING id, project_id, filename, stored_path, content_type, size_bytes,
                      uploaded_by, created_at
            "#,
        )
        .bind(id)
        .bind(project_id)
        .bind(&filename)
        .bind(&stored_value)
        .bind(content_type)
        .bind(size)
        .bind(actor.id)
        .fetch_one(&mut *transaction)
        .await?;
        log_event_in_transaction(
            &mut transaction,
            actor.organization_id,
            Some(actor.id),
            "file.upload",
            "file_asset",
            Some(row.id),
            json!({ "project_id": project_id, "filename": row.filename, "size": row.size_bytes }),
        )
        .await?;
        let intent_deleted = sqlx::query(
            r#"
            DELETE FROM file_storage_jobs
            WHERE id = $1 AND organization_id = $2 AND operation_owner = $3
              AND operation_state = 'receiving'
            "#,
        )
        .bind(job_id)
        .bind(actor.organization_id)
        .bind(operation_owner)
        .execute(&mut *transaction)
        .await?;
        if intent_deleted.rows_affected() != 1 {
            return Err(ApiError::Busy("upload cleanup intent"));
        }
        transaction.commit().await?;
        Ok(row)
    }
    .await;
    match completion {
        Ok(row) => Ok(ok(row)),
        Err(error) => {
            schedule_storage_job_now(&state, current.organization_id, job_id).await;
            Err(error)
        }
    }
}

async fn schedule_storage_job_now(state: &AppState, organization_id: Uuid, job_id: Uuid) {
    if let Err(error) = sqlx::query(
        r#"
        UPDATE file_storage_jobs
        SET next_attempt_at = now(), updated_at = now()
        WHERE id = $1 AND organization_id = $2
        "#,
    )
    .bind(job_id)
    .bind(organization_id)
    .execute(&state.db)
    .await
    {
        tracing::error!(%error, %job_id, "failed to advance file storage cleanup job");
    }
    state.file_storage_notify.notify_one();
}

pub async fn download_file(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Response> {
    let row = sqlx::query_as::<_, FileAsset>(
        r#"
        SELECT f.id, f.project_id, f.filename, f.stored_path, f.content_type,
               f.size_bytes, f.uploaded_by, f.created_at
        FROM file_assets f
        JOIN projects p ON p.id = f.project_id
        WHERE f.id = $1 AND f.project_id = $2 AND p.organization_id = $3
          AND p.archived_at IS NULL
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(current.organization_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    access::project_access(&state, &current, project_id).await?;

    let stored_path = resolve_storage_path(&state, &row.stored_path).await?;
    let bytes = match tokio::fs::read(stored_path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::NotFound);
        }
        Err(error) => return Err(ApiError::Internal(error.into())),
    };
    download_response(&row.filename, row.content_type.as_deref(), bytes)
}

pub async fn update_file(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<UpdateFileRequest>,
) -> ApiResult<Json<ApiEnvelope<FileAsset>>> {
    let filename = safe_filename(payload.filename.trim());
    if filename.is_empty() {
        return Err(ApiError::BadRequest("filename is required".to_string()));
    }
    let mut transaction = state.db.begin().await?;
    let (actor, project_access) =
        access::lock_active_project_write_context(&mut transaction, &current, project_id).await?;
    if !project_access.role.can_write() {
        return Err(ApiError::Forbidden);
    }
    let row = sqlx::query_as::<_, FileAsset>(
        r#"
        UPDATE file_assets
        SET filename = $3
        WHERE id = $1 AND project_id = $2
        RETURNING id, project_id, filename, stored_path, content_type, size_bytes, uploaded_by, created_at
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(filename)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "file.update",
        "file_asset",
        Some(row.id),
        json!({ "project_id": project_id, "filename": row.filename }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn delete_file(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<ApiEnvelope<DeleteFileResponse>>> {
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    access::lock_organization_department_tree(&mut transaction, actor.organization_id).await?;
    access::lock_project_in_transaction(&mut transaction, &actor, project_id, false).await?;
    let project_access =
        access::project_access_in_transaction(&mut transaction, &actor, project_id).await?;
    if !project_access.role.can_write() {
        return Err(ApiError::Forbidden);
    }
    let row = sqlx::query_as::<_, FileAsset>(
        r#"
        SELECT id, project_id, filename, stored_path, content_type,
               size_bytes, uploaded_by, created_at
        FROM file_assets
        WHERE id = $1 AND project_id = $2
        FOR UPDATE
        "#,
    )
    .bind(id)
    .bind(project_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    sqlx::query(
        "DELETE FROM share_links WHERE organization_id = $1 AND target_type = $2 AND target_id = $3",
    )
    .bind(actor.organization_id)
    .bind(share::TARGET_FILE_ASSET)
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    let deleted: Option<Uuid> = sqlx::query_scalar(
        "DELETE FROM file_assets WHERE id = $1 AND project_id = $2 RETURNING id",
    )
    .bind(id)
    .bind(project_id)
    .fetch_optional(&mut *transaction)
    .await?;
    deleted.ok_or(ApiError::NotFound)?;
    sqlx::query(INSERT_DELETE_FILE_JOB_SQL)
        .bind(Uuid::new_v4())
        .bind(actor.organization_id)
        .bind(project_id)
        .bind(&row.stored_path)
        .execute(&mut *transaction)
        .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "file.delete",
        "file_asset",
        Some(id),
        json!({ "project_id": project_id, "filename": row.filename }),
    )
    .await?;
    transaction.commit().await?;
    state.file_storage_notify.notify_one();
    Ok(ok(DeleteFileResponse { deleted: true }))
}

fn safe_filename(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '.' | '-' | '_' | ' ') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn content_disposition(
    filename: &str,
) -> Result<HeaderValue, axum::http::header::InvalidHeaderValue> {
    let filename = filename
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let filename = if filename.is_empty() {
        "download.bin".to_string()
    } else {
        filename
    };
    HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
}

fn content_type_header(content_type: Option<&str>) -> HeaderValue {
    content_type
        .and_then(|value| HeaderValue::from_str(value).ok())
        .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream"))
}

pub(crate) fn download_response(
    filename: &str,
    content_type: Option<&str>,
    bytes: Vec<u8>,
) -> ApiResult<Response> {
    let disposition =
        content_disposition(filename).map_err(|error| ApiError::Internal(error.into()))?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type_header(content_type))
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        .body(Body::from(bytes))
        .map_err(|error| ApiError::Internal(error.into()))
}

#[cfg(test)]
mod tests {
    use super::{
        INSERT_DELETE_FILE_JOB_SQL, LIST_FILES_SQL, STAGING_CLEANUP_DELAY_SECONDS,
        UPLOAD_IO_TIMEOUT, content_disposition, content_type_header, upload_storage_values,
    };
    use uuid::Uuid;

    #[test]
    fn content_disposition_uses_attachment_and_sanitized_filename() {
        let value = content_disposition("report \"final\".txt").unwrap();

        assert_eq!(
            value.to_str().unwrap(),
            "attachment; filename=\"report__final_.txt\""
        );
    }

    #[test]
    fn content_type_falls_back_for_invalid_stored_value() {
        let value = content_type_header(Some("invalid\nvalue"));

        assert_eq!(value.to_str().unwrap(), "application/octet-stream");
    }

    #[test]
    fn upload_cleanup_intent_cannot_expire_before_bounded_stream() {
        assert!(STAGING_CLEANUP_DELAY_SECONDS > UPLOAD_IO_TIMEOUT.as_secs() as i64);
    }

    #[test]
    fn new_upload_paths_are_file_root_relative() {
        let project_id = Uuid::new_v4();
        let file_id = Uuid::new_v4();
        let (staging, stored) = upload_storage_values(project_id, file_id, "report.txt");
        assert_eq!(
            staging,
            format!("{project_id}/.{file_id}-report.txt.uploading")
        );
        assert_eq!(stored, format!("{project_id}/{file_id}-report.txt"));
        assert!(!std::path::Path::new(&stored).is_absolute());
    }

    #[test]
    fn file_list_requires_an_active_visible_project() {
        assert!(LIST_FILES_SQL.contains("project.archived_at IS NULL"));
        assert!(!LIST_FILES_SQL.contains("project_members"));
    }

    #[test]
    fn file_delete_persists_worker_outbox_with_locked_stored_path() {
        assert!(INSERT_DELETE_FILE_JOB_SQL.contains("kind, source_path"));
        assert!(INSERT_DELETE_FILE_JOB_SQL.contains("'delete', $4, now()"));
    }
}
