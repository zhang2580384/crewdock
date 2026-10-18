use crate::{
    access,
    app::AppState,
    audit::log_event_in_transaction,
    auth::{self, CurrentUser},
    error::{ApiError, ApiResult},
    models::{Department, SystemRole},
    response::{ApiEnvelope, ok},
};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

const APPROVED_REQUEST_DEPARTMENT_REFERENCE_SQL: &str = r#"
    SELECT EXISTS(
      SELECT 1
      FROM registration_requests
      WHERE organization_id = $2 AND status = 'approved'
        AND $1 = ANY(approved_department_ids)
    )
"#;

#[derive(Deserialize)]
pub struct CreateDepartmentRequest {
    pub parent_id: Option<Uuid>,
    pub name: String,
}

#[derive(Deserialize)]
pub struct UpdateDepartmentRequest {
    pub name: String,
}

#[derive(Deserialize)]
pub struct MoveDepartmentRequest {
    pub parent_id: Option<Uuid>,
    pub position: i32,
}

#[derive(Serialize)]
pub struct DeleteResponse {
    pub deleted: bool,
}

fn validate_department_name(name: &str) -> ApiResult<()> {
    if name.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "department name is required".to_string(),
        ));
    }
    Ok(())
}

fn ensure_valid_department_move(
    department_id: Uuid,
    parent_id: Option<Uuid>,
    descendant_ids: &[Uuid],
) -> ApiResult<()> {
    if parent_id == Some(department_id)
        || parent_id.is_some_and(|parent_id| descendant_ids.contains(&parent_id))
    {
        return Err(ApiError::BadRequest(
            "department cannot be moved into itself or a descendant".to_string(),
        ));
    }
    Ok(())
}

fn normalize_move_position(position: i32, sibling_count: i64) -> i32 {
    position.clamp(0, sibling_count.clamp(0, i32::MAX as i64) as i32)
}

fn normalize_reordered_position(position: i32, sibling_count: i64) -> i32 {
    normalize_move_position(position, sibling_count)
}

