use crate::{
    app::AppState,
    auth::{self, CurrentUser},
    error::{ApiError, ApiResult},
    models::{Project, ProjectListItem, ProjectRole, SystemRole},
};
use serde::Serialize;
use sqlx::{Executor, FromRow, Postgres, Transaction};
use uuid::Uuid;

const ORGANIZATION_MUTATION_LOCK_SQL: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended($1::text || ':department-tree', 0))";

pub struct ProjectAccess {
    pub project_id: Uuid,
    pub role: ProjectRole,
}

#[derive(FromRow)]
struct ProjectAccessRow {
    project_id: Uuid,
    role: Option<ProjectRole>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct ProjectAssignableUser {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub department_id: Option<Uuid>,
    pub access_role: ProjectRole,
}

const PROJECT_ASSIGNABLE_USERS_SQL: &str = r#"
    WITH RECURSIVE target_project AS (
      SELECT id
      FROM projects
      WHERE id = $1 AND organization_id = $2 AND archived_at IS NULL
    ),
    department_grant_scope AS (
      SELECT pdg.department_id, pdg.project_role, pdg.include_descendants,
             ARRAY[pdg.department_id] AS path
      FROM project_department_grants pdg
      JOIN target_project target ON target.id = pdg.project_id

      UNION ALL

      SELECT child.id, scope.project_role, scope.include_descendants,
             scope.path || child.id
      FROM departments child
      JOIN department_grant_scope scope ON child.parent_id = scope.department_id
      WHERE scope.include_descendants
        AND child.organization_id = $2
        AND NOT child.id = ANY(scope.path)
    ),
    role_candidates AS (
      SELECT u.id AS user_id, 'administrator'::project_role AS project_role
      FROM users u
      CROSS JOIN target_project
      WHERE u.organization_id = $2
        AND u.is_active = TRUE
        AND u.system_role = 'system_admin'::system_role

      UNION ALL

      SELECT pm.user_id, pm.project_role
      FROM project_members pm
      JOIN target_project target ON target.id = pm.project_id

      UNION ALL

      SELECT membership.user_id, scope.project_role
      FROM department_grant_scope scope
      JOIN user_department_memberships membership
        ON membership.department_id = scope.department_id
       AND membership.organization_id = $2
      JOIN users u ON u.id = membership.user_id
      WHERE u.organization_id = $2 AND u.is_active = TRUE
    ),
    effective_users AS (
      SELECT candidate.user_id, MAX(candidate.project_role) AS access_role
      FROM role_candidates candidate
      GROUP BY candidate.user_id
    )
    SELECT u.id, u.email, u.display_name, u.department_id, effective.access_role
    FROM effective_users effective
    JOIN users u ON u.id = effective.user_id
    WHERE u.organization_id = $2
      AND u.is_active = TRUE
      AND ($3::uuid IS NULL OR u.id = $3)
    ORDER BY u.display_name ASC, u.email ASC, u.id
"#;

pub fn highest_project_role(
    roles: impl IntoIterator<Item = Option<ProjectRole>>,
) -> Option<ProjectRole> {
    roles.into_iter().flatten().max()
}

async fn project_access_with_executor<'e, E>(
    executor: E,
    current: &CurrentUser,
    project_id: Uuid,
    include_archived: bool,
) -> ApiResult<ProjectAccess>
where
    E: Executor<'e, Database = Postgres>,
{
    let is_system_admin = current.system_role == SystemRole::SystemAdmin;
    let row = sqlx::query_as::<_, ProjectAccessRow>(
        r#"
        WITH RECURSIVE target_project AS (
          SELECT id
          FROM projects
          WHERE id = $1
            AND organization_id = $2
            AND ($3 OR archived_at IS NULL)
        ),
        department_ancestors AS (
          SELECT d.id, d.parent_id, 0 AS depth, ARRAY[d.id] AS path
          FROM user_department_memberships membership
          JOIN departments d ON d.id = membership.department_id
          WHERE membership.user_id = $4 AND membership.organization_id = $2

          UNION ALL

          SELECT parent.id, parent.parent_id, ancestors.depth + 1,
                 ancestors.path || parent.id
          FROM departments parent
          JOIN department_ancestors ancestors ON ancestors.parent_id = parent.id
          WHERE parent.organization_id = $2
            AND NOT parent.id = ANY(ancestors.path)
        ),
        role_candidates AS (
          SELECT pm.project_role
          FROM project_members pm
          JOIN target_project target ON target.id = pm.project_id
          WHERE pm.user_id = $5 AND NOT $6

          UNION ALL

          SELECT pdg.project_role
          FROM project_department_grants pdg
          JOIN department_ancestors ancestors ON ancestors.id = pdg.department_id
          JOIN target_project target ON target.id = pdg.project_id
          WHERE (ancestors.depth = 0 OR pdg.include_descendants)
            AND NOT $6
        )
        SELECT target.id AS project_id,
               CASE
                 WHEN $6 THEN 'administrator'::project_role
                 ELSE MAX(candidates.project_role)
               END AS role
        FROM target_project target
        LEFT JOIN role_candidates candidates ON TRUE
        GROUP BY target.id
        "#,
    )
    .bind(project_id)
    .bind(current.organization_id)
    .bind(include_archived)
    .bind(current.id)
    .bind(current.id)
    .bind(is_system_admin)
    .fetch_optional(executor)
    .await?
    .ok_or(ApiError::NotFound)?;

    let role = row.role.ok_or(ApiError::Forbidden)?;
    Ok(ProjectAccess {
        project_id: row.project_id,
        role,
    })
}

