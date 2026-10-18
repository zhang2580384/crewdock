use crate::{
    access,
    app::AppState,
    audit::log_event_in_transaction,
    auth::{self, CurrentUser},
    error::{ApiError, ApiResult},
    models::{
        MarkdownDoc, MarkdownFolder, Project, ProjectDepartmentGrant, ProjectLink, ProjectListItem,
        ProjectRole, ProjectUserGrant, SystemRole, User, UserRole,
    },
    response::{ApiEnvelope, ok},
    share,
};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use std::collections::HashSet;
use uuid::Uuid;

const CANCEL_PROJECT_UPLOAD_JOBS_SQL: &str = r#"
    UPDATE file_storage_jobs
    SET operation_state = 'cancelled', consecutive_failures = 0,
        terminal_at = NULL,
        next_attempt_at = GREATEST(next_attempt_at, now() + interval '15 minutes'),
        updated_at = now()
    WHERE project_id = $1 AND organization_id = $2 AND kind = 'upload_cleanup'
"#;
const DELETE_PROJECT_FILE_DELETE_JOBS_SQL: &str = r#"
    DELETE FROM file_storage_jobs
    WHERE project_id = $1 AND organization_id = $2 AND kind = 'delete'
"#;
const LIST_LINKS_SQL: &str = r#"
    SELECT link.id, link.project_id, link.title, link.url, link.description,
           link.tags, link.created_at
    FROM project_links link
    JOIN projects project ON project.id = link.project_id
    WHERE link.project_id = $1 AND project.organization_id = $2
      AND project.archived_at IS NULL
    ORDER BY link.created_at DESC
"#;
const LIST_FOLDERS_SQL: &str = r#"
    WITH RECURSIVE department_ancestors AS (
      SELECT department.id, department.parent_id, ARRAY[department.id] AS path
      FROM user_department_memberships membership
      JOIN departments department ON department.id = membership.department_id
      WHERE membership.user_id = $3 AND membership.organization_id = $2

      UNION ALL

      SELECT parent.id, parent.parent_id, ancestors.path || parent.id
      FROM departments parent
      JOIN department_ancestors ancestors ON ancestors.parent_id = parent.id
      WHERE parent.organization_id = $2
        AND NOT parent.id = ANY(ancestors.path)
    ),
    folder_ancestors AS (
      SELECT folder.id AS target_folder_id, folder.id AS ancestor_folder_id,
             folder.parent_id, ARRAY[folder.id] AS path
      FROM markdown_folders folder
      WHERE folder.project_id = $1

      UNION ALL

      SELECT ancestors.target_folder_id, parent.id, parent.parent_id,
             ancestors.path || parent.id
      FROM markdown_folders parent
      JOIN folder_ancestors ancestors ON ancestors.parent_id = parent.id
      WHERE parent.project_id = $1
        AND NOT parent.id = ANY(ancestors.path)
    )
    SELECT folder.id, folder.project_id, folder.parent_id, folder.name,
           folder.sort_order, folder.created_by, folder.created_at, folder.updated_at,
           (
             $4 OR (
               $5 AND NOT EXISTS (
                 SELECT 1
                 FROM folder_ancestors ancestors
                 WHERE ancestors.target_folder_id = folder.id
                   AND EXISTS (
                     SELECT 1 FROM markdown_folder_department_grants restriction
                     WHERE restriction.folder_id = ancestors.ancestor_folder_id
                   )
                   AND NOT EXISTS (
                     SELECT 1
                     FROM markdown_folder_department_grants matching
                     WHERE matching.folder_id = ancestors.ancestor_folder_id
                       AND matching.department_id IN (SELECT id FROM department_ancestors)
                       AND matching.access_role >= 'editor'::project_role
                   )
               )
             )
           ) AS can_write,
           EXISTS (
             SELECT 1 FROM markdown_folder_department_grants own_grant
             WHERE own_grant.folder_id = folder.id
           ) AS has_custom_access
    FROM markdown_folders folder
    JOIN projects project ON project.id = folder.project_id
    WHERE folder.project_id = $1 AND project.organization_id = $2
      AND project.archived_at IS NULL
      AND (
        $4 OR NOT EXISTS (
          SELECT 1
          FROM folder_ancestors ancestors
          WHERE ancestors.target_folder_id = folder.id
            AND EXISTS (
              SELECT 1 FROM markdown_folder_department_grants restriction
              WHERE restriction.folder_id = ancestors.ancestor_folder_id
            )
            AND NOT EXISTS (
              SELECT 1
              FROM markdown_folder_department_grants matching
              WHERE matching.folder_id = ancestors.ancestor_folder_id
                AND matching.department_id IN (SELECT id FROM department_ancestors)
            )
        )
      )
    ORDER BY folder.parent_id NULLS FIRST, folder.sort_order ASC, folder.name ASC
"#;
const LIST_DOCS_SQL: &str = r#"
    WITH RECURSIVE department_ancestors AS (
      SELECT department.id, department.parent_id, ARRAY[department.id] AS path
      FROM user_department_memberships membership
      JOIN departments department ON department.id = membership.department_id
      WHERE membership.user_id = $3 AND membership.organization_id = $2

      UNION ALL

      SELECT parent.id, parent.parent_id, ancestors.path || parent.id
      FROM departments parent
      JOIN department_ancestors ancestors ON ancestors.parent_id = parent.id
      WHERE parent.organization_id = $2
        AND NOT parent.id = ANY(ancestors.path)
    ),
    folder_ancestors AS (
      SELECT folder.id AS target_folder_id, folder.id AS ancestor_folder_id,
             folder.parent_id, ARRAY[folder.id] AS path
      FROM markdown_folders folder
      WHERE folder.project_id = $1

      UNION ALL

      SELECT ancestors.target_folder_id, parent.id, parent.parent_id,
             ancestors.path || parent.id
      FROM markdown_folders parent
      JOIN folder_ancestors ancestors ON ancestors.parent_id = parent.id
      WHERE parent.project_id = $1
        AND NOT parent.id = ANY(ancestors.path)
    )
    SELECT doc.id, doc.project_id, doc.folder_id, doc.title, doc.body_markdown,
           doc.tags, doc.sort_order, doc.created_by, doc.updated_at,
           (
             $4 OR (
               $5
               AND NOT EXISTS (
                 SELECT 1
                 FROM folder_ancestors ancestors
                 WHERE ancestors.target_folder_id = doc.folder_id
                   AND EXISTS (
                     SELECT 1 FROM markdown_folder_department_grants restriction
                     WHERE restriction.folder_id = ancestors.ancestor_folder_id
                   )
                   AND NOT EXISTS (
                     SELECT 1
                     FROM markdown_folder_department_grants matching
                     WHERE matching.folder_id = ancestors.ancestor_folder_id
                       AND matching.department_id IN (SELECT id FROM department_ancestors)
                       AND matching.access_role >= 'editor'::project_role
                   )
               )
               AND (
                 NOT EXISTS (
                   SELECT 1 FROM markdown_doc_department_grants restriction
                   WHERE restriction.doc_id = doc.id
                 )
                 OR EXISTS (
                   SELECT 1 FROM markdown_doc_department_grants matching
                   WHERE matching.doc_id = doc.id
                     AND matching.department_id IN (SELECT id FROM department_ancestors)
                     AND matching.access_role >= 'editor'::project_role
                 )
               )
             )
           ) AS can_write,
           EXISTS (
             SELECT 1 FROM markdown_doc_department_grants own_grant
             WHERE own_grant.doc_id = doc.id
           ) AS has_custom_access
    FROM markdown_docs doc
    JOIN projects project ON project.id = doc.project_id
    WHERE doc.project_id = $1 AND project.organization_id = $2
      AND project.archived_at IS NULL
      AND (
        $4 OR (
          NOT EXISTS (
            SELECT 1
            FROM folder_ancestors ancestors
            WHERE ancestors.target_folder_id = doc.folder_id
              AND EXISTS (
                SELECT 1 FROM markdown_folder_department_grants restriction
                WHERE restriction.folder_id = ancestors.ancestor_folder_id
              )
              AND NOT EXISTS (
                SELECT 1
                FROM markdown_folder_department_grants matching
                WHERE matching.folder_id = ancestors.ancestor_folder_id
                  AND matching.department_id IN (SELECT id FROM department_ancestors)
              )
          )
          AND (
            NOT EXISTS (
              SELECT 1 FROM markdown_doc_department_grants restriction
              WHERE restriction.doc_id = doc.id
            )
            OR EXISTS (
              SELECT 1 FROM markdown_doc_department_grants matching
              WHERE matching.doc_id = doc.id
                AND matching.department_id IN (SELECT id FROM department_ancestors)
            )
          )
        )
      )
    ORDER BY doc.folder_id NULLS FIRST, doc.sort_order ASC, doc.title ASC
"#;

#[derive(Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub department_id: Option<Uuid>,
}

#[derive(Deserialize)]
pub struct UpdateProjectRequest {
    pub name: String,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub department_id: Option<Option<Uuid>>,
}

#[derive(Deserialize)]
pub struct PutProjectUserGrantRequest {
    pub project_role: ProjectRole,
}

