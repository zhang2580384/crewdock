use crate::{app::AppState, error::ApiResult};
use serde_json::Value;
use sqlx::{Executor, Postgres, Transaction};
use uuid::Uuid;

async fn insert_event<'e, E>(
    executor: E,
    organization_id: Uuid,
    actor_id: Option<Uuid>,
    action: &str,
    target_type: &str,
    target_id: Option<Uuid>,
    metadata: Value,
) -> ApiResult<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        INSERT INTO audit_events (organization_id, actor_id, action, target_type, target_id, metadata)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .bind(metadata)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn log_event(
    state: &AppState,
    organization_id: Uuid,
    actor_id: Option<Uuid>,
    action: &str,
    target_type: &str,
    target_id: Option<Uuid>,
    metadata: Value,
) -> ApiResult<()> {
    insert_event(
        &state.db,
        organization_id,
        actor_id,
        action,
        target_type,
        target_id,
        metadata,
    )
    .await
}

pub async fn log_event_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    actor_id: Option<Uuid>,
    action: &str,
    target_type: &str,
    target_id: Option<Uuid>,
    metadata: Value,
) -> ApiResult<()> {
    insert_event(
        &mut **transaction,
        organization_id,
        actor_id,
        action,
        target_type,
        target_id,
        metadata,
    )
    .await
}
