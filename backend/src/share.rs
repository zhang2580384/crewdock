use crate::{
    access,
    app::AppState,
    audit::{log_event, log_event_in_transaction},
    auth::{self, CurrentUser, hash_token},
    error::{ApiError, ApiResult},
    file_storage::resolve_storage_path,
    models::{FileAsset, MarkdownDoc, Project, ProjectLink, ProjectRole, Ticket},
    public_url::base_url_from_request,
    response::{ApiEnvelope, ok},
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

pub(crate) const TARGET_PROJECT: &str = "project";
pub(crate) const TARGET_PROJECT_LINK: &str = "project_link";
pub(crate) const TARGET_MARKDOWN_DOC: &str = "markdown_doc";
pub(crate) const TARGET_FILE_ASSET: &str = "file_asset";
pub(crate) const TARGET_TICKET: &str = "ticket";

#[derive(Deserialize)]
pub struct CreateShareLinkRequest {
    pub target_type: String,
    pub target_id: Uuid,
    pub title: String,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct ShareLinkRow {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub target_type: String,
    pub target_id: Uuid,
    pub title: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct CreateShareLinkResponse {
    pub share: ShareLinkRow,
    pub url: String,
}

#[derive(Serialize)]
pub struct DeleteShareLinkResponse {
    pub deleted: bool,
}

#[derive(Deserialize)]
pub struct ResolveShareLinkQuery {
    pub token: String,
}

#[derive(Serialize)]
pub struct ResolveShareLinkResponse {
    pub share: ShareLinkRow,
    pub target: serde_json::Value,
}

#[derive(sqlx::FromRow)]
struct ShareTokenRow {
    id: Uuid,
    organization_id: Uuid,
    target_type: String,
    target_id: Uuid,
    title: String,
    expires_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct ShareWriteTargetRow {
    target_type: String,
    target_id: Uuid,
}

fn validate_target_type(target_type: &str) -> ApiResult<()> {
    if matches!(
        target_type,
        TARGET_PROJECT
            | TARGET_PROJECT_LINK
            | TARGET_MARKDOWN_DOC
            | TARGET_FILE_ASSET
            | TARGET_TICKET
    ) {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "unsupported share target type".to_string(),
        ))
    }
}

fn share_is_active(expires_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    expires_at.is_none_or(|expires_at| expires_at > now)
}

fn ensure_share_manager(role: ProjectRole) -> ApiResult<()> {
    if !role.can_manage() {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

fn locked_project_share_target(target_id: Uuid, project_id: Uuid) -> ApiResult<Uuid> {
    if target_id == project_id {
        Ok(target_id)
    } else {
        Err(ApiError::NotFound)
    }
}

async fn target_project_id_with_executor<'e, E>(
    executor: E,
    organization_id: Uuid,
    target_type: &str,
    target_id: Uuid,
) -> ApiResult<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let project_id = match target_type {
        "project" => {
            sqlx::query_scalar("SELECT id FROM projects WHERE id = $1 AND organization_id = $2")
                .bind(target_id)
                .bind(organization_id)
                .fetch_optional(executor)
                .await?
        }
        "project_link" => {
            sqlx::query_scalar(
                r#"
                SELECT l.project_id FROM project_links l
                JOIN projects p ON p.id = l.project_id
                WHERE l.id = $1 AND p.organization_id = $2
                "#,
            )
            .bind(target_id)
            .bind(organization_id)
            .fetch_optional(executor)
            .await?
        }
        "markdown_doc" => {
            sqlx::query_scalar(
                r#"
                SELECT d.project_id FROM markdown_docs d
                JOIN projects p ON p.id = d.project_id
                WHERE d.id = $1 AND p.organization_id = $2
                "#,
            )
            .bind(target_id)
            .bind(organization_id)
            .fetch_optional(executor)
            .await?
        }
        "file_asset" => {
            sqlx::query_scalar(
                r#"
                SELECT f.project_id FROM file_assets f
                JOIN projects p ON p.id = f.project_id
                WHERE f.id = $1 AND p.organization_id = $2
                "#,
            )
            .bind(target_id)
            .bind(organization_id)
            .fetch_optional(executor)
            .await?
        }
        "ticket" => {
            sqlx::query_scalar(
                r#"
                SELECT t.project_id FROM tickets t
                JOIN projects p ON p.id = t.project_id
                WHERE t.id = $1 AND p.organization_id = $2
                "#,
            )
            .bind(target_id)
            .bind(organization_id)
            .fetch_optional(executor)
            .await?
        }
        _ => {
            return Err(ApiError::BadRequest(
                "unsupported share target type".to_string(),
            ));
        }
    };
    project_id.ok_or(ApiError::NotFound)
}

async fn lock_share_target_in_transaction(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    organization_id: Uuid,
    project_id: Uuid,
    target_type: &str,
    target_id: Uuid,
) -> ApiResult<()> {
    let target = match target_type {
        "project" => Some(locked_project_share_target(target_id, project_id)?),
        "project_link" => {
            sqlx::query_scalar(
                r#"
                SELECT link.id
                FROM project_links link
                JOIN projects project ON project.id = link.project_id
                WHERE link.id = $1 AND link.project_id = $2 AND project.organization_id = $3
                FOR SHARE OF link
                "#,
            )
            .bind(target_id)
            .bind(project_id)
            .bind(organization_id)
            .fetch_optional(&mut **transaction)
            .await?
        }
        "markdown_doc" => {
            sqlx::query_scalar(
                r#"
                SELECT doc.id
                FROM markdown_docs doc
                JOIN projects project ON project.id = doc.project_id
                WHERE doc.id = $1 AND doc.project_id = $2 AND project.organization_id = $3
                FOR SHARE OF doc
                "#,
            )
            .bind(target_id)
            .bind(project_id)
            .bind(organization_id)
            .fetch_optional(&mut **transaction)
            .await?
        }
        "file_asset" => {
            sqlx::query_scalar(
                r#"
                SELECT file.id
                FROM file_assets file
                JOIN projects project ON project.id = file.project_id
                WHERE file.id = $1 AND file.project_id = $2 AND project.organization_id = $3
                FOR SHARE OF file
                "#,
            )
            .bind(target_id)
            .bind(project_id)
            .bind(organization_id)
            .fetch_optional(&mut **transaction)
            .await?
        }
        "ticket" => {
            sqlx::query_scalar(
                r#"
                SELECT ticket.id
                FROM tickets ticket
                JOIN projects project ON project.id = ticket.project_id
                WHERE ticket.id = $1 AND ticket.project_id = $2 AND project.organization_id = $3
                FOR SHARE OF ticket
                "#,
            )
            .bind(target_id)
            .bind(project_id)
            .bind(organization_id)
            .fetch_optional(&mut **transaction)
            .await?
        }
        _ => {
            return Err(ApiError::BadRequest(
                "unsupported share target type".to_string(),
            ));
        }
    };
    target.ok_or(ApiError::NotFound)?;
    Ok(())
}

const LIST_SHARE_LINKS_SQL: &str = r#"
    SELECT listed.id, listed.organization_id, listed.target_type, listed.target_id,
           listed.title, listed.expires_at, listed.created_at
    FROM share_links listed
    WHERE listed.organization_id = $1
      AND (
        (listed.target_type = 'project' AND EXISTS (
            SELECT 1 FROM projects project
            WHERE project.id = listed.target_id AND project.organization_id = $1
              AND project.archived_at IS NULL AND project.id = ANY($2)
        ))
        OR (listed.target_type = 'project_link' AND EXISTS (
            SELECT 1 FROM project_links link
            JOIN projects project ON project.id = link.project_id
            WHERE link.id = listed.target_id AND project.organization_id = $1
              AND project.archived_at IS NULL AND link.project_id = ANY($2)
        ))
        OR (listed.target_type = 'markdown_doc' AND EXISTS (
            SELECT 1 FROM markdown_docs doc
            JOIN projects project ON project.id = doc.project_id
            WHERE doc.id = listed.target_id AND project.organization_id = $1
              AND project.archived_at IS NULL AND doc.project_id = ANY($2)
        ))
        OR (listed.target_type = 'file_asset' AND EXISTS (
            SELECT 1 FROM file_assets file
            JOIN projects project ON project.id = file.project_id
            WHERE file.id = listed.target_id AND project.organization_id = $1
              AND project.archived_at IS NULL AND file.project_id = ANY($2)
        ))
        OR (listed.target_type = 'ticket' AND EXISTS (
            SELECT 1 FROM tickets ticket
            JOIN projects project ON project.id = ticket.project_id
            WHERE ticket.id = listed.target_id AND project.organization_id = $1
              AND project.archived_at IS NULL AND ticket.project_id = ANY($2)
        ))
      )
    ORDER BY listed.created_at DESC
    LIMIT 100
"#;

pub async fn list_share_links(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<Vec<ShareLinkRow>>>> {
    let project_ids = access::manageable_project_ids(&state, &current).await?;
    let rows = sqlx::query_as::<_, ShareLinkRow>(LIST_SHARE_LINKS_SQL)
        .bind(current.organization_id)
        .bind(&project_ids)
        .fetch_all(&state.db)
        .await?;
    Ok(ok(rows))
}

pub async fn create_share_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    current: CurrentUser,
    Json(payload): Json<CreateShareLinkRequest>,
) -> ApiResult<Json<ApiEnvelope<CreateShareLinkResponse>>> {
    let public_base_url = base_url_from_request(&headers, &state.config.public_base_url);
    validate_target_type(&payload.target_type)?;
    if payload.title.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "share link title is required".to_string(),
        ));
    }
    if !share_is_active(payload.expires_at, Utc::now()) {
        return Err(ApiError::BadRequest(
            "share link expiration must be in the future".to_string(),
        ));
    }
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    access::lock_organization_department_tree(&mut transaction, actor.organization_id).await?;
    let project_id = target_project_id_with_executor(
        &mut *transaction,
        actor.organization_id,
        &payload.target_type,
        payload.target_id,
    )
    .await?;
    access::lock_project_in_transaction(&mut transaction, &actor, project_id, false).await?;
    let project_access =
        access::project_access_in_transaction(&mut transaction, &actor, project_id).await?;
    ensure_share_manager(project_access.role)?;
    lock_share_target_in_transaction(
        &mut transaction,
        actor.organization_id,
        project_id,
        &payload.target_type,
        payload.target_id,
    )
    .await?;
    let raw_token = Uuid::new_v4().to_string() + "." + &Uuid::new_v4().to_string();
    let token_hash = hash_token(&raw_token, &state.config.session_secret);
    let share = sqlx::query_as::<_, ShareLinkRow>(
        r#"
        INSERT INTO share_links
            (organization_id, target_type, target_id, title, token_hash, created_by, expires_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, organization_id, target_type, target_id, title, expires_at, created_at
        "#,
    )
    .bind(actor.organization_id)
    .bind(payload.target_type)
    .bind(payload.target_id)
    .bind(payload.title)
    .bind(token_hash)
    .bind(actor.id)
    .bind(payload.expires_at)
    .fetch_one(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "share_link.create",
        "share_link",
        Some(share.id),
        json!({ "target_type": share.target_type, "target_id": share.target_id }),
    )
    .await?;
    transaction.commit().await?;
    let path = if share.target_type == "file_asset" {
        format!("/api/share-links/{}/resolve", share.id)
    } else {
        format!("/shared/{}", share.id)
    };
    let url = format!("{public_base_url}{path}?token={raw_token}");
    Ok(ok(CreateShareLinkResponse { share, url }))
}