pub async fn project_access(
    state: &AppState,
    current: &CurrentUser,
    project_id: Uuid,
) -> ApiResult<ProjectAccess> {
    project_access_with_executor(&state.db, current, project_id, false).await
}

pub(crate) async fn project_access_including_archived(
    state: &AppState,
    current: &CurrentUser,
    project_id: Uuid,
) -> ApiResult<ProjectAccess> {
    project_access_with_executor(&state.db, current, project_id, true).await
}

pub(crate) async fn project_access_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    current: &CurrentUser,
    project_id: Uuid,
) -> ApiResult<ProjectAccess> {
    project_access_with_executor(&mut **transaction, current, project_id, false).await
}

pub(crate) async fn project_access_including_archived_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    current: &CurrentUser,
    project_id: Uuid,
) -> ApiResult<ProjectAccess> {
    project_access_with_executor(&mut **transaction, current, project_id, true).await
}

pub(crate) async fn lock_project_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    current: &CurrentUser,
    project_id: Uuid,
    include_archived: bool,
) -> ApiResult<Project> {
    lock_project_in_transaction_optional(transaction, current, project_id, include_archived)
        .await?
        .ok_or(ApiError::NotFound)
}

pub(crate) async fn lock_project_in_transaction_optional(
    transaction: &mut Transaction<'_, Postgres>,
    current: &CurrentUser,
    project_id: Uuid,
    include_archived: bool,
) -> ApiResult<Option<Project>> {
    Ok(sqlx::query_as::<_, Project>(
        r#"
        SELECT id, organization_id, department_id, name, slug, description, metadata,
               created_at, archived_at
        FROM projects
        WHERE id = $1
          AND organization_id = $2
          AND ($3 OR archived_at IS NULL)
        FOR UPDATE
        "#,
    )
    .bind(project_id)
    .bind(current.organization_id)
    .bind(include_archived)
    .fetch_optional(&mut **transaction)
    .await?)
}

pub(crate) async fn lock_active_project_write_context(
    transaction: &mut Transaction<'_, Postgres>,
    request_actor: &CurrentUser,
    project_id: Uuid,
) -> ApiResult<(CurrentUser, ProjectAccess)> {
    let actor = auth::lock_current_user_in_transaction(transaction, request_actor).await?;
    lock_organization_department_tree(transaction, actor.organization_id).await?;
    lock_project_in_transaction(transaction, &actor, project_id, false).await?;
    let project_access = project_access_in_transaction(transaction, &actor, project_id).await?;
    Ok((actor, project_access))
}

