use crate::{app::AppState, auth::hash_password, models::UserRole};
use anyhow::Result;
use serde_json::json;
use uuid::Uuid;

pub async fn bootstrap_admin(state: &AppState) -> Result<()> {
    let org_id = ensure_organization(state).await?;
    let user_exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM users WHERE lower(email) = lower($1)")
            .bind(&state.config.bootstrap_email)
            .fetch_optional(&state.db)
            .await?;
    if user_exists.is_none() {
        let password_hash = hash_password(&state.config.bootstrap_password)?;
        let admin_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO users (organization_id, email, display_name, password_hash, role)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id
            "#,
        )
        .bind(org_id)
        .bind(&state.config.bootstrap_email)
        .bind(&state.config.bootstrap_name)
        .bind(password_hash)
        .bind(UserRole::SystemAdmin)
        .fetch_one(&state.db)
        .await?;

        let project_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO projects (organization_id, name, slug, description, metadata)
            VALUES ($1, '默认项目', 'default', 'crewdock 初始项目空间', $2)
            ON CONFLICT (organization_id, slug) DO UPDATE SET name = excluded.name
            RETURNING id
            "#,
        )
        .bind(org_id)
        .bind(json!({ "bootstrap": true }))
        .fetch_one(&state.db)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO project_members (project_id, user_id, role)
            VALUES ($1, $2, $3)
            ON CONFLICT (project_id, user_id) DO NOTHING
            "#,
        )
        .bind(project_id)
        .bind(admin_id)
        .bind(UserRole::ProjectAdmin)
        .execute(&state.db)
        .await?;
    }
    Ok(())
}

async fn ensure_organization(state: &AppState) -> Result<Uuid> {
    let org_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO organizations (name, slug)
        VALUES ('共舟工作台', 'crewdock')
        ON CONFLICT (slug) DO UPDATE SET name = excluded.name
        RETURNING id
        "#,
    )
    .fetch_one(&state.db)
    .await?;
    Ok(org_id)
}
