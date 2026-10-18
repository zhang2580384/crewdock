use crate::{
    access,
    app::AppState,
    audit::log_event_in_transaction,
    auth::{self, CurrentUser, clear_session_cache, hash_password, validate_password},
    error::{ApiError, ApiResult},
    models::{ProjectRole, ProjectUserGrant, SystemRole, User},
    projects,
    response::{ApiEnvelope, ok},
};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Executor, FromRow, Postgres};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

const USER_COLUMNS: &str = r#"
    id, organization_id, department_id, email, display_name, role, system_role,
    is_active, totp_enabled, created_at
"#;
const LAST_DEPARTMENT_ADMIN_SQL: &str = r#"
    SELECT admin.department_id
    FROM department_admins admin
    WHERE admin.user_id = $1
      AND NOT (admin.department_id = ANY($2))
      AND NOT EXISTS (
        SELECT 1
        FROM department_admins other
        JOIN users other_user ON other_user.id = other.user_id
        WHERE other.department_id = admin.department_id
          AND other.user_id <> $1
          AND other_user.organization_id = $3
          AND other_user.is_active = TRUE
      )
    LIMIT 1
"#;

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub email: String,
    pub display_name: String,
    pub password: String,
    pub system_role: Option<SystemRole>,
    pub department_id: Option<Uuid>,
    pub department_ids: Option<Vec<Uuid>>,
    pub primary_department_id: Option<Uuid>,
}

#[derive(Deserialize)]
pub struct UpdateUserRequest {
    pub display_name: Option<String>,
    pub system_role: Option<SystemRole>,
    pub department_id: Option<Option<Uuid>>,
    pub department_ids: Option<Vec<Uuid>>,
    pub primary_department_id: Option<Option<Uuid>>,
    pub is_active: Option<bool>,
}