async fn ensure_department_manager(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    current: &CurrentUser,
    department_id: Uuid,
) -> ApiResult<()> {
    if access::manages_department_in_transaction(transaction, current, department_id).await? {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

async fn lock_department_mutation_context(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_actor: &CurrentUser,
) -> ApiResult<CurrentUser> {
    let actor = auth::lock_current_user_in_transaction(transaction, request_actor).await?;
    access::lock_organization_department_tree(transaction, actor.organization_id).await?;
    Ok(actor)
}

pub async fn list_departments(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<Vec<Department>>>> {
    let rows = sqlx::query_as::<_, Department>(
        r#"
        SELECT id, organization_id, parent_id, name, sort_order
        FROM departments
        WHERE organization_id = $1
        ORDER BY parent_id NULLS FIRST, sort_order ASC, name ASC, id ASC
        "#,
    )
    .bind(current.organization_id)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(rows))
}

pub async fn create_department(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(payload): Json<CreateDepartmentRequest>,
) -> ApiResult<Json<ApiEnvelope<Department>>> {
    let mut transaction = state.db.begin().await?;
    let actor = lock_department_mutation_context(&mut transaction, &current).await?;
    match payload.parent_id {
        Some(parent_id) => ensure_department_manager(&mut transaction, &actor, parent_id).await?,
        None if actor.system_role != SystemRole::SystemAdmin => {
            return Err(ApiError::Forbidden);
        }
        None => {}
    }
    validate_department_name(&payload.name)?;

    let sort_order: i32 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(MAX(sort_order) + 1, 0)
        FROM departments
        WHERE organization_id = $1 AND parent_id IS NOT DISTINCT FROM $2
        "#,
    )
    .bind(actor.organization_id)
    .bind(payload.parent_id)
    .fetch_one(&mut *transaction)
    .await?;
    let row = sqlx::query_as::<_, Department>(
        r#"
        INSERT INTO departments (organization_id, parent_id, name, sort_order)
        VALUES ($1, $2, $3, $4)
        RETURNING id, organization_id, parent_id, name, sort_order
        "#,
    )
    .bind(actor.organization_id)
    .bind(payload.parent_id)
    .bind(payload.name.trim())
    .bind(sort_order)
    .fetch_one(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "department.create",
        "department",
        Some(row.id),
        json!({ "parent_id": row.parent_id, "position": row.sort_order, "name": row.name }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn update_department(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateDepartmentRequest>,
) -> ApiResult<Json<ApiEnvelope<Department>>> {
    let mut transaction = state.db.begin().await?;
    let actor = lock_department_mutation_context(&mut transaction, &current).await?;
    ensure_department_manager(&mut transaction, &actor, id).await?;
    validate_department_name(&payload.name)?;
    let row = sqlx::query_as::<_, Department>(
        r#"
        UPDATE departments
        SET name = $3, updated_at = now()
        WHERE id = $1 AND organization_id = $2
        RETURNING id, organization_id, parent_id, name, sort_order
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .bind(payload.name.trim())
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "department.update",
        "department",
        Some(row.id),
        json!({ "parent_id": row.parent_id, "position": row.sort_order, "name": row.name }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn move_department(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<MoveDepartmentRequest>,
) -> ApiResult<Json<ApiEnvelope<Department>>> {
    let mut transaction = state.db.begin().await?;
    let actor = lock_department_mutation_context(&mut transaction, &current).await?;
    ensure_department_manager(&mut transaction, &actor, id).await?;
    match payload.parent_id {
        Some(parent_id) => ensure_department_manager(&mut transaction, &actor, parent_id).await?,
        None if actor.system_role != SystemRole::SystemAdmin => {
            return Err(ApiError::Forbidden);
        }
        None => {}
    }

    let existing = sqlx::query_as::<_, (Option<Uuid>, i32)>(
        r#"
        SELECT parent_id, sort_order
        FROM departments
        WHERE id = $1 AND organization_id = $2
        FOR UPDATE
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;

    if actor.system_role != SystemRole::SystemAdmin && existing.0 != payload.parent_id {
        let is_assigned_root: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
              SELECT 1 FROM department_admins
              WHERE department_id = $1 AND user_id = $2
            )
            "#,
        )
        .bind(id)
        .bind(actor.id)
        .fetch_one(&mut *transaction)
        .await?;
        if is_assigned_root {
            return Err(ApiError::Forbidden);
        }
    }

    let descendant_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        WITH RECURSIVE descendants AS (
          SELECT id
          FROM departments
          WHERE organization_id = $2 AND parent_id = $1

          UNION

          SELECT department.id
          FROM departments department
          JOIN descendants ON department.parent_id = descendants.id
          WHERE department.organization_id = $2
        )
        SELECT id FROM descendants
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .fetch_all(&mut *transaction)
    .await?;
    ensure_valid_department_move(id, payload.parent_id, &descendant_ids)?;

    let sibling_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM departments
        WHERE organization_id = $1
          AND parent_id IS NOT DISTINCT FROM $2
          AND id <> $3
        "#,
    )
    .bind(actor.organization_id)
    .bind(payload.parent_id)
    .bind(id)
    .fetch_one(&mut *transaction)
    .await?;
    let position = normalize_reordered_position(payload.position, sibling_count);

    sqlx::query(
        r#"
        UPDATE departments SET sort_order = sort_order - 1
        WHERE organization_id = $1
          AND parent_id IS NOT DISTINCT FROM $2
          AND id <> $3
          AND sort_order > $4
        "#,
    )
    .bind(actor.organization_id)
    .bind(existing.0)
    .bind(id)
    .bind(existing.1)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE departments SET sort_order = sort_order + 1
        WHERE organization_id = $1
          AND parent_id IS NOT DISTINCT FROM $2
          AND id <> $3
          AND sort_order >= $4
        "#,
    )
    .bind(actor.organization_id)
    .bind(payload.parent_id)
    .bind(id)
    .bind(position)
    .execute(&mut *transaction)
    .await?;
    let row = sqlx::query_as::<_, Department>(
        r#"
        UPDATE departments
        SET parent_id = $3, sort_order = $4, updated_at = now()
        WHERE id = $1 AND organization_id = $2
        RETURNING id, organization_id, parent_id, name, sort_order
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .bind(payload.parent_id)
    .bind(position)
    .fetch_one(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "department.move",
        "department",
        Some(row.id),
        json!({
            "old_parent_id": existing.0,
            "parent_id": row.parent_id,
            "old_position": existing.1,
            "position": row.sort_order,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn delete_department(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<DeleteResponse>>> {
    let mut transaction = state.db.begin().await?;
    let actor = lock_department_mutation_context(&mut transaction, &current).await?;
    ensure_department_manager(&mut transaction, &actor, id).await?;
    let existing = sqlx::query_as::<_, (Option<Uuid>, i32)>(
        r#"
        SELECT parent_id, sort_order
        FROM departments
        WHERE id = $1 AND organization_id = $2
        FOR UPDATE
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    let used_by_approved_request: bool =
        sqlx::query_scalar(APPROVED_REQUEST_DEPARTMENT_REFERENCE_SQL)
            .bind(id)
            .bind(actor.organization_id)
            .fetch_one(&mut *transaction)
            .await?;
    if used_by_approved_request {
        return Err(ApiError::Conflict(
            "department is assigned to an account awaiting activation".to_string(),
        ));
    }
    let has_references: bool = sqlx::query_scalar(
        r#"
        SELECT
          EXISTS (
            SELECT 1 FROM departments
            WHERE parent_id = $1
          )
          OR EXISTS (
            SELECT 1 FROM users
            WHERE department_id = $1
          )
          OR EXISTS (
            SELECT 1 FROM user_department_memberships
            WHERE department_id = $1
          )
          OR EXISTS (
            SELECT 1 FROM projects
            WHERE department_id = $1
          )
          OR EXISTS (
            SELECT 1 FROM project_department_grants
            WHERE department_id = $1
          )
          OR EXISTS (
            SELECT 1 FROM site_department_grants
            WHERE department_id = $1
          )
          OR EXISTS (
            SELECT 1 FROM markdown_folder_department_grants
            WHERE department_id = $1
          )
          OR EXISTS (
            SELECT 1 FROM markdown_doc_department_grants
            WHERE department_id = $1
          )
          OR EXISTS (
            SELECT 1 FROM department_admins
            WHERE department_id = $1
          )
        "#,
    )
    .bind(id)
    .fetch_one(&mut *transaction)
    .await?;
    if has_references {
        return Err(ApiError::BadRequest("department is not empty".to_string()));
    }

    sqlx::query("DELETE FROM departments WHERE id = $1 AND organization_id = $2")
        .bind(id)
        .bind(actor.organization_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        r#"
        UPDATE departments SET sort_order = sort_order - 1
        WHERE organization_id = $1
          AND parent_id IS NOT DISTINCT FROM $2
          AND sort_order > $3
        "#,
    )
    .bind(actor.organization_id)
    .bind(existing.0)
    .bind(existing.1)
    .execute(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "department.delete",
        "department",
        Some(id),
        json!({ "parent_id": existing.0, "position": existing.1 }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(DeleteResponse { deleted: true }))
}

#[cfg(test)]
mod tests {
    use super::{
        APPROVED_REQUEST_DEPARTMENT_REFERENCE_SQL, ensure_valid_department_move,
        normalize_move_position, normalize_reordered_position, validate_department_name,
    };
    use uuid::Uuid;

    #[test]
    fn department_validation_rejects_blank_name() {
        let error = validate_department_name("  ").unwrap_err();

        assert_eq!(
            error.to_string(),
            "bad request: department name is required"
        );
    }

    #[test]
    fn department_delete_checks_every_approved_onboarding_department() {
        assert!(APPROVED_REQUEST_DEPARTMENT_REFERENCE_SQL.contains("status = 'approved'"));
        assert!(
            APPROVED_REQUEST_DEPARTMENT_REFERENCE_SQL.contains("$1 = ANY(approved_department_ids)")
        );
    }

    #[test]
    fn department_move_rejects_itself_as_parent() {
        let department_id = Uuid::new_v4();
        let error =
            ensure_valid_department_move(department_id, Some(department_id), &[]).unwrap_err();

        assert_eq!(
            error.to_string(),
            "bad request: department cannot be moved into itself or a descendant"
        );
    }

    #[test]
    fn department_move_rejects_descendant_as_parent() {
        let department_id = Uuid::new_v4();
        let descendant_id = Uuid::new_v4();
        let error =
            ensure_valid_department_move(department_id, Some(descendant_id), &[descendant_id])
                .unwrap_err();

        assert_eq!(
            error.to_string(),
            "bad request: department cannot be moved into itself or a descendant"
        );
    }

    #[test]
    fn department_move_position_is_bounded_by_sibling_count() {
        assert_eq!(normalize_move_position(-2, 3), 0);
        assert_eq!(normalize_move_position(2, 3), 2);
        assert_eq!(normalize_move_position(99, 3), 3);
    }

    #[test]
    fn department_same_parent_forward_move_uses_final_index() {
        assert_eq!(normalize_reordered_position(2, 2), 2);
    }

    #[test]
    fn department_same_parent_backward_move_uses_final_index() {
        assert_eq!(normalize_reordered_position(0, 2), 0);
    }
}