pub async fn delete_share_link(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<DeleteShareLinkResponse>>> {
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    access::lock_organization_department_tree(&mut transaction, actor.organization_id).await?;
    let candidate = sqlx::query_as::<_, ShareWriteTargetRow>(
        r#"
        SELECT target_type, target_id
        FROM share_links
        WHERE id = $1 AND organization_id = $2
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    let project_id = target_project_id_with_executor(
        &mut *transaction,
        actor.organization_id,
        &candidate.target_type,
        candidate.target_id,
    )
    .await?;
    access::lock_project_in_transaction(&mut transaction, &actor, project_id, false).await?;
    let project_access =
        access::project_access_in_transaction(&mut transaction, &actor, project_id).await?;
    ensure_share_manager(project_access.role)?;
    let locked = sqlx::query_as::<_, ShareWriteTargetRow>(
        r#"
        SELECT target_type, target_id
        FROM share_links
        WHERE id = $1 AND organization_id = $2
        FOR UPDATE
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    if locked.target_type != candidate.target_type || locked.target_id != candidate.target_id {
        return Err(ApiError::NotFound);
    }
    let deleted: Option<Uuid> = sqlx::query_scalar(
        r#"
        DELETE FROM share_links
        WHERE id = $1
          AND organization_id = $2
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .fetch_optional(&mut *transaction)
    .await?;
    deleted.ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "share_link.delete",
        "share_link",
        Some(id),
        json!({}),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(DeleteShareLinkResponse { deleted: true }))
}

pub async fn resolve_share_link(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<ResolveShareLinkQuery>,
) -> ApiResult<Response> {
    let token_hash = hash_token(&query.token, &state.config.session_secret);
    let row = sqlx::query_as::<_, ShareTokenRow>(
        r#"
        SELECT id, organization_id, target_type, target_id, title, expires_at, created_at
        FROM share_links
        WHERE id = $1 AND token_hash = $2
        "#,
    )
    .bind(id)
    .bind(token_hash)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    if !share_is_active(row.expires_at, Utc::now()) {
        return Err(ApiError::NotFound);
    }
    if row.target_type == "file_asset" {
        return resolve_file_target(&state, &row).await;
    }
    let target = resolve_target(&state, &row).await?;
    let share = ShareLinkRow {
        id: row.id,
        organization_id: row.organization_id,
        target_type: row.target_type,
        target_id: row.target_id,
        title: row.title,
        expires_at: row.expires_at,
        created_at: row.created_at,
    };
    log_event(
        &state,
        share.organization_id,
        None,
        "share_link.resolve",
        "share_link",
        Some(share.id),
        json!({ "target_type": share.target_type, "target_id": share.target_id }),
    )
    .await?;
    Ok(ok(ResolveShareLinkResponse { share, target }).into_response())
}

async fn resolve_file_target(state: &AppState, share: &ShareTokenRow) -> ApiResult<Response> {
    let file = sqlx::query_as::<_, FileAsset>(
        r#"
        SELECT f.id, f.project_id, f.filename, f.stored_path, f.content_type,
               f.size_bytes, f.uploaded_by, f.created_at
        FROM file_assets f
        JOIN projects p ON p.id = f.project_id
        WHERE f.id = $1 AND p.organization_id = $2 AND p.archived_at IS NULL
        "#,
    )
    .bind(share.target_id)
    .bind(share.organization_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    let stored_path = resolve_storage_path(state, &file.stored_path).await?;
    let bytes = match tokio::fs::read(stored_path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::NotFound);
        }
        Err(error) => return Err(ApiError::Internal(error.into())),
    };
    log_event(
        &state,
        share.organization_id,
        None,
        "share_link.file_download",
        "share_link",
        Some(share.id),
        json!({ "target_id": share.target_id }),
    )
    .await?;
    crate::files::download_response(&file.filename, file.content_type.as_deref(), bytes)
}

async fn resolve_target(state: &AppState, share: &ShareTokenRow) -> ApiResult<serde_json::Value> {
    let value = match share.target_type.as_str() {
        "project" => {
            let row = sqlx::query_as::<_, Project>(
                r#"
                SELECT id, organization_id, department_id, name, slug, description, metadata,
                       created_at, archived_at
                FROM projects
                WHERE id = $1 AND organization_id = $2 AND archived_at IS NULL
                "#,
            )
            .bind(share.target_id)
            .bind(share.organization_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or(ApiError::NotFound)?;
            serde_json::to_value(row)
        }
        "project_link" => {
            let row = sqlx::query_as::<_, ProjectLink>(
                r#"
                SELECT l.id, l.project_id, l.title, l.url, l.description, l.tags, l.created_at
                FROM project_links l
                JOIN projects p ON p.id = l.project_id
                WHERE l.id = $1 AND p.organization_id = $2 AND p.archived_at IS NULL
                "#,
            )
            .bind(share.target_id)
            .bind(share.organization_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or(ApiError::NotFound)?;
            serde_json::to_value(row)
        }
        "markdown_doc" => {
            let row = sqlx::query_as::<_, MarkdownDoc>(
                r#"
                SELECT d.id, d.project_id, d.folder_id, d.title, d.body_markdown, d.tags,
                       d.sort_order, d.created_by, d.updated_at
                FROM markdown_docs d
                JOIN projects p ON p.id = d.project_id
                WHERE d.id = $1 AND p.organization_id = $2 AND p.archived_at IS NULL
                "#,
            )
            .bind(share.target_id)
            .bind(share.organization_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or(ApiError::NotFound)?;
            serde_json::to_value(row)
        }
        "ticket" => {
            let row = sqlx::query_as::<_, Ticket>(
                r#"
                SELECT t.id, t.project_id, t.title, t.description_markdown, t.status,
                       t.assignee_id, t.created_by, t.updated_at
                FROM tickets t
                JOIN projects p ON p.id = t.project_id
                WHERE t.id = $1 AND p.organization_id = $2 AND p.archived_at IS NULL
                "#,
            )
            .bind(share.target_id)
            .bind(share.organization_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or(ApiError::NotFound)?;
            serde_json::to_value(row)
        }
        _ => return Err(ApiError::NotFound),
    };
    value.map_err(|error| ApiError::Internal(error.into()))
}

#[cfg(test)]
mod tests {
    use super::{
        LIST_SHARE_LINKS_SQL, TARGET_FILE_ASSET, TARGET_MARKDOWN_DOC, TARGET_PROJECT,
        TARGET_PROJECT_LINK, TARGET_TICKET, ensure_share_manager, locked_project_share_target,
        share_is_active, validate_target_type,
    };
    use crate::models::ProjectRole;
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    #[test]
    fn share_target_validation_rejects_unsupported_type() {
        let error = validate_target_type("vault_item").unwrap_err();

        assert_eq!(
            error.to_string(),
            "bad request: unsupported share target type"
        );
    }

    #[test]
    fn share_expiry_rejects_expired_link() {
        let now = Utc::now();

        assert!(!share_is_active(Some(now - Duration::seconds(1)), now));
        assert!(share_is_active(Some(now + Duration::seconds(1)), now));
        assert!(share_is_active(None, now));
    }

    #[test]
    fn share_target_literals_cover_all_transactional_delete_paths() {
        for target_type in [
            TARGET_PROJECT,
            TARGET_PROJECT_LINK,
            TARGET_MARKDOWN_DOC,
            TARGET_FILE_ASSET,
            TARGET_TICKET,
        ] {
            validate_target_type(target_type).unwrap();
        }
    }

    #[test]
    fn project_share_target_must_match_the_locked_project() {
        let project_id = Uuid::new_v4();

        assert_eq!(
            locked_project_share_target(project_id, project_id).unwrap(),
            project_id
        );
        assert!(locked_project_share_target(Uuid::new_v4(), project_id).is_err());
    }

    #[test]
    fn share_cleanup_lookup_has_matching_composite_index() {
        let migration = include_str!("../migrations/202607150005_access_foundation.sql");
        assert!(migration.contains("CREATE INDEX idx_share_links_target"));
        assert!(migration.contains("ON share_links(organization_id, target_type, target_id)"));
    }

    #[test]
    fn share_list_uses_effective_visible_active_projects() {
        assert!(LIST_SHARE_LINKS_SQL.contains("project.archived_at IS NULL"));
        assert!(LIST_SHARE_LINKS_SQL.contains("ANY($2)"));
        assert!(!LIST_SHARE_LINKS_SQL.contains("created_by"));
        assert!(!LIST_SHARE_LINKS_SQL.contains("project_members"));
    }

    #[test]
    fn share_mutations_require_project_manage_permission() {
        ensure_share_manager(ProjectRole::Administrator).unwrap();
        assert!(ensure_share_manager(ProjectRole::Editor).is_err());
        assert!(ensure_share_manager(ProjectRole::Viewer).is_err());
    }
}
