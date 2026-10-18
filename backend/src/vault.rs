use crate::{
    access,
    app::AppState,
    audit::{log_event, log_event_in_transaction},
    auth::{self, CurrentUser},
    error::{ApiError, ApiResult},
    models::VaultItemListRow,
    response::{ApiEnvelope, ok},
};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct CreateVaultItemRequest {
    pub project_id: Uuid,
    pub title: String,
    pub kind: String,
    pub username: Option<String>,
    pub url: Option<String>,
    pub secret: String,
    pub notes: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
pub struct UpdateVaultItemRequest {
    pub title: String,
    pub kind: String,
    pub username: Option<String>,
    pub url: Option<String>,
    pub secret: Option<String>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Serialize)]
pub struct VaultSecretResponse {
    pub id: Uuid,
    pub secret: String,
}

#[derive(Serialize)]
pub struct DeleteVaultItemResponse {
    pub deleted: bool,
}

const LIST_VAULT_SQL: &str = r#"
    SELECT vault.id, vault.project_id, vault.title, vault.kind, vault.username,
           vault.url, vault.notes, vault.tags, vault.created_at
    FROM vault_items vault
    JOIN projects project ON project.id = vault.project_id
    WHERE project.organization_id = $1 AND project.archived_at IS NULL
      AND vault.project_id = ANY($2)
    ORDER BY vault.created_at DESC
"#;

#[derive(sqlx::FromRow)]
struct VaultSecretRow {
    id: Uuid,
    project_id: Uuid,
    secret_encrypted: Vec<u8>,
}

fn validate_vault_input(title: &str, kind: &str) -> ApiResult<()> {
    if title.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "vault item title is required".to_string(),
        ));
    }
    if kind.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "vault item kind is required".to_string(),
        ));
    }
    Ok(())
}