pub async fn visible_projects(
    state: &AppState,
    current: &CurrentUser,
    include_archived: bool,
) -> ApiResult<Vec<ProjectListItem>> {
    if current.system_role == SystemRole::SystemAdmin {
        return Ok(sqlx::query_as::<_, ProjectListItem>(
            r#"
            SELECT id, organization_id, department_id, name, slug, description, metadata,
                   created_at, archived_at,
                   'administrator'::project_role AS access_role,
                   TRUE AS can_write
            FROM projects
            WHERE organization_id = $1 AND ($2 OR archived_at IS NULL)
            ORDER BY created_at DESC, id
            "#,
        )
        .bind(current.organization_id)
        .bind(include_archived)
        .fetch_all(&state.db)
        .await?);
    }

    Ok(sqlx::query_as::<_, ProjectListItem>(
        r#"
        WITH RECURSIVE department_ancestors AS (
          SELECT d.id, d.parent_id, 0 AS depth, ARRAY[d.id] AS path
          FROM user_department_memberships membership
          JOIN departments d ON d.id = membership.department_id
          WHERE membership.user_id = $3 AND membership.organization_id = $2

          UNION ALL

          SELECT parent.id, parent.parent_id, ancestors.depth + 1,
                 ancestors.path || parent.id
          FROM departments parent
          JOIN department_ancestors ancestors ON ancestors.parent_id = parent.id
          WHERE parent.organization_id = $2
            AND NOT parent.id = ANY(ancestors.path)
        ),
        role_candidates AS (
          SELECT pm.project_id, pm.project_role
          FROM project_members pm
          JOIN projects p ON p.id = pm.project_id
          WHERE pm.user_id = $1
            AND p.organization_id = $2
            AND ($4 OR p.archived_at IS NULL)

          UNION ALL

          SELECT pdg.project_id, pdg.project_role
          FROM project_department_grants pdg
          JOIN department_ancestors ancestors ON ancestors.id = pdg.department_id
          JOIN projects p ON p.id = pdg.project_id
          WHERE p.organization_id = $2
            AND (ancestors.depth = 0 OR pdg.include_descendants)
            AND ($4 OR p.archived_at IS NULL)
        ),
        effective_roles AS (
          SELECT project_id, MAX(project_role) AS access_role
          FROM role_candidates
          GROUP BY project_id
        )
        SELECT p.id, p.organization_id, p.department_id, p.name, p.slug, p.description,
               p.metadata, p.created_at, p.archived_at, effective.access_role,
               (effective.access_role >= 'editor'::project_role) AS can_write
        FROM effective_roles effective
        JOIN projects p ON p.id = effective.project_id
        WHERE p.organization_id = $2
          AND ($4 OR p.archived_at IS NULL)
        ORDER BY p.created_at DESC, p.id
        "#,
    )
    .bind(current.id)
    .bind(current.organization_id)
    .bind(current.id)
    .bind(include_archived)
    .fetch_all(&state.db)
    .await?)
}

pub(crate) async fn visible_project_ids(
    state: &AppState,
    current: &CurrentUser,
) -> ApiResult<Vec<Uuid>> {
    Ok(visible_projects(state, current, false)
        .await?
        .into_iter()
        .map(|project| project.id)
        .collect())
}

pub(crate) async fn manageable_project_ids(
    state: &AppState,
    current: &CurrentUser,
) -> ApiResult<Vec<Uuid>> {
    Ok(visible_projects(state, current, false)
        .await?
        .into_iter()
        .filter(|project| project.access_role.can_manage())
        .map(|project| project.id)
        .collect())
}

async fn project_assignable_users_with_executor<'e, E>(
    executor: E,
    organization_id: Uuid,
    project_id: Uuid,
    user_id: Option<Uuid>,
) -> ApiResult<Vec<ProjectAssignableUser>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(
        sqlx::query_as::<_, ProjectAssignableUser>(PROJECT_ASSIGNABLE_USERS_SQL)
            .bind(project_id)
            .bind(organization_id)
            .bind(user_id)
            .fetch_all(executor)
            .await?,
    )
}

pub async fn project_assignable_users(
    state: &AppState,
    current: &CurrentUser,
    project_id: Uuid,
) -> ApiResult<Vec<ProjectAssignableUser>> {
    project_access(state, current, project_id).await?;
    project_assignable_users_with_executor(&state.db, current.organization_id, project_id, None)
        .await
}

pub(crate) async fn is_project_assignable_user_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
) -> ApiResult<bool> {
    Ok(project_assignable_users_with_executor(
        &mut **transaction,
        organization_id,
        project_id,
        Some(user_id),
    )
    .await?
    .into_iter()
    .next()
    .is_some())
}