#[derive(Deserialize)]
pub struct PutProjectDepartmentGrantRequest {
    pub project_role: ProjectRole,
    pub include_descendants: bool,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct ListProjectsQuery {
    pub include_archived: bool,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct ProjectGrantCandidate {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub department_id: Option<Uuid>,
}

#[derive(Deserialize)]
pub struct CreateLinkRequest {
    pub title: String,
    pub url: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
pub struct UpdateLinkRequest {
    pub title: String,
    pub url: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
pub struct CreateDocRequest {
    pub folder_id: Option<Uuid>,
    pub title: String,
    pub body_markdown: String,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
pub struct UpdateDocRequest {
    pub title: String,
    pub body_markdown: String,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
pub struct CreateFolderRequest {
    pub parent_id: Option<Uuid>,
    pub name: String,
}

#[derive(Deserialize)]
pub struct UpdateFolderRequest {
    pub name: String,
}

#[derive(Deserialize)]
pub struct MoveFolderRequest {
    pub parent_id: Option<Uuid>,
    pub position: i32,
}

#[derive(Deserialize)]
pub struct MoveDocRequest {
    pub folder_id: Option<Uuid>,
    pub position: i32,
}

#[derive(Deserialize)]
pub struct MarkdownDepartmentGrantInput {
    pub department_id: Uuid,
    pub access_role: ProjectRole,
}

#[derive(Deserialize)]
pub struct ReplaceMarkdownDepartmentPermissionsRequest {
    pub department_grants: Vec<MarkdownDepartmentGrantInput>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MarkdownDepartmentGrantView {
    pub department_id: Uuid,
    pub department_name: String,
    pub access_role: ProjectRole,
}

#[derive(sqlx::FromRow)]
struct MarkdownResourceAccess {
    can_read: bool,
    can_write: bool,
}

#[derive(Clone, Copy)]
enum MarkdownPermissionTarget {
    Folder,
    Doc,
}

#[derive(Serialize)]
pub struct DeleteResponse {
    pub deleted: bool,
}

#[derive(Serialize)]
pub struct ProjectDeleteResponse {
    pub deleted: bool,
    pub files_cleanup_pending: bool,
}

fn validate_project_input(name: &str, slug: &str) -> ApiResult<()> {
    if name.trim().is_empty() {
        return Err(ApiError::BadRequest("project name is required".to_string()));
    }
    if slug.is_empty()
        || !slug
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(ApiError::BadRequest(
            "project slug may only contain lowercase letters, numbers, and hyphens".to_string(),
        ));
    }
    Ok(())
}

fn generate_project_slug(name: &str, suffix: &str) -> String {
    let mut prefix = String::new();
    let mut pending_separator = false;

    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_separator && !prefix.is_empty() {
                prefix.push('-');
            }
            prefix.push(ch.to_ascii_lowercase());
            pending_separator = false;
        } else if !prefix.is_empty() {
            pending_separator = true;
        }
    }

    if prefix.is_empty() {
        prefix.push_str("project");
    }
    format!("{prefix}-{suffix}")
}

fn generated_project_slug(name: &str) -> String {
    let suffix = Uuid::new_v4().simple().to_string();
    generate_project_slug(name, &suffix[..12])
}

fn legacy_role_for_project_role(project_role: ProjectRole) -> UserRole {
    match project_role {
        ProjectRole::Administrator => UserRole::ProjectAdmin,
        ProjectRole::Editor => UserRole::ProjectMember,
        ProjectRole::Viewer => UserRole::ReadonlyMember,
    }
}

fn ensure_last_direct_admin_change(
    actor_is_system_admin: bool,
    current_role: Option<ProjectRole>,
    requested_role: Option<ProjectRole>,
    direct_admin_count: i64,
) -> ApiResult<()> {
    let removes_administrator = current_role == Some(ProjectRole::Administrator)
        && requested_role != Some(ProjectRole::Administrator);
    if !actor_is_system_admin && removes_administrator && direct_admin_count <= 1 {
        return Err(ApiError::BadRequest(
            "project must retain at least one direct administrator".to_string(),
        ));
    }
    Ok(())
}

fn ensure_project_administrator(role: ProjectRole) -> ApiResult<()> {
    if !role.can_manage() {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

// Project mutations use one global order: actor row, department tree, then project row.
async fn lock_fresh_actor_and_department_tree(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_actor: &CurrentUser,
) -> ApiResult<CurrentUser> {
    let actor = auth::lock_current_user_in_transaction(transaction, request_actor).await?;
    access::lock_organization_department_tree(transaction, actor.organization_id).await?;
    Ok(actor)
}

async fn count_direct_project_administrators(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: Uuid,
) -> ApiResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM project_members WHERE project_id = $1 AND project_role = 'administrator'::project_role",
    )
    .bind(project_id)
    .fetch_one(&mut **transaction)
    .await?)
}

pub(crate) struct LockedProjectUserGrantContext {
    pub actor: CurrentUser,
    pub target: User,
    project_id: Uuid,
    user_id: Uuid,
}

pub(crate) async fn lock_project_user_grant_context(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_actor: &CurrentUser,
    project_id: Uuid,
    user_id: Uuid,
    require_active_target: bool,
) -> ApiResult<LockedProjectUserGrantContext> {
    let actor = lock_fresh_actor_and_department_tree(transaction, request_actor).await?;
    access::lock_project_in_transaction(transaction, &actor, project_id, false).await?;
    let project_access =
        access::project_access_in_transaction(transaction, &actor, project_id).await?;
    ensure_project_administrator(project_access.role)?;
    let target = auth::lock_project_grant_target_in_transaction(
        transaction,
        &actor,
        user_id,
        require_active_target,
    )
    .await?;
    Ok(LockedProjectUserGrantContext {
        actor,
        target,
        project_id,
        user_id,
    })
}

pub(crate) async fn put_locked_project_user_grant(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    context: &LockedProjectUserGrantContext,
    project_role: ProjectRole,
) -> ApiResult<ProjectUserGrant> {
    let existing_role = sqlx::query_scalar::<_, ProjectRole>(
        "SELECT project_role FROM project_members WHERE project_id = $1 AND user_id = $2",
    )
    .bind(context.project_id)
    .bind(context.user_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let direct_admin_count =
        count_direct_project_administrators(transaction, context.project_id).await?;
    ensure_last_direct_admin_change(
        context.actor.system_role == SystemRole::SystemAdmin,
        existing_role,
        Some(project_role),
        direct_admin_count,
    )?;

    sqlx::query(
        r#"
        INSERT INTO project_members (project_id, user_id, role, project_role)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (project_id, user_id) DO UPDATE
        SET role = excluded.role, project_role = excluded.project_role
        "#,
    )
    .bind(context.project_id)
    .bind(context.user_id)
    .bind(legacy_role_for_project_role(project_role))
    .bind(project_role)
    .execute(&mut **transaction)
    .await?;
    let grant = sqlx::query_as::<_, ProjectUserGrant>(
        r#"
        SELECT u.id AS user_id, u.display_name, u.email, pm.project_role
        FROM project_members pm
        JOIN users u ON u.id = pm.user_id
        WHERE pm.project_id = $1 AND pm.user_id = $2
        "#,
    )
    .bind(context.project_id)
    .bind(context.user_id)
    .fetch_one(&mut **transaction)
    .await?;
    let action = if existing_role.is_some() {
        "project.user_grant.update"
    } else {
        "project.user_grant.create"
    };
    log_event_in_transaction(
        transaction,
        context.actor.organization_id,
        Some(context.actor.id),
        action,
        "project_user_grant",
        Some(context.user_id),
        json!({
            "project_id": context.project_id,
            "user_id": context.user_id,
            "project_role": project_role,
            "previous_project_role": existing_role,
        }),
    )
    .await?;
    Ok(grant)
}

pub(crate) async fn delete_locked_project_user_grant(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    context: &LockedProjectUserGrantContext,
) -> ApiResult<DeleteResponse> {
    let existing_role = sqlx::query_scalar::<_, ProjectRole>(
        "SELECT project_role FROM project_members WHERE project_id = $1 AND user_id = $2",
    )
    .bind(context.project_id)
    .bind(context.user_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    let direct_admin_count =
        count_direct_project_administrators(transaction, context.project_id).await?;
    ensure_last_direct_admin_change(
        context.actor.system_role == SystemRole::SystemAdmin,
        Some(existing_role),
        None,
        direct_admin_count,
    )?;
    sqlx::query("DELETE FROM project_members WHERE project_id = $1 AND user_id = $2")
        .bind(context.project_id)
        .bind(context.user_id)
        .execute(&mut **transaction)
        .await?;
    log_event_in_transaction(
        transaction,
        context.actor.organization_id,
        Some(context.actor.id),
        "project.user_grant.delete",
        "project_user_grant",
        Some(context.user_id),
        json!({
            "project_id": context.project_id,
            "user_id": context.user_id,
            "project_role": existing_role,
        }),
    )
    .await?;
    Ok(DeleteResponse { deleted: true })
}

fn validate_link_input(title: &str, url: &str) -> ApiResult<()> {
    if title.trim().is_empty() {
        return Err(ApiError::BadRequest("link title is required".to_string()));
    }
    if url.trim().is_empty() {
        return Err(ApiError::BadRequest("link url is required".to_string()));
    }
    Ok(())
}

fn validate_doc_input(title: &str) -> ApiResult<()> {
    if title.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "document title is required".to_string(),
        ));
    }
    Ok(())
}

fn validate_folder_name(name: &str) -> ApiResult<()> {
    if name.trim().is_empty() {
        return Err(ApiError::BadRequest("folder name is required".to_string()));
    }
    Ok(())
}

fn ensure_valid_folder_move(
    folder_id: Uuid,
    parent_id: Option<Uuid>,
    descendant_ids: &[Uuid],
) -> ApiResult<()> {
    if parent_id == Some(folder_id)
        || parent_id.is_some_and(|parent_id| descendant_ids.contains(&parent_id))
    {
        return Err(ApiError::BadRequest(
            "folder cannot be moved into itself or a descendant".to_string(),
        ));
    }
    Ok(())
}

fn normalize_move_position(position: i32, sibling_count: i64) -> i32 {
    position.clamp(0, sibling_count.clamp(0, i32::MAX as i64) as i32)
}

fn normalize_reordered_position(
    position: i32,
    sibling_count: i64,
    old_position: Option<i32>,
) -> i32 {
    let position = match old_position {
        Some(old_position) if position > old_position => position - 1,
        _ => position,
    };
    normalize_move_position(position, sibling_count)
}

async fn lock_markdown_tree(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: Uuid,
) -> ApiResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text || ':markdown-tree', 0))")
        .bind(project_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

pub async fn list_projects(
    State(state): State<AppState>,
    current: CurrentUser,
    Query(query): Query<ListProjectsQuery>,
) -> ApiResult<Json<ApiEnvelope<Vec<ProjectListItem>>>> {
    let projects = access::visible_projects(&state, &current, query.include_archived).await?;
    Ok(ok(projects))
}

pub async fn list_project_assignable_users(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<Vec<access::ProjectAssignableUser>>>> {
    Ok(ok(access::project_assignable_users(
        &state, &current, project_id,
    )
    .await?))
}

pub async fn list_project_grant_candidates(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<Vec<ProjectGrantCandidate>>>> {
    let project_access = access::project_access(&state, &current, project_id).await?;
    ensure_project_administrator(project_access.role)?;
    let users = sqlx::query_as::<_, ProjectGrantCandidate>(
        r#"
        SELECT id, email, display_name, department_id
        FROM users
        WHERE organization_id = $1 AND is_active = TRUE
        ORDER BY display_name ASC, email ASC, id
        "#,
    )
    .bind(current.organization_id)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(users))
}

pub async fn create_project(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(payload): Json<CreateProjectRequest>,
) -> ApiResult<Json<ApiEnvelope<Project>>> {
    let mut transaction = state.db.begin().await?;
    let actor = lock_fresh_actor_and_department_tree(&mut transaction, &current).await?;
    if payload.name.trim().is_empty() {
        return Err(ApiError::BadRequest("project name is required".to_string()));
    }
    let slug = match payload.slug {
        Some(slug) if !slug.trim().is_empty() => {
            validate_project_input(&payload.name, &slug)?;
            slug
        }
        _ => generated_project_slug(&payload.name),
    };

    let is_system_admin = actor.system_role == SystemRole::SystemAdmin;
    match payload.department_id {
        Some(department_id) => {
            let manages =
                access::manages_department_in_transaction(&mut transaction, &actor, department_id)
                    .await?;
            if !is_system_admin && !manages {
                return Err(ApiError::Forbidden);
            }
        }
        None if !is_system_admin => return Err(ApiError::Forbidden),
        None => {}
    }

    let project = sqlx::query_as::<_, Project>(
        r#"
        INSERT INTO projects (organization_id, department_id, name, slug, description)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, organization_id, department_id, name, slug, description, metadata,
                  created_at, archived_at
        "#,
    )
    .bind(actor.organization_id)
    .bind(payload.department_id)
    .bind(payload.name)
    .bind(slug)
    .bind(payload.description)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO project_members (project_id, user_id, role, project_role)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (project_id, user_id) DO UPDATE
        SET role = excluded.role, project_role = excluded.project_role
        "#,
    )
    .bind(project.id)
    .bind(actor.id)
    .bind(UserRole::ProjectAdmin)
    .bind(ProjectRole::Administrator)
    .execute(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "project.create",
        "project",
        Some(project.id),
        json!({ "name": project.name, "department_id": project.department_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(project))
}

pub async fn update_project(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
    Json(payload): Json<UpdateProjectRequest>,
) -> ApiResult<Json<ApiEnvelope<Project>>> {
    let mut transaction = state.db.begin().await?;
    let actor = lock_fresh_actor_and_department_tree(&mut transaction, &current).await?;
    access::lock_project_in_transaction(&mut transaction, &actor, project_id, false).await?;
    let project_access =
        access::project_access_in_transaction(&mut transaction, &actor, project_id).await?;
    ensure_project_administrator(project_access.role)?;
    if payload.name.trim().is_empty() {
        return Err(ApiError::BadRequest("project name is required".to_string()));
    }

    let slug = match payload.slug {
        Some(slug) if slug.trim().is_empty() => Some(generated_project_slug(&payload.name)),
        Some(slug) => {
            validate_project_input(&payload.name, &slug)?;
            Some(slug)
        }
        None => None,
    };
    let changes_department = payload.department_id.is_some();
    let department_id = payload.department_id.flatten();
    let is_system_admin = actor.system_role == SystemRole::SystemAdmin;

    if changes_department {
        match department_id {
            Some(department_id) => {
                let manages = access::manages_department_in_transaction(
                    &mut transaction,
                    &actor,
                    department_id,
                )
                .await?;
                if !is_system_admin && !manages {
                    return Err(ApiError::Forbidden);
                }
            }
            None if !is_system_admin => return Err(ApiError::Forbidden),
            None => {}
        }
    }

    let project = sqlx::query_as::<_, Project>(
        r#"
        UPDATE projects
        SET name = $3,
            slug = COALESCE($4, slug),
            description = $5,
            department_id = CASE WHEN $6 THEN $7 ELSE department_id END
        WHERE id = $1 AND organization_id = $2 AND archived_at IS NULL
        RETURNING id, organization_id, department_id, name, slug, description, metadata,
                  created_at, archived_at
        "#,
    )
    .bind(project_id)
    .bind(actor.organization_id)
    .bind(payload.name)
    .bind(slug)
    .bind(payload.description)
    .bind(changes_department)
    .bind(department_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "project.update",
        "project",
        Some(project.id),
        json!({
            "name": project.name,
            "slug": project.slug,
            "department_id": project.department_id,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(project))
}

pub async fn delete_project(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<ProjectDeleteResponse>>> {
    let mut transaction = state.db.begin().await?;
    let actor = lock_fresh_actor_and_department_tree(&mut transaction, &current).await?;
    let project =
        access::lock_project_in_transaction_optional(&mut transaction, &actor, project_id, true)
            .await?;
    if actor.system_role != SystemRole::SystemAdmin {
        return Err(ApiError::Forbidden);
    }

    if let Some(project) = project {
        if project.archived_at.is_none() {
            return Err(ApiError::BadRequest(
                "project must be archived before deletion".to_string(),
            ));
        }

        sqlx::query(
            r#"
            DELETE FROM share_links
            WHERE organization_id = $2
              AND (
                (target_type = 'project' AND target_id = $1)
                OR (target_type = 'project_link' AND target_id IN (
                  SELECT id FROM project_links WHERE project_id = $1
                ))
                OR (target_type = 'markdown_doc' AND target_id IN (
                  SELECT id FROM markdown_docs WHERE project_id = $1
                ))
                OR (target_type = 'file_asset' AND target_id IN (
                  SELECT id FROM file_assets WHERE project_id = $1
                ))
                OR (target_type = 'ticket' AND target_id IN (
                  SELECT id FROM tickets WHERE project_id = $1
                ))
              )
            "#,
        )
        .bind(project_id)
        .bind(actor.organization_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(CANCEL_PROJECT_UPLOAD_JOBS_SQL)
            .bind(project_id)
            .bind(actor.organization_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(DELETE_PROJECT_FILE_DELETE_JOBS_SQL)
            .bind(project_id)
            .bind(actor.organization_id)
            .execute(&mut *transaction)
            .await?;
        let cleanup_job: Option<Uuid> = sqlx::query_scalar(
            r#"
            INSERT INTO project_file_cleanup_jobs (project_id, organization_id)
            VALUES ($1, $2)
            ON CONFLICT (project_id) DO UPDATE
            SET consecutive_failures = 0, terminal_at = NULL,
                next_attempt_at = now(), updated_at = now()
            WHERE project_file_cleanup_jobs.organization_id = excluded.organization_id
            RETURNING project_id
            "#,
        )
        .bind(project_id)
        .bind(actor.organization_id)
        .fetch_optional(&mut *transaction)
        .await?;
        cleanup_job.ok_or(ApiError::NotFound)?;
        log_event_in_transaction(
            &mut transaction,
            actor.organization_id,
            Some(actor.id),
            "project.delete",
            "project",
            Some(project_id),
            json!({}),
        )
        .await?;
        let deleted: Option<Uuid> = sqlx::query_scalar(
            "DELETE FROM projects WHERE id = $1 AND organization_id = $2 RETURNING id",
        )
        .bind(project_id)
        .bind(actor.organization_id)
        .fetch_optional(&mut *transaction)
        .await?;
        deleted.ok_or(ApiError::NotFound)?;
    } else {
        let cleanup_job: Option<Uuid> = sqlx::query_scalar(
            r#"
            SELECT project_id
            FROM project_file_cleanup_jobs
            WHERE project_id = $1 AND organization_id = $2
            FOR UPDATE
            "#,
        )
        .bind(project_id)
        .bind(actor.organization_id)
        .fetch_optional(&mut *transaction)
        .await?;
        cleanup_job.ok_or(ApiError::NotFound)?;
        sqlx::query(CANCEL_PROJECT_UPLOAD_JOBS_SQL)
            .bind(project_id)
            .bind(actor.organization_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "UPDATE project_file_cleanup_jobs SET consecutive_failures = 0, terminal_at = NULL, next_attempt_at = now(), updated_at = now() WHERE project_id = $1 AND organization_id = $2",
        )
        .bind(project_id)
        .bind(actor.organization_id)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    state.file_storage_notify.notify_one();
    state.project_cleanup_notify.notify_one();
    Ok(ok(ProjectDeleteResponse {
        deleted: true,
        files_cleanup_pending: true,
    }))
}

pub async fn archive_project(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<Project>>> {
    let mut transaction = state.db.begin().await?;
    let actor = lock_fresh_actor_and_department_tree(&mut transaction, &current).await?;
    let current_project =
        access::lock_project_in_transaction(&mut transaction, &actor, project_id, true).await?;
    let project_access = access::project_access_including_archived_in_transaction(
        &mut transaction,
        &actor,
        project_id,
    )
    .await?;
    ensure_project_administrator(project_access.role)?;
    if current_project.archived_at.is_some() {
        transaction.commit().await?;
        return Ok(ok(current_project));
    }

    let project = sqlx::query_as::<_, Project>(
        r#"
        UPDATE projects
        SET archived_at = now()
        WHERE id = $1 AND organization_id = $2 AND archived_at IS NULL
        RETURNING id, organization_id, department_id, name, slug, description, metadata,
                  created_at, archived_at
        "#,
    )
    .bind(project_id)
    .bind(actor.organization_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "project.archive",
        "project",
        Some(project_id),
        json!({ "archived_at": project.archived_at }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(project))
}

pub async fn restore_project(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<Project>>> {
    let mut transaction = state.db.begin().await?;
    let actor = lock_fresh_actor_and_department_tree(&mut transaction, &current).await?;
    let current_project =
        access::lock_project_in_transaction(&mut transaction, &actor, project_id, true).await?;
    let project_access = access::project_access_including_archived_in_transaction(
        &mut transaction,
        &actor,
        project_id,
    )
    .await?;
    ensure_project_administrator(project_access.role)?;
    if current_project.archived_at.is_none() {
        transaction.commit().await?;
        return Ok(ok(current_project));
    }

    let project = sqlx::query_as::<_, Project>(
        r#"
        UPDATE projects
        SET archived_at = NULL
        WHERE id = $1 AND organization_id = $2
        RETURNING id, organization_id, department_id, name, slug, description, metadata,
                  created_at, archived_at
        "#,
    )
    .bind(project_id)
    .bind(actor.organization_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "project.restore",
        "project",
        Some(project_id),
        json!({}),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(project))
}

pub async fn list_project_user_grants(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<Vec<ProjectUserGrant>>>> {
    let project_access = access::project_access(&state, &current, project_id).await?;
    ensure_project_administrator(project_access.role)?;

    let grants = sqlx::query_as::<_, ProjectUserGrant>(
        r#"
        SELECT u.id AS user_id, u.display_name, u.email, pm.project_role
        FROM project_members pm
        JOIN projects p ON p.id = pm.project_id
        JOIN users u ON u.id = pm.user_id
        WHERE pm.project_id = $1
          AND p.organization_id = $2
          AND p.archived_at IS NULL
          AND u.organization_id = $2
        ORDER BY u.display_name, u.email, u.id
        "#,
    )
    .bind(project_id)
    .bind(current.organization_id)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(grants))
}

pub async fn put_project_user_grant(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, user_id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<PutProjectUserGrantRequest>,
) -> ApiResult<Json<ApiEnvelope<ProjectUserGrant>>> {
    let mut transaction = state.db.begin().await?;
    let context =
        lock_project_user_grant_context(&mut transaction, &current, project_id, user_id, true)
            .await?;
    let grant =
        put_locked_project_user_grant(&mut transaction, &context, payload.project_role).await?;
    transaction.commit().await?;
    Ok(ok(grant))
}

pub async fn delete_project_user_grant(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, user_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<ApiEnvelope<DeleteResponse>>> {
    let mut transaction = state.db.begin().await?;
    let context =
        lock_project_user_grant_context(&mut transaction, &current, project_id, user_id, false)
            .await?;
    let response = delete_locked_project_user_grant(&mut transaction, &context).await?;
    transaction.commit().await?;
    Ok(ok(response))
}

pub async fn list_project_department_grants(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<Vec<ProjectDepartmentGrant>>>> {
    let project_access = access::project_access(&state, &current, project_id).await?;
    ensure_project_administrator(project_access.role)?;

    let grants = sqlx::query_as::<_, ProjectDepartmentGrant>(
        r#"
        SELECT d.id AS department_id, d.name AS department_name, pdg.project_role,
               pdg.include_descendants
        FROM project_department_grants pdg
        JOIN projects p ON p.id = pdg.project_id
        JOIN departments d ON d.id = pdg.department_id
        WHERE pdg.project_id = $1
          AND p.organization_id = $2
          AND p.archived_at IS NULL
          AND d.organization_id = $2
        ORDER BY d.name, d.id
        "#,
    )
    .bind(project_id)
    .bind(current.organization_id)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(grants))
}

pub async fn put_project_department_grant(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, department_id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<PutProjectDepartmentGrantRequest>,
) -> ApiResult<Json<ApiEnvelope<ProjectDepartmentGrant>>> {
    let mut transaction = state.db.begin().await?;
    let actor = lock_fresh_actor_and_department_tree(&mut transaction, &current).await?;
    access::lock_project_in_transaction(&mut transaction, &actor, project_id, false).await?;
    let project_access =
        access::project_access_in_transaction(&mut transaction, &actor, project_id).await?;
    ensure_project_administrator(project_access.role)?;
    let target: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM departments WHERE id = $1 AND organization_id = $2")
            .bind(department_id)
            .bind(actor.organization_id)
            .fetch_optional(&mut *transaction)
            .await?;
    target.ok_or(ApiError::NotFound)?;
    let existed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM project_department_grants WHERE project_id = $1 AND department_id = $2)",
    )
    .bind(project_id)
    .bind(department_id)
    .fetch_one(&mut *transaction)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO project_department_grants
          (project_id, department_id, project_role, include_descendants)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (project_id, department_id) DO UPDATE
        SET project_role = excluded.project_role,
            include_descendants = excluded.include_descendants
        "#,
    )
    .bind(project_id)
    .bind(department_id)
    .bind(payload.project_role)
    .bind(payload.include_descendants)
    .execute(&mut *transaction)
    .await?;
    let grant = sqlx::query_as::<_, ProjectDepartmentGrant>(
        r#"
        SELECT d.id AS department_id, d.name AS department_name, pdg.project_role,
               pdg.include_descendants
        FROM project_department_grants pdg
        JOIN departments d ON d.id = pdg.department_id
        WHERE pdg.project_id = $1 AND pdg.department_id = $2
        "#,
    )
    .bind(project_id)
    .bind(department_id)
    .fetch_one(&mut *transaction)
    .await?;

    let action = if existed {
        "project.department_grant.update"
    } else {
        "project.department_grant.create"
    };
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        action,
        "project_department_grant",
        Some(department_id),
        json!({
            "project_id": project_id,
            "department_id": department_id,
            "project_role": payload.project_role,
            "include_descendants": payload.include_descendants,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(grant))
}

pub async fn delete_project_department_grant(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, department_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<ApiEnvelope<DeleteResponse>>> {
    let mut transaction = state.db.begin().await?;
    let actor = lock_fresh_actor_and_department_tree(&mut transaction, &current).await?;
    access::lock_project_in_transaction(&mut transaction, &actor, project_id, false).await?;
    let project_access =
        access::project_access_in_transaction(&mut transaction, &actor, project_id).await?;
    ensure_project_administrator(project_access.role)?;
    let target: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM departments WHERE id = $1 AND organization_id = $2")
            .bind(department_id)
            .bind(actor.organization_id)
            .fetch_optional(&mut *transaction)
            .await?;
    target.ok_or(ApiError::NotFound)?;
    let existing: Option<(ProjectRole, bool)> = sqlx::query_as(
        r#"
        SELECT project_role, include_descendants
        FROM project_department_grants
        WHERE project_id = $1 AND department_id = $2
        "#,
    )
    .bind(project_id)
    .bind(department_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let (project_role, include_descendants) = existing.ok_or(ApiError::NotFound)?;

    sqlx::query(
        "DELETE FROM project_department_grants WHERE project_id = $1 AND department_id = $2",
    )
    .bind(project_id)
    .bind(department_id)
    .execute(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "project.department_grant.delete",
        "project_department_grant",
        Some(department_id),
        json!({
            "project_id": project_id,
            "department_id": department_id,
            "project_role": project_role,
            "include_descendants": include_descendants,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(DeleteResponse { deleted: true }))
}

pub async fn list_links(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<Vec<ProjectLink>>>> {
    access::project_access(&state, &current, project_id).await?;
    let rows = sqlx::query_as::<_, ProjectLink>(LIST_LINKS_SQL)
        .bind(project_id)
        .bind(current.organization_id)
        .fetch_all(&state.db)
        .await?;
    Ok(ok(rows))
}

pub async fn create_link(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
    Json(payload): Json<CreateLinkRequest>,
) -> ApiResult<Json<ApiEnvelope<ProjectLink>>> {
    let mut transaction = state.db.begin().await?;
    let (actor, project_access) =
        access::lock_active_project_write_context(&mut transaction, &current, project_id).await?;
    if !project_access.role.can_write() {
        return Err(ApiError::Forbidden);
    }
    validate_link_input(&payload.title, &payload.url)?;
    let row = sqlx::query_as::<_, ProjectLink>(
        r#"
        INSERT INTO project_links (project_id, title, url, description, tags, created_by)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, project_id, title, url, description, tags, created_at
        "#,
    )
    .bind(project_id)
    .bind(payload.title)
    .bind(payload.url)
    .bind(payload.description)
    .bind(payload.tags)
    .bind(actor.id)
    .fetch_one(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "project_link.create",
        "project_link",
        Some(row.id),
        json!({ "project_id": project_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn update_link(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<UpdateLinkRequest>,
) -> ApiResult<Json<ApiEnvelope<ProjectLink>>> {
    let mut transaction = state.db.begin().await?;
    let (actor, project_access) =
        access::lock_active_project_write_context(&mut transaction, &current, project_id).await?;
    if !project_access.role.can_write() {
        return Err(ApiError::Forbidden);
    }
    validate_link_input(&payload.title, &payload.url)?;
    let row = sqlx::query_as::<_, ProjectLink>(
        r#"
        UPDATE project_links
        SET title = $3, url = $4, description = $5, tags = $6
        WHERE id = $1 AND project_id = $2
        RETURNING id, project_id, title, url, description, tags, created_at
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(payload.title)
    .bind(payload.url)
    .bind(payload.description)
    .bind(payload.tags)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "project_link.update",
        "project_link",
        Some(row.id),
        json!({ "project_id": project_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn delete_link(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<ApiEnvelope<DeleteResponse>>> {
    let mut transaction = state.db.begin().await?;
    let actor = lock_fresh_actor_and_department_tree(&mut transaction, &current).await?;
    access::lock_project_in_transaction(&mut transaction, &actor, project_id, false).await?;
    let project_access =
        access::project_access_in_transaction(&mut transaction, &actor, project_id).await?;
    if !project_access.role.can_write() {
        return Err(ApiError::Forbidden);
    }
    let target: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM project_links WHERE id = $1 AND project_id = $2 FOR UPDATE",
    )
    .bind(id)
    .bind(project_id)
    .fetch_optional(&mut *transaction)
    .await?;
    target.ok_or(ApiError::NotFound)?;
    sqlx::query(
        "DELETE FROM share_links WHERE organization_id = $1 AND target_type = $2 AND target_id = $3",
    )
    .bind(actor.organization_id)
    .bind(share::TARGET_PROJECT_LINK)
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    let deleted: Option<Uuid> = sqlx::query_scalar(
        "DELETE FROM project_links WHERE id = $1 AND project_id = $2 RETURNING id",
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
        "project_link.delete",
        "project_link",
        Some(id),
        json!({ "project_id": project_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(DeleteResponse { deleted: true }))
}

const MARKDOWN_FOLDER_ACCESS_SQL: &str = r#"
    WITH RECURSIVE department_ancestors AS (
      SELECT department.id, department.parent_id, ARRAY[department.id] AS path
      FROM user_department_memberships membership
      JOIN departments department ON department.id = membership.department_id
      WHERE membership.user_id = $3 AND membership.organization_id = $2

      UNION ALL

      SELECT parent.id, parent.parent_id, ancestors.path || parent.id
      FROM departments parent
      JOIN department_ancestors ancestors ON ancestors.parent_id = parent.id
      WHERE parent.organization_id = $2
        AND NOT parent.id = ANY(ancestors.path)
    ),
    target_folder AS (
      SELECT folder.id, folder.parent_id, ARRAY[folder.id] AS path
      FROM markdown_folders folder
      WHERE folder.id = $6 AND folder.project_id = $1
    ),
    folder_ancestors AS (
      SELECT id, parent_id, path FROM target_folder

      UNION ALL

      SELECT parent.id, parent.parent_id, ancestors.path || parent.id
      FROM markdown_folders parent
      JOIN folder_ancestors ancestors ON ancestors.parent_id = parent.id
      WHERE parent.project_id = $1
        AND NOT parent.id = ANY(ancestors.path)
    )
    SELECT
      (
        $4 OR NOT EXISTS (
          SELECT 1
          FROM folder_ancestors ancestors
          WHERE EXISTS (
            SELECT 1 FROM markdown_folder_department_grants restriction
            WHERE restriction.folder_id = ancestors.id
          )
          AND NOT EXISTS (
            SELECT 1 FROM markdown_folder_department_grants matching
            WHERE matching.folder_id = ancestors.id
              AND matching.department_id IN (SELECT id FROM department_ancestors)
          )
        )
      ) AS can_read,
      (
        $4 OR (
          $5 AND NOT EXISTS (
            SELECT 1
            FROM folder_ancestors ancestors
            WHERE EXISTS (
              SELECT 1 FROM markdown_folder_department_grants restriction
              WHERE restriction.folder_id = ancestors.id
            )
            AND NOT EXISTS (
              SELECT 1 FROM markdown_folder_department_grants matching
              WHERE matching.folder_id = ancestors.id
                AND matching.department_id IN (SELECT id FROM department_ancestors)
                AND matching.access_role >= 'editor'::project_role
            )
          )
        )
      ) AS can_write
    FROM target_folder
"#;

const MARKDOWN_DOC_ACCESS_SQL: &str = r#"
    WITH RECURSIVE department_ancestors AS (
      SELECT department.id, department.parent_id, ARRAY[department.id] AS path
      FROM user_department_memberships membership
      JOIN departments department ON department.id = membership.department_id
      WHERE membership.user_id = $3 AND membership.organization_id = $2

      UNION ALL

      SELECT parent.id, parent.parent_id, ancestors.path || parent.id
      FROM departments parent
      JOIN department_ancestors ancestors ON ancestors.parent_id = parent.id
      WHERE parent.organization_id = $2
        AND NOT parent.id = ANY(ancestors.path)
    ),
    target_doc AS (
      SELECT doc.id, doc.folder_id
      FROM markdown_docs doc
      WHERE doc.id = $6 AND doc.project_id = $1
    ),
    folder_ancestors AS (
      SELECT folder.id, folder.parent_id, ARRAY[folder.id] AS path
      FROM markdown_folders folder
      JOIN target_doc doc ON doc.folder_id = folder.id
      WHERE folder.project_id = $1

      UNION ALL

      SELECT parent.id, parent.parent_id, ancestors.path || parent.id
      FROM markdown_folders parent
      JOIN folder_ancestors ancestors ON ancestors.parent_id = parent.id
      WHERE parent.project_id = $1
        AND NOT parent.id = ANY(ancestors.path)
    )
    SELECT
      (
        $4 OR (
          NOT EXISTS (
            SELECT 1
            FROM folder_ancestors ancestors
            WHERE EXISTS (
              SELECT 1 FROM markdown_folder_department_grants restriction
              WHERE restriction.folder_id = ancestors.id
            )
            AND NOT EXISTS (
              SELECT 1 FROM markdown_folder_department_grants matching
              WHERE matching.folder_id = ancestors.id
                AND matching.department_id IN (SELECT id FROM department_ancestors)
            )
          )
          AND (
            NOT EXISTS (
              SELECT 1 FROM markdown_doc_department_grants restriction
              WHERE restriction.doc_id = target_doc.id
            )
            OR EXISTS (
              SELECT 1 FROM markdown_doc_department_grants matching
              WHERE matching.doc_id = target_doc.id
                AND matching.department_id IN (SELECT id FROM department_ancestors)
            )
          )
        )
      ) AS can_read,
      (
        $4 OR (
          $5
          AND NOT EXISTS (
            SELECT 1
            FROM folder_ancestors ancestors
            WHERE EXISTS (
              SELECT 1 FROM markdown_folder_department_grants restriction
              WHERE restriction.folder_id = ancestors.id
            )
            AND NOT EXISTS (
              SELECT 1 FROM markdown_folder_department_grants matching
              WHERE matching.folder_id = ancestors.id
                AND matching.department_id IN (SELECT id FROM department_ancestors)
                AND matching.access_role >= 'editor'::project_role
            )
          )
          AND (
            NOT EXISTS (
              SELECT 1 FROM markdown_doc_department_grants restriction
              WHERE restriction.doc_id = target_doc.id
            )
            OR EXISTS (
              SELECT 1 FROM markdown_doc_department_grants matching
              WHERE matching.doc_id = target_doc.id
                AND matching.department_id IN (SELECT id FROM department_ancestors)
                AND matching.access_role >= 'editor'::project_role
            )
          )
        )
      ) AS can_write
    FROM target_doc
"#;

async fn markdown_folder_access_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    current: &CurrentUser,
    project_access: &access::ProjectAccess,
    project_id: Uuid,
    folder_id: Uuid,
) -> ApiResult<MarkdownResourceAccess> {
    sqlx::query_as::<_, MarkdownResourceAccess>(MARKDOWN_FOLDER_ACCESS_SQL)
        .bind(project_id)
        .bind(current.organization_id)
        .bind(current.id)
        .bind(project_access.role.can_manage())
        .bind(project_access.role.can_write())
        .bind(folder_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(ApiError::NotFound)
}

async fn markdown_doc_access_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    current: &CurrentUser,
    project_access: &access::ProjectAccess,
    project_id: Uuid,
    doc_id: Uuid,
) -> ApiResult<MarkdownResourceAccess> {
    sqlx::query_as::<_, MarkdownResourceAccess>(MARKDOWN_DOC_ACCESS_SQL)
        .bind(project_id)
        .bind(current.organization_id)
        .bind(current.id)
        .bind(project_access.role.can_manage())
        .bind(project_access.role.can_write())
        .bind(doc_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(ApiError::NotFound)
}

fn ensure_markdown_resource_write(access: &MarkdownResourceAccess) -> ApiResult<()> {
    if !access.can_read {
        return Err(ApiError::NotFound);
    }
    if !access.can_write {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

async fn ensure_markdown_folder_write_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    current: &CurrentUser,
    project_access: &access::ProjectAccess,
    project_id: Uuid,
    folder_id: Option<Uuid>,
) -> ApiResult<()> {
    match folder_id {
        Some(folder_id) => ensure_markdown_resource_write(
            &markdown_folder_access_in_transaction(
                transaction,
                current,
                project_access,
                project_id,
                folder_id,
            )
            .await?,
        ),
        None if project_access.role.can_write() => Ok(()),
        None => Err(ApiError::Forbidden),
    }
}

async fn ensure_markdown_doc_write_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    current: &CurrentUser,
    project_access: &access::ProjectAccess,
    project_id: Uuid,
    doc_id: Uuid,
) -> ApiResult<()> {
    ensure_markdown_resource_write(
        &markdown_doc_access_in_transaction(
            transaction,
            current,
            project_access,
            project_id,
            doc_id,
        )
        .await?,
    )
}

async fn markdown_folder_ancestry_has_custom_permissions(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    folder_id: Option<Uuid>,
) -> ApiResult<bool> {
    let Some(folder_id) = folder_id else {
        return Ok(false);
    };
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        WITH RECURSIVE folder_ancestors AS (
          SELECT folder.id, folder.parent_id, ARRAY[folder.id] AS path
          FROM markdown_folders folder
          WHERE folder.id = $1 AND folder.project_id = $2

          UNION ALL

          SELECT parent.id, parent.parent_id, ancestors.path || parent.id
          FROM markdown_folders parent
          JOIN folder_ancestors ancestors ON ancestors.parent_id = parent.id
          WHERE parent.project_id = $2
            AND NOT parent.id = ANY(ancestors.path)
        )
        SELECT EXISTS (
          SELECT 1
          FROM folder_ancestors ancestors
          JOIN markdown_folder_department_grants permission
            ON permission.folder_id = ancestors.id
        )
        "#,
    )
    .bind(folder_id)
    .bind(project_id)
    .fetch_one(&mut **transaction)
    .await?)
}

async fn ensure_markdown_move_keeps_permission_boundary(
    transaction: &mut Transaction<'_, Postgres>,
    project_access: &access::ProjectAccess,
    project_id: Uuid,
    old_parent_id: Option<Uuid>,
    new_parent_id: Option<Uuid>,
) -> ApiResult<()> {
    if old_parent_id == new_parent_id || project_access.role.can_manage() {
        return Ok(());
    }
    let source_restricted =
        markdown_folder_ancestry_has_custom_permissions(transaction, project_id, old_parent_id)
            .await?;
    let destination_restricted =
        markdown_folder_ancestry_has_custom_permissions(transaction, project_id, new_parent_id)
            .await?;
    if source_restricted || destination_restricted {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

pub async fn list_folders(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<Vec<MarkdownFolder>>>> {
    let project_access = access::project_access(&state, &current, project_id).await?;
    let rows = sqlx::query_as::<_, MarkdownFolder>(LIST_FOLDERS_SQL)
        .bind(project_id)
        .bind(current.organization_id)
        .bind(current.id)
        .bind(project_access.role.can_manage())
        .bind(project_access.role.can_write())
        .fetch_all(&state.db)
        .await?;
    Ok(ok(rows))
}

pub async fn create_folder(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
    Json(payload): Json<CreateFolderRequest>,
) -> ApiResult<Json<ApiEnvelope<MarkdownFolder>>> {
    let mut transaction = state.db.begin().await?;
    let (actor, project_access) =
        access::lock_active_project_write_context(&mut transaction, &current, project_id).await?;
    lock_markdown_tree(&mut transaction, project_id).await?;
    validate_folder_name(&payload.name)?;
    ensure_markdown_folder_write_in_transaction(
        &mut transaction,
        &actor,
        &project_access,
        project_id,
        payload.parent_id,
    )
    .await?;
    if let Some(parent_id) = payload.parent_id {
        let parent_exists: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM markdown_folders WHERE id = $1 AND project_id = $2")
                .bind(parent_id)
                .bind(project_id)
                .fetch_optional(&mut *transaction)
                .await?;
        if parent_exists.is_none() {
            return Err(ApiError::BadRequest(
                "folder destination is invalid".to_string(),
            ));
        }
    }
    let sort_order: i32 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(MAX(sort_order) + 1, 0)
        FROM markdown_folders
        WHERE project_id = $1 AND parent_id IS NOT DISTINCT FROM $2
        "#,
    )
    .bind(project_id)
    .bind(payload.parent_id)
    .fetch_one(&mut *transaction)
    .await?;
    let name = payload.name.trim().chars().take(240).collect::<String>();
    let row = sqlx::query_as::<_, MarkdownFolder>(
        r#"
        INSERT INTO markdown_folders
          (project_id, parent_id, name, sort_order, created_by)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, project_id, parent_id, name, sort_order, created_by, created_at, updated_at
        "#,
    )
    .bind(project_id)
    .bind(payload.parent_id)
    .bind(name)
    .bind(sort_order)
    .bind(actor.id)
    .fetch_one(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "markdown_folder.create",
        "markdown_folder",
        Some(row.id),
        json!({ "project_id": project_id, "parent_id": row.parent_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn update_folder(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<UpdateFolderRequest>,
) -> ApiResult<Json<ApiEnvelope<MarkdownFolder>>> {
    let mut transaction = state.db.begin().await?;
    let (actor, project_access) =
        access::lock_active_project_write_context(&mut transaction, &current, project_id).await?;
    lock_markdown_tree(&mut transaction, project_id).await?;
    ensure_markdown_folder_write_in_transaction(
        &mut transaction,
        &actor,
        &project_access,
        project_id,
        Some(id),
    )
    .await?;
    validate_folder_name(&payload.name)?;
    let name = payload.name.trim().chars().take(240).collect::<String>();
    let row = sqlx::query_as::<_, MarkdownFolder>(
        r#"
        UPDATE markdown_folders
        SET name = $3, updated_at = now()
        WHERE id = $1 AND project_id = $2
        RETURNING id, project_id, parent_id, name, sort_order, created_by, created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(name)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "markdown_folder.update",
        "markdown_folder",
        Some(row.id),
        json!({ "project_id": project_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn delete_folder(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<ApiEnvelope<DeleteResponse>>> {
    let mut transaction = state.db.begin().await?;
    let (actor, project_access) =
        access::lock_active_project_write_context(&mut transaction, &current, project_id).await?;
    lock_markdown_tree(&mut transaction, project_id).await?;
    ensure_markdown_folder_write_in_transaction(
        &mut transaction,
        &actor,
        &project_access,
        project_id,
        Some(id),
    )
    .await?;
    let existing = sqlx::query_as::<_, (Option<Uuid>, i32)>(
        "SELECT parent_id, sort_order FROM markdown_folders WHERE id = $1 AND project_id = $2 FOR UPDATE",
    )
    .bind(id)
    .bind(project_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    let children: i64 = sqlx::query_scalar(
        r#"
        SELECT
          (SELECT COUNT(*) FROM markdown_folders WHERE project_id = $1 AND parent_id = $2) +
          (SELECT COUNT(*) FROM markdown_docs WHERE project_id = $1 AND folder_id = $2)
        "#,
    )
    .bind(project_id)
    .bind(id)
    .fetch_one(&mut *transaction)
    .await?;
    if children > 0 {
        return Err(ApiError::BadRequest("folder is not empty".to_string()));
    }
    sqlx::query("DELETE FROM markdown_folders WHERE id = $1 AND project_id = $2")
        .bind(id)
        .bind(project_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        r#"
        UPDATE markdown_folders SET sort_order = sort_order - 1
        WHERE project_id = $1 AND parent_id IS NOT DISTINCT FROM $2 AND sort_order > $3
        "#,
    )
    .bind(project_id)
    .bind(existing.0)
    .bind(existing.1)
    .execute(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "markdown_folder.delete",
        "markdown_folder",
        Some(id),
        json!({ "project_id": project_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(DeleteResponse { deleted: true }))
}

pub async fn move_folder(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<MoveFolderRequest>,
) -> ApiResult<Json<ApiEnvelope<MarkdownFolder>>> {
    let mut transaction = state.db.begin().await?;
    let (actor, project_access) =
        access::lock_active_project_write_context(&mut transaction, &current, project_id).await?;
    lock_markdown_tree(&mut transaction, project_id).await?;
    ensure_markdown_folder_write_in_transaction(
        &mut transaction,
        &actor,
        &project_access,
        project_id,
        Some(id),
    )
    .await?;
    let existing = sqlx::query_as::<_, (Option<Uuid>, i32)>(
        "SELECT parent_id, sort_order FROM markdown_folders WHERE id = $1 AND project_id = $2 FOR UPDATE",
    )
    .bind(id)
    .bind(project_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    if let Some(parent_id) = payload.parent_id {
        let parent_exists: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM markdown_folders WHERE id = $1 AND project_id = $2")
                .bind(parent_id)
                .bind(project_id)
                .fetch_optional(&mut *transaction)
                .await?;
        if parent_exists.is_none() {
            return Err(ApiError::BadRequest(
                "folder destination is invalid".to_string(),
            ));
        }
    }
    ensure_markdown_folder_write_in_transaction(
        &mut transaction,
        &actor,
        &project_access,
        project_id,
        payload.parent_id,
    )
    .await?;
    ensure_markdown_move_keeps_permission_boundary(
        &mut transaction,
        &project_access,
        project_id,
        existing.0,
        payload.parent_id,
    )
    .await?;
    let descendant_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        WITH RECURSIVE descendants AS (
          SELECT id FROM markdown_folders WHERE project_id = $2 AND parent_id = $1
          UNION
          SELECT folder.id
          FROM markdown_folders folder
          JOIN descendants ON folder.parent_id = descendants.id
          WHERE folder.project_id = $2
        )
        SELECT id FROM descendants
        "#,
    )
    .bind(id)
    .bind(project_id)
    .fetch_all(&mut *transaction)
    .await?;
    ensure_valid_folder_move(id, payload.parent_id, &descendant_ids)?;
    let sibling_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM markdown_folders
        WHERE project_id = $1 AND parent_id IS NOT DISTINCT FROM $2 AND id <> $3
        "#,
    )
    .bind(project_id)
    .bind(payload.parent_id)
    .bind(id)
    .fetch_one(&mut *transaction)
    .await?;
    let old_position = (existing.0 == payload.parent_id).then_some(existing.1);
    let position = normalize_reordered_position(payload.position, sibling_count, old_position);
    sqlx::query(
        r#"
        UPDATE markdown_folders SET sort_order = sort_order - 1
        WHERE project_id = $1 AND parent_id IS NOT DISTINCT FROM $2
          AND id <> $3 AND sort_order > $4
        "#,
    )
    .bind(project_id)
    .bind(existing.0)
    .bind(id)
    .bind(existing.1)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE markdown_folders SET sort_order = sort_order + 1
        WHERE project_id = $1 AND parent_id IS NOT DISTINCT FROM $2
          AND id <> $3 AND sort_order >= $4
        "#,
    )
    .bind(project_id)
    .bind(payload.parent_id)
    .bind(id)
    .bind(position)
    .execute(&mut *transaction)
    .await?;
    let row = sqlx::query_as::<_, MarkdownFolder>(
        r#"
        UPDATE markdown_folders
        SET parent_id = $3, sort_order = $4, updated_at = now()
        WHERE id = $1 AND project_id = $2
        RETURNING id, project_id, parent_id, name, sort_order, created_by, created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(payload.parent_id)
    .bind(position)
    .fetch_one(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "markdown_folder.move",
        "markdown_folder",
        Some(row.id),
        json!({ "project_id": project_id, "parent_id": row.parent_id, "position": position }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn move_doc(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<MoveDocRequest>,
) -> ApiResult<Json<ApiEnvelope<MarkdownDoc>>> {
    let mut transaction = state.db.begin().await?;
    let (actor, project_access) =
        access::lock_active_project_write_context(&mut transaction, &current, project_id).await?;
    lock_markdown_tree(&mut transaction, project_id).await?;
    ensure_markdown_doc_write_in_transaction(
        &mut transaction,
        &actor,
        &project_access,
        project_id,
        id,
    )
    .await?;
    let existing = sqlx::query_as::<_, (Option<Uuid>, i32)>(
        "SELECT folder_id, sort_order FROM markdown_docs WHERE id = $1 AND project_id = $2 FOR UPDATE",
    )
    .bind(id)
    .bind(project_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    if let Some(folder_id) = payload.folder_id {
        let folder_exists: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM markdown_folders WHERE id = $1 AND project_id = $2")
                .bind(folder_id)
                .bind(project_id)
                .fetch_optional(&mut *transaction)
                .await?;
        if folder_exists.is_none() {
            return Err(ApiError::BadRequest(
                "folder destination is invalid".to_string(),
            ));
        }
    }
    ensure_markdown_folder_write_in_transaction(
        &mut transaction,
        &actor,
        &project_access,
        project_id,
        payload.folder_id,
    )
    .await?;
    ensure_markdown_move_keeps_permission_boundary(
        &mut transaction,
        &project_access,
        project_id,
        existing.0,
        payload.folder_id,
    )
    .await?;
    let sibling_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM markdown_docs
        WHERE project_id = $1 AND folder_id IS NOT DISTINCT FROM $2 AND id <> $3
        "#,
    )
    .bind(project_id)
    .bind(payload.folder_id)
    .bind(id)
    .fetch_one(&mut *transaction)
    .await?;
    let old_position = (existing.0 == payload.folder_id).then_some(existing.1);
    let position = normalize_reordered_position(payload.position, sibling_count, old_position);
    sqlx::query(
        r#"
        UPDATE markdown_docs SET sort_order = sort_order - 1
        WHERE project_id = $1 AND folder_id IS NOT DISTINCT FROM $2
          AND id <> $3 AND sort_order > $4
        "#,
    )
    .bind(project_id)
    .bind(existing.0)
    .bind(id)
    .bind(existing.1)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE markdown_docs SET sort_order = sort_order + 1
        WHERE project_id = $1 AND folder_id IS NOT DISTINCT FROM $2
          AND id <> $3 AND sort_order >= $4
        "#,
    )
    .bind(project_id)
    .bind(payload.folder_id)
    .bind(id)
    .bind(position)
    .execute(&mut *transaction)
    .await?;
    let row = sqlx::query_as::<_, MarkdownDoc>(
        r#"
        UPDATE markdown_docs
        SET folder_id = $3, sort_order = $4, updated_at = now()
        WHERE id = $1 AND project_id = $2
        RETURNING id, project_id, folder_id, title, body_markdown, tags, sort_order,
                  created_by, updated_at
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(payload.folder_id)
    .bind(position)
    .fetch_one(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "markdown_doc.move",
        "markdown_doc",
        Some(row.id),
        json!({ "project_id": project_id, "folder_id": row.folder_id, "position": position }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn list_docs(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<Vec<MarkdownDoc>>>> {
    let project_access = access::project_access(&state, &current, project_id).await?;
    let rows = sqlx::query_as::<_, MarkdownDoc>(LIST_DOCS_SQL)
        .bind(project_id)
        .bind(current.organization_id)
        .bind(current.id)
        .bind(project_access.role.can_manage())
        .bind(project_access.role.can_write())
        .fetch_all(&state.db)
        .await?;
    Ok(ok(rows))
}

pub async fn create_doc(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(project_id): Path<Uuid>,
    Json(payload): Json<CreateDocRequest>,
) -> ApiResult<Json<ApiEnvelope<MarkdownDoc>>> {
    let mut transaction = state.db.begin().await?;
    let (actor, project_access) =
        access::lock_active_project_write_context(&mut transaction, &current, project_id).await?;
    lock_markdown_tree(&mut transaction, project_id).await?;
    validate_doc_input(&payload.title)?;
    ensure_markdown_folder_write_in_transaction(
        &mut transaction,
        &actor,
        &project_access,
        project_id,
        payload.folder_id,
    )
    .await?;
    if let Some(folder_id) = payload.folder_id {
        let folder_exists: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM markdown_folders WHERE id = $1 AND project_id = $2")
                .bind(folder_id)
                .bind(project_id)
                .fetch_optional(&mut *transaction)
                .await?;
        if folder_exists.is_none() {
            return Err(ApiError::BadRequest(
                "folder destination is invalid".to_string(),
            ));
        }
    }
    let sort_order: i32 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(MAX(sort_order) + 1, 0)
        FROM markdown_docs
        WHERE project_id = $1 AND folder_id IS NOT DISTINCT FROM $2
        "#,
    )
    .bind(project_id)
    .bind(payload.folder_id)
    .fetch_one(&mut *transaction)
    .await?;
    let row = sqlx::query_as::<_, MarkdownDoc>(
        r#"
        INSERT INTO markdown_docs
          (project_id, folder_id, title, body_markdown, tags, sort_order, created_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, project_id, folder_id, title, body_markdown, tags, sort_order,
                  created_by, updated_at
        "#,
    )
    .bind(project_id)
    .bind(payload.folder_id)
    .bind(payload.title)
    .bind(payload.body_markdown)
    .bind(payload.tags)
    .bind(sort_order)
    .bind(actor.id)
    .fetch_one(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "markdown_doc.create",
        "markdown_doc",
        Some(row.id),
        json!({ "project_id": project_id, "folder_id": row.folder_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn update_doc(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<UpdateDocRequest>,
) -> ApiResult<Json<ApiEnvelope<MarkdownDoc>>> {
    let mut transaction = state.db.begin().await?;
    let (actor, project_access) =
        access::lock_active_project_write_context(&mut transaction, &current, project_id).await?;
    lock_markdown_tree(&mut transaction, project_id).await?;
    ensure_markdown_doc_write_in_transaction(
        &mut transaction,
        &actor,
        &project_access,
        project_id,
        id,
    )
    .await?;
    validate_doc_input(&payload.title)?;
    let row = sqlx::query_as::<_, MarkdownDoc>(
        r#"
        UPDATE markdown_docs
        SET title = $3, body_markdown = $4, tags = $5, updated_at = now()
        WHERE id = $1 AND project_id = $2
        RETURNING id, project_id, folder_id, title, body_markdown, tags, sort_order,
                  created_by, updated_at
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(payload.title)
    .bind(payload.body_markdown)
    .bind(payload.tags)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "markdown_doc.update",
        "markdown_doc",
        Some(row.id),
        json!({ "project_id": project_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn delete_doc(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<ApiEnvelope<DeleteResponse>>> {
    let mut transaction = state.db.begin().await?;
    let actor = lock_fresh_actor_and_department_tree(&mut transaction, &current).await?;
    access::lock_project_in_transaction(&mut transaction, &actor, project_id, false).await?;
    let project_access =
        access::project_access_in_transaction(&mut transaction, &actor, project_id).await?;
    lock_markdown_tree(&mut transaction, project_id).await?;
    ensure_markdown_doc_write_in_transaction(
        &mut transaction,
        &actor,
        &project_access,
        project_id,
        id,
    )
    .await?;
    let existing = sqlx::query_as::<_, (Option<Uuid>, i32)>(
        "SELECT folder_id, sort_order FROM markdown_docs WHERE id = $1 AND project_id = $2 FOR UPDATE",
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
    .bind(share::TARGET_MARKDOWN_DOC)
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("DELETE FROM markdown_docs WHERE id = $1 AND project_id = $2")
        .bind(id)
        .bind(project_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        r#"
        UPDATE markdown_docs SET sort_order = sort_order - 1
        WHERE project_id = $1 AND folder_id IS NOT DISTINCT FROM $2 AND sort_order > $3
        "#,
    )
    .bind(project_id)
    .bind(existing.0)
    .bind(existing.1)
    .execute(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "markdown_doc.delete",
        "markdown_doc",
        Some(id),
        json!({ "project_id": project_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(DeleteResponse { deleted: true }))
}

impl MarkdownPermissionTarget {
    fn table_name(self) -> &'static str {
        match self {
            Self::Folder => "markdown_folder_department_grants",
            Self::Doc => "markdown_doc_department_grants",
        }
    }

    fn resource_type(self) -> &'static str {
        match self {
            Self::Folder => "markdown_folder",
            Self::Doc => "markdown_doc",
        }
    }

    fn audit_action(self) -> &'static str {
        match self {
            Self::Folder => "markdown_folder.permissions.update",
            Self::Doc => "markdown_doc.permissions.update",
        }
    }
}

async fn ensure_markdown_permission_target_exists(
    transaction: &mut Transaction<'_, Postgres>,
    target: MarkdownPermissionTarget,
    project_id: Uuid,
    id: Uuid,
) -> ApiResult<()> {
    let exists = match target {
        MarkdownPermissionTarget::Folder => {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM markdown_folders WHERE id = $1 AND project_id = $2 FOR UPDATE",
            )
            .bind(id)
            .bind(project_id)
            .fetch_optional(&mut **transaction)
            .await?
        }
        MarkdownPermissionTarget::Doc => {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM markdown_docs WHERE id = $1 AND project_id = $2 FOR UPDATE",
            )
            .bind(id)
            .bind(project_id)
            .fetch_optional(&mut **transaction)
            .await?
        }
    };
    exists.map(|_| ()).ok_or(ApiError::NotFound)
}

async fn list_markdown_department_permissions(
    state: &AppState,
    current: &CurrentUser,
    target: MarkdownPermissionTarget,
    project_id: Uuid,
    id: Uuid,
) -> ApiResult<Vec<MarkdownDepartmentGrantView>> {
    let project_access = access::project_access(state, current, project_id).await?;
    if !project_access.role.can_manage() {
        return Err(ApiError::Forbidden);
    }
    let exists = match target {
        MarkdownPermissionTarget::Folder => {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM markdown_folders WHERE id = $1 AND project_id = $2",
            )
            .bind(id)
            .bind(project_id)
            .fetch_optional(&state.db)
            .await?
        }
        MarkdownPermissionTarget::Doc => {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM markdown_docs WHERE id = $1 AND project_id = $2",
            )
            .bind(id)
            .bind(project_id)
            .fetch_optional(&state.db)
            .await?
        }
    };
    if exists.is_none() {
        return Err(ApiError::NotFound);
    }

    let rows = match target {
        MarkdownPermissionTarget::Folder => {
            sqlx::query_as::<_, MarkdownDepartmentGrantView>(
                r#"
            SELECT permission.department_id, department.name AS department_name,
                   permission.access_role
            FROM markdown_folder_department_grants permission
            JOIN departments department ON department.id = permission.department_id
            WHERE permission.folder_id = $1 AND permission.project_id = $2
              AND permission.organization_id = $3
            ORDER BY department.name, permission.department_id
            "#,
            )
            .bind(id)
            .bind(project_id)
            .bind(current.organization_id)
            .fetch_all(&state.db)
            .await?
        }
        MarkdownPermissionTarget::Doc => {
            sqlx::query_as::<_, MarkdownDepartmentGrantView>(
                r#"
            SELECT permission.department_id, department.name AS department_name,
                   permission.access_role
            FROM markdown_doc_department_grants permission
            JOIN departments department ON department.id = permission.department_id
            WHERE permission.doc_id = $1 AND permission.project_id = $2
              AND permission.organization_id = $3
            ORDER BY department.name, permission.department_id
            "#,
            )
            .bind(id)
            .bind(project_id)
            .bind(current.organization_id)
            .fetch_all(&state.db)
            .await?
        }
    };
    Ok(rows)
}

async fn replace_markdown_department_permissions(
    state: &AppState,
    current: &CurrentUser,
    target: MarkdownPermissionTarget,
    project_id: Uuid,
    id: Uuid,
    payload: ReplaceMarkdownDepartmentPermissionsRequest,
) -> ApiResult<Vec<MarkdownDepartmentGrantView>> {
    let mut seen = HashSet::new();
    for grant in &payload.department_grants {
        if grant.access_role == ProjectRole::Administrator {
            return Err(ApiError::BadRequest(
                "markdown permissions only support viewer or editor".to_string(),
            ));
        }
        if !seen.insert(grant.department_id) {
            return Err(ApiError::BadRequest(
                "markdown permission department scope is invalid".to_string(),
            ));
        }
    }

    let mut transaction = state.db.begin().await?;
    let (actor, project_access) =
        access::lock_active_project_write_context(&mut transaction, current, project_id).await?;
    if !project_access.role.can_manage() {
        return Err(ApiError::Forbidden);
    }
    lock_markdown_tree(&mut transaction, project_id).await?;
    ensure_markdown_permission_target_exists(&mut transaction, target, project_id, id).await?;

    let department_ids = payload
        .department_grants
        .iter()
        .map(|grant| grant.department_id)
        .collect::<Vec<_>>();
    if !department_ids.is_empty() {
        let valid_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM departments WHERE organization_id = $1 AND id = ANY($2)",
        )
        .bind(actor.organization_id)
        .bind(&department_ids)
        .fetch_one(&mut *transaction)
        .await?;
        if valid_count != department_ids.len() as i64 {
            return Err(ApiError::BadRequest(
                "markdown permission department scope is invalid".to_string(),
            ));
        }
    }

    match target {
        MarkdownPermissionTarget::Folder => {
            sqlx::query("DELETE FROM markdown_folder_department_grants WHERE folder_id = $1")
                .bind(id)
                .execute(&mut *transaction)
                .await?;
            for grant in &payload.department_grants {
                sqlx::query(
                    r#"
                    INSERT INTO markdown_folder_department_grants
                      (folder_id, project_id, department_id, organization_id, access_role)
                    VALUES ($1, $2, $3, $4, $5)
                    "#,
                )
                .bind(id)
                .bind(project_id)
                .bind(grant.department_id)
                .bind(actor.organization_id)
                .bind(grant.access_role)
                .execute(&mut *transaction)
                .await?;
            }
        }
        MarkdownPermissionTarget::Doc => {
            sqlx::query("DELETE FROM markdown_doc_department_grants WHERE doc_id = $1")
                .bind(id)
                .execute(&mut *transaction)
                .await?;
            for grant in &payload.department_grants {
                sqlx::query(
                    r#"
                    INSERT INTO markdown_doc_department_grants
                      (doc_id, project_id, department_id, organization_id, access_role)
                    VALUES ($1, $2, $3, $4, $5)
                    "#,
                )
                .bind(id)
                .bind(project_id)
                .bind(grant.department_id)
                .bind(actor.organization_id)
                .bind(grant.access_role)
                .execute(&mut *transaction)
                .await?;
            }
        }
    }

    let rows = match target {
        MarkdownPermissionTarget::Folder => {
            sqlx::query_as::<_, MarkdownDepartmentGrantView>(
                r#"
            SELECT permission.department_id, department.name AS department_name,
                   permission.access_role
            FROM markdown_folder_department_grants permission
            JOIN departments department ON department.id = permission.department_id
            WHERE permission.folder_id = $1
            ORDER BY department.name, permission.department_id
            "#,
            )
            .bind(id)
            .fetch_all(&mut *transaction)
            .await?
        }
        MarkdownPermissionTarget::Doc => {
            sqlx::query_as::<_, MarkdownDepartmentGrantView>(
                r#"
            SELECT permission.department_id, department.name AS department_name,
                   permission.access_role
            FROM markdown_doc_department_grants permission
            JOIN departments department ON department.id = permission.department_id
            WHERE permission.doc_id = $1
            ORDER BY department.name, permission.department_id
            "#,
            )
            .bind(id)
            .fetch_all(&mut *transaction)
            .await?
        }
    };
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        target.audit_action(),
        target.resource_type(),
        Some(id),
        json!({
            "project_id": project_id,
            "department_grants": payload.department_grants.len(),
            "inherit_project": payload.department_grants.is_empty(),
            "grant_table": target.table_name(),
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(rows)
}

pub async fn list_folder_department_permissions(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<ApiEnvelope<Vec<MarkdownDepartmentGrantView>>>> {
    Ok(ok(list_markdown_department_permissions(
        &state,
        &current,
        MarkdownPermissionTarget::Folder,
        project_id,
        id,
    )
    .await?))
}

pub async fn replace_folder_department_permissions(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<ReplaceMarkdownDepartmentPermissionsRequest>,
) -> ApiResult<Json<ApiEnvelope<Vec<MarkdownDepartmentGrantView>>>> {
    Ok(ok(replace_markdown_department_permissions(
        &state,
        &current,
        MarkdownPermissionTarget::Folder,
        project_id,
        id,
        payload,
    )
    .await?))
}

pub async fn list_doc_department_permissions(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<ApiEnvelope<Vec<MarkdownDepartmentGrantView>>>> {
    Ok(ok(list_markdown_department_permissions(
        &state,
        &current,
        MarkdownPermissionTarget::Doc,
        project_id,
        id,
    )
    .await?))
}

pub async fn replace_doc_department_permissions(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<ReplaceMarkdownDepartmentPermissionsRequest>,
) -> ApiResult<Json<ApiEnvelope<Vec<MarkdownDepartmentGrantView>>>> {
    Ok(ok(replace_markdown_department_permissions(
        &state,
        &current,
        MarkdownPermissionTarget::Doc,
        project_id,
        id,
        payload,
    )
    .await?))
}

#[cfg(test)]
mod tests {
    use super::{
        CANCEL_PROJECT_UPLOAD_JOBS_SQL, DELETE_PROJECT_FILE_DELETE_JOBS_SQL, LIST_DOCS_SQL,
        LIST_FOLDERS_SQL, LIST_LINKS_SQL, MARKDOWN_DOC_ACCESS_SQL, MARKDOWN_FOLDER_ACCESS_SQL,
        ensure_last_direct_admin_change, ensure_valid_folder_move, generate_project_slug,
        legacy_role_for_project_role, normalize_move_position, normalize_reordered_position,
        validate_doc_input, validate_folder_name, validate_link_input, validate_project_input,
    };
    use crate::models::{ProjectRole, UserRole};
    use uuid::Uuid;

    #[test]
    fn chinese_only_project_name_generates_ascii_fallback_slug() {
        let slug = generate_project_slug("\u{9879}\u{76ee}\u{5bfc}\u{822a}", "a1b2c3d4");

        assert_eq!(slug, "project-a1b2c3d4");
        assert!(
            slug.chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        );
    }

    #[test]
    fn ascii_project_name_generates_normalized_unique_slug() {
        let first = generate_project_slug("My New Project", "a1b2c3d4");
        let second = generate_project_slug("My New Project", "e5f6a7b8");

        assert_eq!(first, "my-new-project-a1b2c3d4");
        assert_eq!(second, "my-new-project-e5f6a7b8");
        assert_ne!(first, second);
    }

    #[test]
    fn project_roles_map_to_legacy_member_roles() {
        assert_eq!(
            legacy_role_for_project_role(ProjectRole::Administrator),
            UserRole::ProjectAdmin
        );
        assert_eq!(
            legacy_role_for_project_role(ProjectRole::Editor),
            UserRole::ProjectMember
        );
        assert_eq!(
            legacy_role_for_project_role(ProjectRole::Viewer),
            UserRole::ReadonlyMember
        );
    }

    #[test]
    fn permanent_delete_cancels_or_supersedes_project_storage_jobs() {
        assert!(CANCEL_PROJECT_UPLOAD_JOBS_SQL.contains("operation_state = 'cancelled'"));
        assert!(CANCEL_PROJECT_UPLOAD_JOBS_SQL.contains("terminal_at = NULL"));
        assert!(CANCEL_PROJECT_UPLOAD_JOBS_SQL.contains("GREATEST(next_attempt_at"));
        assert!(DELETE_PROJECT_FILE_DELETE_JOBS_SQL.contains("kind = 'delete'"));
    }

    #[test]
    fn project_resource_lists_require_active_projects() {
        for query in [LIST_LINKS_SQL, LIST_FOLDERS_SQL, LIST_DOCS_SQL] {
            assert!(query.contains("project.archived_at IS NULL"));
            assert!(!query.contains("project_members"));
        }
    }

    #[test]
    fn markdown_lists_and_mutations_enforce_department_permissions() {
        for query in [LIST_FOLDERS_SQL, LIST_DOCS_SQL] {
            assert!(query.contains("department_ancestors"));
            assert!(query.contains("has_custom_access"));
            assert!(query.contains("AS can_write"));
        }
        assert!(MARKDOWN_FOLDER_ACCESS_SQL.contains("markdown_folder_department_grants"));
        assert!(MARKDOWN_DOC_ACCESS_SQL.contains("markdown_doc_department_grants"));
        assert!(MARKDOWN_DOC_ACCESS_SQL.contains("markdown_folder_department_grants"));
    }

    #[test]
    fn markdown_permission_schema_guards_project_and_organization_ownership() {
        let migration =
            include_str!("../migrations/202607170007_markdown_department_permissions.sql");
        assert!(migration.contains("FOREIGN KEY (folder_id, project_id)"));
        assert!(migration.contains("FOREIGN KEY (doc_id, project_id)"));
        assert!(migration.contains("FOREIGN KEY (project_id, organization_id)"));
        assert!(migration.contains("FOREIGN KEY (department_id, organization_id)"));
        assert!(migration.contains("CHECK (access_role IN"));
    }

    #[test]
    fn last_direct_admin_policy_blocks_project_admin_and_allows_system_recovery() {
        for requested_role in [None, Some(ProjectRole::Editor)] {
            let error = ensure_last_direct_admin_change(
                false,
                Some(ProjectRole::Administrator),
                requested_role,
                1,
            )
            .unwrap_err();
            assert_eq!(
                error.to_string(),
                "bad request: project must retain at least one direct administrator"
            );

            ensure_last_direct_admin_change(
                true,
                Some(ProjectRole::Administrator),
                requested_role,
                1,
            )
            .unwrap();
        }
    }

    #[test]
    fn project_validation_rejects_blank_name() {
        let error = validate_project_input("  ", "valid-slug").unwrap_err();

        assert_eq!(error.to_string(), "bad request: project name is required");
    }

    #[test]
    fn project_validation_rejects_invalid_slug() {
        let error = validate_project_input("Project", "Invalid Slug").unwrap_err();

        assert_eq!(
            error.to_string(),
            "bad request: project slug may only contain lowercase letters, numbers, and hyphens"
        );
    }

    #[test]
    fn link_validation_rejects_blank_url() {
        let error = validate_link_input("Link", " ").unwrap_err();

        assert_eq!(error.to_string(), "bad request: link url is required");
    }

    #[test]
    fn doc_validation_rejects_blank_title() {
        let error = validate_doc_input(" ").unwrap_err();

        assert_eq!(error.to_string(), "bad request: document title is required");
    }

    #[test]
    fn folder_validation_rejects_blank_name() {
        let error = validate_folder_name("  ").unwrap_err();

        assert_eq!(error.to_string(), "bad request: folder name is required");
    }

    #[test]
    fn folder_move_rejects_itself_as_parent() {
        let folder_id = Uuid::new_v4();
        let error = ensure_valid_folder_move(folder_id, Some(folder_id), &[]).unwrap_err();

        assert_eq!(
            error.to_string(),
            "bad request: folder cannot be moved into itself or a descendant"
        );
    }

    #[test]
    fn folder_move_rejects_descendant_as_parent() {
        let folder_id = Uuid::new_v4();
        let descendant_id = Uuid::new_v4();
        let error =
            ensure_valid_folder_move(folder_id, Some(descendant_id), &[descendant_id]).unwrap_err();

        assert_eq!(
            error.to_string(),
            "bad request: folder cannot be moved into itself or a descendant"
        );
    }

    #[test]
    fn move_position_is_bounded_by_sibling_count() {
        assert_eq!(normalize_move_position(-2, 3), 0);
        assert_eq!(normalize_move_position(2, 3), 2);
        assert_eq!(normalize_move_position(99, 3), 3);
    }

    #[test]
    fn forward_move_accounts_for_the_removed_sibling() {
        assert_eq!(normalize_reordered_position(2, 2, Some(0)), 1);
        assert_eq!(normalize_reordered_position(0, 2, Some(2)), 0);
        assert_eq!(normalize_reordered_position(2, 2, None), 2);
    }
}