pub async fn list_vault(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<Vec<VaultItemListRow>>>> {
    let project_ids = access::visible_project_ids(&state, &current).await?;
    let rows = sqlx::query_as::<_, VaultItemListRow>(LIST_VAULT_SQL)
        .bind(current.organization_id)
        .bind(&project_ids)
        .fetch_all(&state.db)
        .await?;
    Ok(ok(rows))
}

pub async fn create_vault_item(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(payload): Json<CreateVaultItemRequest>,
) -> ApiResult<Json<ApiEnvelope<VaultItemListRow>>> {
    let mut transaction = state.db.begin().await?;
    let (actor, project_access) =
        access::lock_active_project_write_context(&mut transaction, &current, payload.project_id)
            .await?;
    if !project_access.role.can_write() {
        return Err(ApiError::Forbidden);
    }
    validate_vault_input(&payload.title, &payload.kind)?;
    let encrypted = state
        .crypto
        .encrypt(payload.secret.as_bytes())
        .map_err(ApiError::Internal)?;
    let row = sqlx::query_as::<_, VaultItemListRow>(
        r#"
        INSERT INTO vault_items
            (project_id, title, kind, username, url, secret_encrypted, notes, tags, created_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id, project_id, title, kind, username, url, notes, tags, created_at
        "#,
    )
    .bind(payload.project_id)
    .bind(payload.title)
    .bind(payload.kind)
    .bind(payload.username)
    .bind(payload.url)
    .bind(encrypted)
    .bind(payload.notes)
    .bind(payload.tags)
    .bind(actor.id)
    .fetch_one(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "vault.create",
        "vault_item",
        Some(row.id),
        json!({ "project_id": row.project_id, "title": row.title }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn update_vault_item(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateVaultItemRequest>,
) -> ApiResult<Json<ApiEnvelope<VaultItemListRow>>> {
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    access::lock_organization_department_tree(&mut transaction, actor.organization_id).await?;
    let project_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT v.project_id FROM vault_items v JOIN projects p ON p.id = v.project_id WHERE v.id = $1 AND p.organization_id = $2",
    )
    .bind(id)
    .bind(actor.organization_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let project_id = project_id.ok_or(ApiError::NotFound)?;
    access::lock_project_in_transaction(&mut transaction, &actor, project_id, false).await?;
    let project_access =
        access::project_access_in_transaction(&mut transaction, &actor, project_id).await?;
    if !project_access.role.can_write() {
        return Err(ApiError::Forbidden);
    }
    validate_vault_input(&payload.title, &payload.kind)?;
    let secret_updated = payload.secret.is_some();
    let encrypted = payload
        .secret
        .map(|secret| state.crypto.encrypt(secret.as_bytes()))
        .transpose()
        .map_err(ApiError::Internal)?;
    let row = sqlx::query_as::<_, VaultItemListRow>(
        r#"
        UPDATE vault_items
        SET title = $2, kind = $3, username = $4, url = $5,
            secret_encrypted = COALESCE($6, secret_encrypted), notes = $7, tags = $8
        WHERE id = $1 AND project_id = $9
        RETURNING id, project_id, title, kind, username, url, notes, tags, created_at
        "#,
    )
    .bind(id)
    .bind(payload.title)
    .bind(payload.kind)
    .bind(payload.username)
    .bind(payload.url)
    .bind(encrypted)
    .bind(payload.notes)
    .bind(payload.tags)
    .bind(project_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "vault.update",
        "vault_item",
        Some(row.id),
        json!({ "project_id": project_id, "secret_updated": secret_updated }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn delete_vault_item(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<DeleteVaultItemResponse>>> {
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    access::lock_organization_department_tree(&mut transaction, actor.organization_id).await?;
    let project_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT v.project_id FROM vault_items v JOIN projects p ON p.id = v.project_id WHERE v.id = $1 AND p.organization_id = $2",
    )
    .bind(id)
    .bind(actor.organization_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let project_id = project_id.ok_or(ApiError::NotFound)?;
    access::lock_project_in_transaction(&mut transaction, &actor, project_id, false).await?;
    let project_access =
        access::project_access_in_transaction(&mut transaction, &actor, project_id).await?;
    if !project_access.role.can_write() {
        return Err(ApiError::Forbidden);
    }
    let deleted: Option<Uuid> = sqlx::query_scalar(
        "DELETE FROM vault_items WHERE id = $1 AND project_id = $2 RETURNING id",
    )
    .bind(id)
    .bind(project_id)
    .fetch_optional(&mut *transaction)
    .await?;
    deleted.ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "vault.delete",
        "vault_item",
        Some(id),
        json!({ "project_id": project_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(DeleteVaultItemResponse { deleted: true }))
}

pub async fn reveal_secret(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<VaultSecretResponse>>> {
    let row = sqlx::query_as::<_, VaultSecretRow>(
        r#"
        SELECT v.id, v.project_id, v.secret_encrypted
        FROM vault_items v
        JOIN projects p ON p.id = v.project_id
        WHERE v.id = $1 AND p.organization_id = $2 AND p.archived_at IS NULL
        "#,
    )
    .bind(id)
    .bind(current.organization_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    access::project_access(&state, &current, row.project_id).await?;
    let secret = state
        .crypto
        .decrypt(&row.secret_encrypted)
        .map_err(ApiError::Internal)?;
    log_event(
        &state,
        current.organization_id,
        Some(current.id),
        "vault.reveal",
        "vault_item",
        Some(row.id),
        json!({ "project_id": row.project_id }),
    )
    .await?;
    Ok(ok(VaultSecretResponse {
        id: row.id,
        secret: String::from_utf8_lossy(&secret).to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::{LIST_VAULT_SQL, validate_vault_input};

    #[test]
    fn vault_validation_rejects_blank_kind() {
        let error = validate_vault_input("Credential", " ").unwrap_err();

        assert_eq!(
            error.to_string(),
            "bad request: vault item kind is required"
        );
    }

    #[test]
    fn vault_list_uses_effective_visible_active_projects() {
        assert!(LIST_VAULT_SQL.contains("project.archived_at IS NULL"));
        assert!(LIST_VAULT_SQL.contains("vault.project_id = ANY($2)"));
        assert!(!LIST_VAULT_SQL.contains("project_members"));
    }
}