pub(crate) async fn lock_organization_department_tree(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
) -> ApiResult<()> {
    sqlx::query(ORGANIZATION_MUTATION_LOCK_SQL)
        .bind(organization_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn manages_department_with_executor<'e, E>(
    executor: E,
    current: &CurrentUser,
    department_id: Uuid,
) -> ApiResult<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query_scalar::<_, bool>(
        r#"
        WITH RECURSIVE target_department AS (
          SELECT id, parent_id
          FROM departments
          WHERE id = $2 AND organization_id = $3
        ),
        department_ancestors AS (
          SELECT target.id, target.parent_id, ARRAY[target.id] AS path
          FROM target_department target

          UNION ALL

          SELECT parent.id, parent.parent_id, ancestors.path || parent.id
          FROM departments parent
          JOIN department_ancestors ancestors ON ancestors.parent_id = parent.id
          WHERE parent.organization_id = $3
            AND NOT parent.id = ANY(ancestors.path)
        )
        SELECT CASE
          WHEN $4 THEN TRUE
          ELSE EXISTS (
            SELECT 1
            FROM department_ancestors ancestors
            JOIN department_admins admin ON admin.department_id = ancestors.id
            WHERE admin.user_id = $1
          )
        END
        FROM target_department
        "#,
    )
    .bind(current.id)
    .bind(department_id)
    .bind(current.organization_id)
    .bind(current.system_role == SystemRole::SystemAdmin)
    .fetch_optional(executor)
    .await?
    .ok_or(ApiError::NotFound)
}

pub async fn manages_department(
    state: &AppState,
    current: &CurrentUser,
    department_id: Uuid,
) -> ApiResult<bool> {
    manages_department_with_executor(&state.db, current, department_id).await
}

pub async fn belongs_to_department(
    state: &AppState,
    current: &CurrentUser,
    department_id: Uuid,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
          SELECT 1
          FROM user_department_memberships membership
          WHERE membership.user_id = $1
            AND membership.department_id = $2
            AND membership.organization_id = $3
        )
        "#,
    )
    .bind(current.id)
    .bind(department_id)
    .bind(current.organization_id)
    .fetch_one(&state.db)
    .await?)
}

pub(crate) async fn manages_department_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    current: &CurrentUser,
    department_id: Uuid,
) -> ApiResult<bool> {
    manages_department_with_executor(&mut **transaction, current, department_id).await
}

#[cfg(test)]
mod tests {
    use super::{
        ORGANIZATION_MUTATION_LOCK_SQL, PROJECT_ASSIGNABLE_USERS_SQL, highest_project_role,
    };
    use crate::models::ProjectRole;

    #[test]
    fn highest_role_prefers_direct_editor_over_department_viewer() {
        assert_eq!(
            highest_project_role([Some(ProjectRole::Editor), Some(ProjectRole::Viewer)]),
            Some(ProjectRole::Editor)
        );
    }

    #[test]
    fn highest_role_returns_none_without_grants() {
        assert_eq!(highest_project_role([None, None]), None);
    }

    #[test]
    fn highest_role_accepts_department_only_grant() {
        assert_eq!(
            highest_project_role([None, Some(ProjectRole::Editor)]),
            Some(ProjectRole::Editor)
        );
    }

    #[test]
    fn assignable_users_include_all_effective_access_sources() {
        assert!(PROJECT_ASSIGNABLE_USERS_SQL.contains("archived_at IS NULL"));
        assert!(PROJECT_ASSIGNABLE_USERS_SQL.contains("u.organization_id = $2"));
        assert!(PROJECT_ASSIGNABLE_USERS_SQL.contains("u.is_active = TRUE"));
        assert!(PROJECT_ASSIGNABLE_USERS_SQL.contains("u.system_role = 'system_admin'"));
        assert!(PROJECT_ASSIGNABLE_USERS_SQL.contains("FROM project_members pm"));
        assert!(PROJECT_ASSIGNABLE_USERS_SQL.contains("FROM project_department_grants pdg"));
        assert!(PROJECT_ASSIGNABLE_USERS_SQL.contains("JOIN department_grant_scope scope"));
        assert!(PROJECT_ASSIGNABLE_USERS_SQL.contains("scope.include_descendants"));
        assert!(PROJECT_ASSIGNABLE_USERS_SQL.contains("MAX(candidate.project_role)"));
    }

    #[test]
    fn organization_mutations_use_a_transaction_scoped_advisory_lock() {
        assert!(ORGANIZATION_MUTATION_LOCK_SQL.contains("pg_advisory_xact_lock"));
        assert!(ORGANIZATION_MUTATION_LOCK_SQL.contains("department-tree"));
    }
}