#[derive(Deserialize)]
pub struct ResetPasswordRequest {
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateManagedDepartmentsRequest {
    pub department_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct ManagedDepartmentsView {
    pub department_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct AccountCapabilities {
    pub can_edit_profile: bool,
    pub can_change_system_role: bool,
    pub can_change_department: bool,
    pub can_change_active: bool,
    pub can_reset_password: bool,
}

#[derive(Debug, Serialize)]
pub struct AccountView {
    #[serde(flatten)]
    pub user: User,
    pub department_ids: Vec<Uuid>,
    pub capabilities: AccountCapabilities,
}

#[derive(Debug, Serialize)]
pub struct AccountManagementCapabilities {
    pub can_create_users: bool,
    pub can_assign_system_role: bool,
    pub can_create_without_department: bool,
    pub manageable_department_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct AccountManagementView {
    pub users: Vec<AccountView>,
    pub capabilities: AccountManagementCapabilities,
}

#[derive(Debug, Serialize, FromRow)]
pub struct AccountProjectGrant {
    pub project_id: Uuid,
    pub project_name: String,
    pub project_role: ProjectRole,
}

fn account_view(user: User, department_ids: Vec<Uuid>, actor: &CurrentUser) -> AccountView {
    let system_admin = actor.system_role == SystemRole::SystemAdmin;
    let is_self = user.id == actor.id;
    let can_manage_target = system_admin || user.system_role == SystemRole::Member;
    AccountView {
        user,
        department_ids,
        capabilities: AccountCapabilities {
            can_edit_profile: can_manage_target,
            can_change_system_role: system_admin && !is_self,
            can_change_department: can_manage_target,
            can_change_active: system_admin && !is_self,
            can_reset_password: can_manage_target,
        },
    }
}

#[derive(FromRow)]
struct UserDepartmentRow {
    user_id: Uuid,
    department_id: Uuid,
}

async fn department_membership_map(
    state: &AppState,
    organization_id: Uuid,
    user_ids: &[Uuid],
) -> ApiResult<HashMap<Uuid, Vec<Uuid>>> {
    if user_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query_as::<_, UserDepartmentRow>(
        r#"
        SELECT user_id, department_id
        FROM user_department_memberships
        WHERE organization_id = $1 AND user_id = ANY($2)
        ORDER BY is_primary DESC, created_at, department_id
        "#,
    )
    .bind(organization_id)
    .bind(user_ids)
    .fetch_all(&state.db)
    .await?;
    let mut memberships = HashMap::<Uuid, Vec<Uuid>>::new();
    for row in rows {
        memberships
            .entry(row.user_id)
            .or_default()
            .push(row.department_id);
    }
    Ok(memberships)
}

async fn department_ids_in_transaction(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    organization_id: Uuid,
    user_id: Uuid,
) -> ApiResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT department_id
        FROM user_department_memberships
        WHERE organization_id = $1 AND user_id = $2
        ORDER BY is_primary DESC, created_at, department_id
        "#,
    )
    .bind(organization_id)
    .bind(user_id)
    .fetch_all(&mut **transaction)
    .await?)
}

fn normalize_department_selection(
    department_ids: Vec<Uuid>,
    primary_department_id: Option<Uuid>,
) -> ApiResult<(Vec<Uuid>, Option<Uuid>)> {
    let mut seen = HashSet::new();
    let department_ids = department_ids
        .into_iter()
        .filter(|department_id| seen.insert(*department_id))
        .collect::<Vec<_>>();
    if department_ids.is_empty() {
        if primary_department_id.is_some() {
            return Err(ApiError::BadRequest(
                "primary department must be included in departments".to_string(),
            ));
        }
        return Ok((department_ids, None));
    }
    let primary_department_id = primary_department_id
        .ok_or_else(|| ApiError::BadRequest("primary department is required".to_string()))?;
    if !department_ids.contains(&primary_department_id) {
        return Err(ApiError::BadRequest(
            "primary department must be included in departments".to_string(),
        ));
    }
    Ok((department_ids, Some(primary_department_id)))
}

async fn sync_department_memberships(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    organization_id: Uuid,
    user_id: Uuid,
    department_ids: &[Uuid],
    primary_department_id: Option<Uuid>,
) -> ApiResult<()> {
    sqlx::query("DELETE FROM user_department_memberships WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut **transaction)
        .await?;
    if !department_ids.is_empty() {
        sqlx::query(
            r#"
            INSERT INTO user_department_memberships (
              user_id, department_id, organization_id, is_primary
            )
            SELECT $1, department_id, $2, department_id = $3
            FROM unnest($4::uuid[]) AS department_id
            "#,
        )
        .bind(user_id)
        .bind(organization_id)
        .bind(primary_department_id)
        .bind(department_ids)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn managed_department_ids_with_executor<'e, E>(
    executor: E,
    actor: &CurrentUser,
) -> ApiResult<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    if actor.system_role == SystemRole::SystemAdmin {
        return Ok(sqlx::query_scalar(
            "SELECT id FROM departments WHERE organization_id = $1 ORDER BY id",
        )
        .bind(actor.organization_id)
        .fetch_all(executor)
        .await?);
    }

    Ok(sqlx::query_scalar(
        r#"
        WITH RECURSIVE managed_departments AS (
          SELECT department.id
          FROM departments department
          JOIN department_admins admin ON admin.department_id = department.id
          WHERE admin.user_id = $1 AND department.organization_id = $2

          UNION

          SELECT child.id
          FROM departments child
          JOIN managed_departments managed ON child.parent_id = managed.id
          WHERE child.organization_id = $2
        )
        SELECT id FROM managed_departments ORDER BY id
        "#,
    )
    .bind(actor.id)
    .bind(actor.organization_id)
    .fetch_all(executor)
    .await?)
}

fn ensure_account_manager(actor: &CurrentUser, managed_department_ids: &[Uuid]) -> ApiResult<()> {
    if actor.system_role != SystemRole::SystemAdmin && managed_department_ids.is_empty() {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

fn account_is_in_scope(
    actor: &CurrentUser,
    managed_department_ids: &[Uuid],
    department_ids: &[Uuid],
) -> bool {
    actor.system_role == SystemRole::SystemAdmin
        || department_ids
            .iter()
            .any(|department_id| managed_department_ids.contains(department_id))
}

fn ensure_scoped_create_fields(system_role: &SystemRole, department_ids: &[Uuid]) -> ApiResult<()> {
    if *system_role != SystemRole::Member {
        return Err(ApiError::Forbidden);
    }
    if department_ids.is_empty() {
        return Err(ApiError::BadRequest(
            "department is required for scoped account creation".to_string(),
        ));
    }
    Ok(())
}

fn ensure_scoped_update_fields(payload: &UpdateUserRequest) -> ApiResult<()> {
    if payload.system_role.is_some() || payload.is_active.is_some() {
        return Err(ApiError::Forbidden);
    }
    if payload.department_id == Some(None)
        || payload.primary_department_id == Some(None)
        || payload.department_ids.as_ref().is_some_and(Vec::is_empty)
    {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

fn ensure_scoped_target_is_member(
    actor_system_role: &SystemRole,
    target_system_role: &SystemRole,
) -> ApiResult<()> {
    if *actor_system_role != SystemRole::SystemAdmin && *target_system_role != SystemRole::Member {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

async fn ensure_department_allowed(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    actor: &CurrentUser,
    department_id: Uuid,
) -> ApiResult<()> {
    if access::manages_department_in_transaction(transaction, actor, department_id).await? {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

async fn ensure_departments_allowed(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    actor: &CurrentUser,
    department_ids: &[Uuid],
) -> ApiResult<()> {
    if department_ids.is_empty() {
        return Ok(());
    }
    ensure_departments_exist(transaction, actor.organization_id, department_ids).await?;
    for department_id in department_ids {
        ensure_department_allowed(transaction, actor, *department_id).await?;
    }
    Ok(())
}

async fn ensure_departments_exist(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    organization_id: Uuid,
    department_ids: &[Uuid],
) -> ApiResult<()> {
    if department_ids.is_empty() {
        return Ok(());
    }
    let existing_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM departments WHERE organization_id = $1 AND id = ANY($2)",
    )
    .bind(organization_id)
    .bind(department_ids)
    .fetch_one(&mut **transaction)
    .await?;
    if existing_count != department_ids.len() as i64 {
        return Err(ApiError::BadRequest(
            "one or more departments are invalid".to_string(),
        ));
    }
    Ok(())
}

async fn ensure_department_changes_allowed(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    actor: &CurrentUser,
    existing_department_ids: &[Uuid],
    next_department_ids: &[Uuid],
    existing_primary_department_id: Option<Uuid>,
    next_primary_department_id: Option<Uuid>,
) -> ApiResult<()> {
    ensure_departments_exist(transaction, actor.organization_id, next_department_ids).await?;
    if actor.system_role == SystemRole::SystemAdmin {
        return Ok(());
    }
    for department_id in existing_department_ids
        .iter()
        .chain(next_department_ids.iter())
        .filter(|department_id| {
            existing_department_ids.contains(department_id)
                != next_department_ids.contains(department_id)
        })
    {
        ensure_department_allowed(transaction, actor, *department_id).await?;
    }
    if existing_primary_department_id != next_primary_department_id {
        if let Some(department_id) = next_primary_department_id {
            ensure_department_allowed(transaction, actor, department_id).await?;
        }
    }
    Ok(())
}

async fn lock_account_write_context(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    request_actor: &CurrentUser,
) -> ApiResult<(CurrentUser, Vec<Uuid>)> {
    let actor = auth::lock_current_user_in_transaction(transaction, request_actor).await?;
    access::lock_organization_department_tree(transaction, actor.organization_id).await?;
    let managed_department_ids =
        managed_department_ids_with_executor(&mut **transaction, &actor).await?;
    ensure_account_manager(&actor, &managed_department_ids)?;
    Ok((actor, managed_department_ids))
}

async fn ensure_account_visible(
    state: &AppState,
    actor: &CurrentUser,
    user_id: Uuid,
) -> ApiResult<()> {
    let managed_department_ids = managed_department_ids_with_executor(&state.db, actor).await?;
    ensure_account_manager(actor, &managed_department_ids)?;
    let user = sqlx::query_as::<_, User>(&format!(
        "SELECT {USER_COLUMNS} FROM users WHERE id = $1 AND organization_id = $2"
    ))
    .bind(user_id)
    .bind(actor.organization_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    let mut memberships =
        department_membership_map(state, actor.organization_id, &[user.id]).await?;
    let department_ids = memberships.remove(&user.id).unwrap_or_default();
    if !account_is_in_scope(actor, &managed_department_ids, &department_ids) {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

pub async fn list_users(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<AccountManagementView>>> {
    let managed_department_ids = managed_department_ids_with_executor(&state.db, &current).await?;
    ensure_account_manager(&current, &managed_department_ids)?;
    let rows = if current.system_role == SystemRole::SystemAdmin {
        sqlx::query_as::<_, User>(&format!(
            "SELECT {USER_COLUMNS} FROM users WHERE organization_id = $1 ORDER BY created_at DESC, email ASC"
        ))
        .bind(current.organization_id)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as::<_, User>(&format!(
            "SELECT {USER_COLUMNS} FROM users WHERE organization_id = $1 AND EXISTS (SELECT 1 FROM user_department_memberships membership WHERE membership.user_id = users.id AND membership.organization_id = $1 AND membership.department_id = ANY($2)) ORDER BY created_at DESC, email ASC"
        ))
        .bind(current.organization_id)
        .bind(&managed_department_ids)
        .fetch_all(&state.db)
        .await?
    };
    let user_ids = rows.iter().map(|user| user.id).collect::<Vec<_>>();
    let mut memberships =
        department_membership_map(&state, current.organization_id, &user_ids).await?;
    let system_admin = current.system_role == SystemRole::SystemAdmin;
    Ok(ok(AccountManagementView {
        users: rows
            .into_iter()
            .map(|user| {
                let department_ids = memberships.remove(&user.id).unwrap_or_default();
                account_view(user, department_ids, &current)
            })
            .collect(),
        capabilities: AccountManagementCapabilities {
            can_create_users: true,
            can_assign_system_role: system_admin,
            can_create_without_department: system_admin,
            manageable_department_ids: managed_department_ids,
        },
    }))
}

pub async fn create_user(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(payload): Json<CreateUserRequest>,
) -> ApiResult<Json<ApiEnvelope<AccountView>>> {
    let email = clean_email(&payload.email)?;
    let display_name = required_text(&payload.display_name, "display_name")?;
    let password_hash = hash_password(&validate_password(&payload.password)?)?;
    let system_role = payload.system_role.unwrap_or(SystemRole::Member);
    let primary_department_id = payload.primary_department_id.or(payload.department_id);
    let mut department_ids = payload.department_ids.unwrap_or_default();
    if department_ids.is_empty() {
        if let Some(department_id) = primary_department_id {
            department_ids.push(department_id);
        }
    }
    let (department_ids, primary_department_id) =
        normalize_department_selection(department_ids, primary_department_id)?;

    let mut transaction = state.db.begin().await?;
    let (actor, _) = lock_account_write_context(&mut transaction, &current).await?;
    if actor.system_role != SystemRole::SystemAdmin {
        ensure_scoped_create_fields(&system_role, &department_ids)?;
    }
    ensure_departments_allowed(&mut transaction, &actor, &department_ids).await?;
    auth::lock_email_identity_in_transaction(&mut transaction, actor.organization_id, &email)
        .await?;
    ensure_email_available(&mut transaction, &email).await?;

    let user = sqlx::query_as::<_, User>(&format!(
        r#"
        INSERT INTO users (
          organization_id, department_id, email, display_name, password_hash, system_role
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING {USER_COLUMNS}
        "#
    ))
    .bind(actor.organization_id)
    .bind(primary_department_id)
    .bind(&email)
    .bind(&display_name)
    .bind(password_hash)
    .bind(system_role)
    .fetch_one(&mut *transaction)
    .await
    .map_err(auth::map_email_identity_error)?;
    sync_department_memberships(
        &mut transaction,
        actor.organization_id,
        user.id,
        &department_ids,
        primary_department_id,
    )
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "user.create",
        "user",
        Some(user.id),
        json!({
            "email": user.email,
            "system_role": user.system_role,
            "department_id": user.department_id,
            "department_ids": department_ids,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(account_view(user, department_ids, &actor)))
}

pub async fn update_user(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateUserRequest>,
) -> ApiResult<Json<ApiEnvelope<AccountView>>> {
    let mut transaction = state.db.begin().await?;
    let (actor, managed_department_ids) =
        lock_account_write_context(&mut transaction, &current).await?;
    let existing =
        auth::lock_account_target_for_update_in_transaction(&mut transaction, &actor, id).await?;
    let existing_department_ids =
        department_ids_in_transaction(&mut transaction, actor.organization_id, id).await?;
    if !account_is_in_scope(&actor, &managed_department_ids, &existing_department_ids) {
        return Err(ApiError::NotFound);
    }
    ensure_scoped_target_is_member(&actor.system_role, &existing.system_role)?;
    if actor.system_role != SystemRole::SystemAdmin {
        ensure_scoped_update_fields(&payload)?;
    }

    let display_name = match payload.display_name {
        Some(value) => required_text(&value, "display_name")?,
        None => existing.display_name.clone(),
    };
    let system_role = payload
        .system_role
        .unwrap_or_else(|| existing.system_role.clone());
    let departments_changed = payload.department_ids.is_some()
        || payload.primary_department_id.is_some()
        || payload.department_id.is_some();
    let (department_ids, primary_department_id) = if departments_changed {
        let department_ids = payload
            .department_ids
            .clone()
            .unwrap_or_else(|| existing_department_ids.clone());
        let primary_department_id = payload
            .primary_department_id
            .unwrap_or_else(|| payload.department_id.unwrap_or(existing.department_id));
        normalize_department_selection(department_ids, primary_department_id)?
    } else {
        (existing_department_ids.clone(), existing.department_id)
    };
    let is_active = payload.is_active.unwrap_or(existing.is_active);

    if existing.system_role == SystemRole::SystemAdmin
        && (system_role != SystemRole::SystemAdmin || !is_active)
    {
        let other_active_system_admins: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM users
            WHERE organization_id = $1 AND id <> $2
              AND system_role = 'system_admin' AND is_active = TRUE
            "#,
        )
        .bind(actor.organization_id)
        .bind(id)
        .fetch_one(&mut *transaction)
        .await?;
        if other_active_system_admins == 0 {
            return Err(ApiError::BadRequest(
                "cannot remove or deactivate the last system administrator".to_string(),
            ));
        }
    }
    let cleared_managed_department_ids = clear_managed_departments_if_required(
        &mut transaction,
        actor.organization_id,
        id,
        !is_active || system_role == SystemRole::SystemAdmin,
    )
    .await?;

    ensure_department_changes_allowed(
        &mut transaction,
        &actor,
        &existing_department_ids,
        &department_ids,
        existing.department_id,
        primary_department_id,
    )
    .await?;
    if department_ids.is_empty() && actor.system_role != SystemRole::SystemAdmin {
        return Err(ApiError::Forbidden);
    }
    if id == actor.id {
        if system_role != SystemRole::SystemAdmin {
            return Err(ApiError::BadRequest(
                "cannot remove your own system administrator role".to_string(),
            ));
        }
        if !is_active {
            return Err(ApiError::BadRequest(
                "cannot deactivate your own account".to_string(),
            ));
        }
    }

    let user = sqlx::query_as::<_, User>(&format!(
        r#"
        UPDATE users
        SET display_name = $1, system_role = $2, department_id = $3, is_active = $4
        WHERE id = $5 AND organization_id = $6
        RETURNING {USER_COLUMNS}
        "#
    ))
    .bind(display_name)
    .bind(system_role)
    .bind(primary_department_id)
    .bind(is_active)
    .bind(id)
    .bind(actor.organization_id)
    .fetch_one(&mut *transaction)
    .await?;
    sync_department_memberships(
        &mut transaction,
        actor.organization_id,
        user.id,
        &department_ids,
        primary_department_id,
    )
    .await?;
    let token_hashes = if existing.is_active && !user.is_active {
        sqlx::query_scalar::<_, String>(
            "DELETE FROM sessions WHERE user_id = $1 RETURNING token_hash",
        )
        .bind(id)
        .fetch_all(&mut *transaction)
        .await?
    } else {
        Vec::new()
    };
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "user.update",
        "user",
        Some(user.id),
        json!({
            "email": user.email,
            "system_role": user.system_role,
            "department_id": user.department_id,
            "department_ids": department_ids,
            "is_active": user.is_active,
            "sessions_revoked": token_hashes.len(),
            "managed_department_ids_cleared": cleared_managed_department_ids,
        }),
    )
    .await?;
    transaction.commit().await?;

    clear_session_cache(&state, &token_hashes).await;
    Ok(ok(account_view(user, department_ids, &actor)))
}

pub async fn reset_password(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<ResetPasswordRequest>,
) -> ApiResult<Json<ApiEnvelope<AccountView>>> {
    let password_hash = hash_password(&validate_password(&payload.password)?)?;
    let mut transaction = state.db.begin().await?;
    let (actor, managed_department_ids) =
        lock_account_write_context(&mut transaction, &current).await?;
    let existing =
        auth::lock_account_target_for_update_in_transaction(&mut transaction, &actor, id).await?;
    let department_ids =
        department_ids_in_transaction(&mut transaction, actor.organization_id, id).await?;
    if !account_is_in_scope(&actor, &managed_department_ids, &department_ids) {
        return Err(ApiError::NotFound);
    }
    ensure_scoped_target_is_member(&actor.system_role, &existing.system_role)?;
    let user = sqlx::query_as::<_, User>(&format!(
        r#"
        UPDATE users
        SET password_hash = $1
        WHERE id = $2 AND organization_id = $3
        RETURNING {USER_COLUMNS}
        "#
    ))
    .bind(password_hash)
    .bind(id)
    .bind(actor.organization_id)
    .fetch_one(&mut *transaction)
    .await?;
    let token_hashes = sqlx::query_scalar::<_, String>(
        "DELETE FROM sessions WHERE user_id = $1 RETURNING token_hash",
    )
    .bind(id)
    .fetch_all(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "user.reset_password",
        "user",
        Some(user.id),
        json!({ "email": existing.email, "sessions_revoked": token_hashes.len() }),
    )
    .await?;
    transaction.commit().await?;

    clear_session_cache(&state, &token_hashes).await;
    Ok(ok(account_view(user, department_ids, &actor)))
}

pub async fn list_user_project_grants(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<Vec<AccountProjectGrant>>>> {
    ensure_account_visible(&state, &current, id).await?;
    let manageable_project_ids = access::manageable_project_ids(&state, &current).await?;
    if manageable_project_ids.is_empty() {
        return Ok(ok(Vec::new()));
    }
    let grants = sqlx::query_as::<_, AccountProjectGrant>(
        r#"
        SELECT project.id AS project_id, project.name AS project_name, member.project_role
        FROM project_members member
        JOIN projects project ON project.id = member.project_id
        WHERE member.user_id = $1
          AND project.organization_id = $2
          AND project.archived_at IS NULL
          AND project.id = ANY($3)
        ORDER BY project.name, project.id
        "#,
    )
    .bind(id)
    .bind(current.organization_id)
    .bind(&manageable_project_ids)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(grants))
}

pub async fn put_user_project_grant(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((id, project_id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<projects::PutProjectUserGrantRequest>,
) -> ApiResult<Json<ApiEnvelope<ProjectUserGrant>>> {
    let mut transaction = state.db.begin().await?;
    let context =
        projects::lock_project_user_grant_context(&mut transaction, &current, project_id, id, true)
            .await?;
    let managed_department_ids =
        managed_department_ids_with_executor(&mut *transaction, &context.actor).await?;
    ensure_account_manager(&context.actor, &managed_department_ids)?;
    let department_ids = department_ids_in_transaction(
        &mut transaction,
        context.actor.organization_id,
        context.target.id,
    )
    .await?;
    if !account_is_in_scope(&context.actor, &managed_department_ids, &department_ids) {
        return Err(ApiError::NotFound);
    }
    let grant =
        projects::put_locked_project_user_grant(&mut transaction, &context, payload.project_role)
            .await?;
    transaction.commit().await?;
    Ok(ok(grant))
}

pub async fn delete_user_project_grant(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((id, project_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<ApiEnvelope<projects::DeleteResponse>>> {
    let mut transaction = state.db.begin().await?;
    let context = projects::lock_project_user_grant_context(
        &mut transaction,
        &current,
        project_id,
        id,
        false,
    )
    .await?;
    let managed_department_ids =
        managed_department_ids_with_executor(&mut *transaction, &context.actor).await?;
    ensure_account_manager(&context.actor, &managed_department_ids)?;
    let department_ids = department_ids_in_transaction(
        &mut transaction,
        context.actor.organization_id,
        context.target.id,
    )
    .await?;
    if !account_is_in_scope(&context.actor, &managed_department_ids, &department_ids) {
        return Err(ApiError::NotFound);
    }
    let response = projects::delete_locked_project_user_grant(&mut transaction, &context).await?;
    transaction.commit().await?;
    Ok(ok(response))
}

async fn ensure_email_available(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    email: &str,
) -> ApiResult<()> {
    let existing: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE lower(email) = lower($1) LIMIT 1")
            .bind(email)
            .fetch_optional(&mut **transaction)
            .await?;
    if existing.is_some() {
        return Err(ApiError::Conflict("email already exists".to_string()));
    }
    let active_registration: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
          SELECT 1 FROM registration_requests
          WHERE lower(email) = lower($1)
            AND status IN ('pending_verification', 'pending_review', 'approved')
        )
        "#,
    )
    .bind(email)
    .fetch_one(&mut **transaction)
    .await?;
    if active_registration {
        return Err(ApiError::Conflict(
            "email already has an active registration or invitation".to_string(),
        ));
    }
    Ok(())
}

async fn clear_managed_departments_if_required(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    organization_id: Uuid,
    user_id: Uuid,
    required: bool,
) -> ApiResult<Vec<Uuid>> {
    if !required {
        return Ok(Vec::new());
    }
    let department_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT department_id FROM department_admins WHERE user_id = $1 ORDER BY department_id",
    )
    .bind(user_id)
    .fetch_all(&mut **transaction)
    .await?;
    if department_ids.is_empty() {
        return Ok(department_ids);
    }
    let empty_scope = Vec::<Uuid>::new();
    let last_department: Option<Uuid> = sqlx::query_scalar(LAST_DEPARTMENT_ADMIN_SQL)
        .bind(user_id)
        .bind(&empty_scope)
        .bind(organization_id)
        .fetch_optional(&mut **transaction)
        .await?;
    if last_department.is_some() {
        return Err(ApiError::Conflict(
            "assign another department administrator before deactivating or promoting this account"
                .to_string(),
        ));
    }
    sqlx::query("DELETE FROM department_admins WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut **transaction)
        .await?;
    Ok(department_ids)
}

fn clean_email(value: &str) -> ApiResult<String> {
    auth::normalize_email(value)
}

pub async fn get_managed_departments(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<ManagedDepartmentsView>>> {
    if current.system_role != SystemRole::SystemAdmin {
        return Err(ApiError::Forbidden);
    }
    let target_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND organization_id = $2)",
    )
    .bind(id)
    .bind(current.organization_id)
    .fetch_one(&state.db)
    .await?;
    if !target_exists {
        return Err(ApiError::NotFound);
    }
    let department_ids = sqlx::query_scalar(
        r#"
        SELECT admin.department_id
        FROM department_admins admin
        JOIN departments department ON department.id = admin.department_id
        WHERE admin.user_id = $1 AND department.organization_id = $2
        ORDER BY department.sort_order, department.name, department.id
        "#,
    )
    .bind(id)
    .bind(current.organization_id)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(ManagedDepartmentsView { department_ids }))
}

pub async fn update_managed_departments(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateManagedDepartmentsRequest>,
) -> ApiResult<Json<ApiEnvelope<ManagedDepartmentsView>>> {
    if current.system_role != SystemRole::SystemAdmin {
        return Err(ApiError::Forbidden);
    }
    let mut seen = HashSet::new();
    let department_ids = payload
        .department_ids
        .into_iter()
        .filter(|department_id| seen.insert(*department_id))
        .collect::<Vec<_>>();
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    if actor.system_role != SystemRole::SystemAdmin {
        return Err(ApiError::Forbidden);
    }
    access::lock_organization_department_tree(&mut transaction, actor.organization_id).await?;
    let target =
        auth::lock_account_target_for_update_in_transaction(&mut transaction, &actor, id).await?;
    if !managed_departments_target_allowed(&target.system_role, target.is_active) {
        return Err(ApiError::BadRequest(
            "managed departments can only be assigned to active member accounts".to_string(),
        ));
    }
    ensure_departments_exist(&mut transaction, actor.organization_id, &department_ids).await?;
    let existing_department_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT department_id FROM department_admins WHERE user_id = $1 ORDER BY department_id",
    )
    .bind(id)
    .fetch_all(&mut *transaction)
    .await?;
    let last_managed_department: Option<Uuid> = sqlx::query_scalar(LAST_DEPARTMENT_ADMIN_SQL)
        .bind(id)
        .bind(&department_ids)
        .bind(actor.organization_id)
        .fetch_optional(&mut *transaction)
        .await?;
    if last_managed_department.is_some() {
        return Err(ApiError::Conflict(
            "assign another department administrator before removing the last one".to_string(),
        ));
    }
    sqlx::query("DELETE FROM department_admins WHERE user_id = $1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    if !department_ids.is_empty() {
        sqlx::query(
            r#"
            INSERT INTO department_admins (department_id, user_id)
            SELECT department_id, $1
            FROM unnest($2::uuid[]) AS department_id
            "#,
        )
        .bind(id)
        .bind(&department_ids)
        .execute(&mut *transaction)
        .await?;
    }
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "user.managed_departments.update",
        "user",
        Some(id),
        json!({
            "previous_department_ids": existing_department_ids,
            "department_ids": department_ids,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(ManagedDepartmentsView { department_ids }))
}

fn managed_departments_target_allowed(system_role: &SystemRole, is_active: bool) -> bool {
    *system_role == SystemRole::Member && is_active
}

fn required_text(value: &str, field: &str) -> ApiResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ApiError::BadRequest(format!("{field} is required")));
    }
    Ok(trimmed.chars().take(240).collect())
}

#[cfg(test)]
mod tests {
    use super::{
        LAST_DEPARTMENT_ADMIN_SQL, UpdateUserRequest, ensure_scoped_create_fields,
        ensure_scoped_target_is_member, ensure_scoped_update_fields,
        managed_departments_target_allowed,
    };
    use crate::models::SystemRole;
    use uuid::Uuid;

    #[test]
    fn scoped_admin_can_only_create_department_members() {
        let department_id = Uuid::new_v4();
        ensure_scoped_create_fields(&SystemRole::Member, &[department_id]).unwrap();
        assert!(ensure_scoped_create_fields(&SystemRole::SystemAdmin, &[department_id]).is_err());
        assert!(ensure_scoped_create_fields(&SystemRole::Member, &[]).is_err());
    }

    #[test]
    fn scoped_admin_cannot_change_system_role_or_active_state() {
        for payload in [
            UpdateUserRequest {
                display_name: None,
                system_role: Some(SystemRole::Member),
                department_id: None,
                department_ids: None,
                primary_department_id: None,
                is_active: None,
            },
            UpdateUserRequest {
                display_name: None,
                system_role: None,
                department_id: None,
                department_ids: None,
                primary_department_id: None,
                is_active: Some(true),
            },
        ] {
            assert!(ensure_scoped_update_fields(&payload).is_err());
        }
    }

    #[test]
    fn scoped_admin_cannot_clear_primary_department() {
        let payload = UpdateUserRequest {
            display_name: Some("Scoped member".to_string()),
            system_role: None,
            department_id: Some(None),
            department_ids: Some(Vec::new()),
            primary_department_id: Some(None),
            is_active: None,
        };
        assert!(ensure_scoped_update_fields(&payload).is_err());
    }

    #[test]
    fn scoped_admin_cannot_manage_a_system_administrator_account() {
        assert!(
            ensure_scoped_target_is_member(&SystemRole::Member, &SystemRole::SystemAdmin).is_err()
        );
        ensure_scoped_target_is_member(&SystemRole::Member, &SystemRole::Member).unwrap();
        ensure_scoped_target_is_member(&SystemRole::SystemAdmin, &SystemRole::SystemAdmin).unwrap();
    }

    #[test]
    fn managed_departments_require_an_active_member_and_preserve_the_last_admin() {
        assert!(managed_departments_target_allowed(
            &SystemRole::Member,
            true
        ));
        assert!(!managed_departments_target_allowed(
            &SystemRole::Member,
            false
        ));
        assert!(!managed_departments_target_allowed(
            &SystemRole::SystemAdmin,
            true
        ));
        assert!(LAST_DEPARTMENT_ADMIN_SQL.contains("other.user_id <> $1"));
        assert!(LAST_DEPARTMENT_ADMIN_SQL.contains("other_user.is_active = TRUE"));
    }
}
