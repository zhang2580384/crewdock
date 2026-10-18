use crate::{
    access,
    app::AppState,
    audit::log_event_in_transaction,
    auth::{self, CurrentUser},
    error::{ApiError, ApiResult},
    models::{Ticket, TicketStatus},
    response::{ApiEnvelope, ok},
    share,
};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct CreateTicketRequest {
    pub project_id: Uuid,
    pub title: String,
    pub description_markdown: String,
    pub assignee_id: Option<Uuid>,
}

#[derive(Deserialize)]
pub struct UpdateTicketStatusRequest {
    pub status: TicketStatus,
}

#[derive(Deserialize)]
pub struct UpdateTicketRequest {
    pub title: String,
    pub description_markdown: String,
    pub status: TicketStatus,
    pub assignee_id: Option<Uuid>,
}

#[derive(serde::Serialize)]
pub struct DeleteTicketResponse {
    pub deleted: bool,
}

const LIST_TICKETS_SQL: &str = r#"
    SELECT ticket.id, ticket.project_id, ticket.title, ticket.description_markdown,
           ticket.status, ticket.assignee_id, ticket.created_by, ticket.updated_at
    FROM tickets ticket
    JOIN projects project ON project.id = ticket.project_id
    WHERE project.organization_id = $1 AND project.archived_at IS NULL
      AND ticket.project_id = ANY($2)
    ORDER BY ticket.updated_at DESC
"#;

fn validate_ticket_input(title: &str) -> ApiResult<()> {
    if title.trim().is_empty() {
        return Err(ApiError::BadRequest("ticket title is required".to_string()));
    }
    Ok(())
}

async fn validate_assignee_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    organization_id: Uuid,
    project_id: Uuid,
    assignee_id: Option<Uuid>,
) -> ApiResult<()> {
    let Some(assignee_id) = assignee_id else {
        return Ok(());
    };
    let valid = access::is_project_assignable_user_in_transaction(
        transaction,
        organization_id,
        project_id,
        assignee_id,
    )
    .await?;
    if !valid {
        return Err(ApiError::BadRequest(
            "assignee must have effective access to the project".to_string(),
        ));
    }
    Ok(())
}

pub async fn list_tickets(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<Vec<Ticket>>>> {
    let project_ids = access::visible_project_ids(&state, &current).await?;
    let rows = sqlx::query_as::<_, Ticket>(LIST_TICKETS_SQL)
        .bind(current.organization_id)
        .bind(&project_ids)
        .fetch_all(&state.db)
        .await?;
    Ok(ok(rows))
}

pub async fn create_ticket(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(payload): Json<CreateTicketRequest>,
) -> ApiResult<Json<ApiEnvelope<Ticket>>> {
    let mut transaction = state.db.begin().await?;
    let (actor, project_access) =
        access::lock_active_project_write_context(&mut transaction, &current, payload.project_id)
            .await?;
    if !project_access.role.can_write() {
        return Err(ApiError::Forbidden);
    }
    validate_ticket_input(&payload.title)?;
    validate_assignee_in_transaction(
        &mut transaction,
        actor.organization_id,
        payload.project_id,
        payload.assignee_id,
    )
    .await?;
    let row = sqlx::query_as::<_, Ticket>(
        r#"
        INSERT INTO tickets (project_id, title, description_markdown, assignee_id, created_by)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, project_id, title, description_markdown, status, assignee_id, created_by, updated_at
        "#,
    )
    .bind(payload.project_id)
    .bind(payload.title)
    .bind(payload.description_markdown)
    .bind(payload.assignee_id)
    .bind(actor.id)
    .fetch_one(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "ticket.create",
        "ticket",
        Some(row.id),
        json!({ "project_id": row.project_id, "title": row.title }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn update_ticket(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateTicketRequest>,
) -> ApiResult<Json<ApiEnvelope<Ticket>>> {
    let mut transaction = state.db.begin().await?;
    let (actor, project_id, project_access) =
        lock_ticket_write_context(&mut transaction, &current, id).await?;
    if !project_access.role.can_write() {
        return Err(ApiError::Forbidden);
    }
    validate_ticket_input(&payload.title)?;
    validate_assignee_in_transaction(
        &mut transaction,
        actor.organization_id,
        project_id,
        payload.assignee_id,
    )
    .await?;
    let row = sqlx::query_as::<_, Ticket>(
        r#"
        UPDATE tickets
        SET title = $2, description_markdown = $3, status = $4,
            assignee_id = $5, updated_at = now()
        WHERE id = $1 AND project_id = $6
        RETURNING id, project_id, title, description_markdown, status, assignee_id, created_by, updated_at
        "#,
    )
    .bind(id)
    .bind(payload.title)
    .bind(payload.description_markdown)
    .bind(payload.status)
    .bind(payload.assignee_id)
    .bind(project_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "ticket.update",
        "ticket",
        Some(row.id),
        json!({ "project_id": project_id, "status": row.status, "assignee_id": row.assignee_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn delete_ticket(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<DeleteTicketResponse>>> {
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    access::lock_organization_department_tree(&mut transaction, actor.organization_id).await?;
    let project_id: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT ticket.project_id
        FROM tickets ticket
        JOIN projects project ON project.id = ticket.project_id
        WHERE ticket.id = $1 AND project.organization_id = $2
        "#,
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
    let target: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM tickets WHERE id = $1 AND project_id = $2 FOR UPDATE")
            .bind(id)
            .bind(project_id)
            .fetch_optional(&mut *transaction)
            .await?;
    target.ok_or(ApiError::NotFound)?;
    sqlx::query(
        "DELETE FROM share_links WHERE organization_id = $1 AND target_type = $2 AND target_id = $3",
    )
    .bind(actor.organization_id)
    .bind(share::TARGET_TICKET)
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    let deleted: Option<Uuid> =
        sqlx::query_scalar("DELETE FROM tickets WHERE id = $1 AND project_id = $2 RETURNING id")
            .bind(id)
            .bind(project_id)
            .fetch_optional(&mut *transaction)
            .await?;
    deleted.ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "ticket.delete",
        "ticket",
        Some(id),
        json!({ "project_id": project_id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(DeleteTicketResponse { deleted: true }))
}

async fn lock_ticket_write_context(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_actor: &CurrentUser,
    id: Uuid,
) -> ApiResult<(CurrentUser, Uuid, crate::access::ProjectAccess)> {
    let actor = auth::lock_current_user_in_transaction(transaction, request_actor).await?;
    access::lock_organization_department_tree(transaction, actor.organization_id).await?;
    let project_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT t.project_id FROM tickets t JOIN projects p ON p.id = t.project_id WHERE t.id = $1 AND p.organization_id = $2",
    )
    .bind(id)
    .bind(actor.organization_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let project_id = project_id.ok_or(ApiError::NotFound)?;
    access::lock_project_in_transaction(transaction, &actor, project_id, false).await?;
    let project_access =
        access::project_access_in_transaction(transaction, &actor, project_id).await?;
    Ok((actor, project_id, project_access))
}

pub async fn update_ticket_status(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateTicketStatusRequest>,
) -> ApiResult<Json<ApiEnvelope<Ticket>>> {
    let mut transaction = state.db.begin().await?;
    let (actor, _project_id, project_access) =
        lock_ticket_write_context(&mut transaction, &current, id).await?;
    if !project_access.role.can_write() {
        return Err(ApiError::Forbidden);
    }
    let row = sqlx::query_as::<_, Ticket>(
        r#"
        UPDATE tickets
        SET status = $2, updated_at = now()
        WHERE id = $1
        RETURNING id, project_id, title, description_markdown, status, assignee_id, created_by, updated_at
        "#,
    )
    .bind(id)
    .bind(payload.status)
    .fetch_one(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "ticket.status_update",
        "ticket",
        Some(row.id),
        json!({ "status": row.status }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

#[cfg(test)]
mod tests {
    use super::{LIST_TICKETS_SQL, validate_ticket_input};

    #[test]
    fn ticket_validation_rejects_blank_title() {
        let error = validate_ticket_input(" ").unwrap_err();

        assert_eq!(error.to_string(), "bad request: ticket title is required");
    }

    #[test]
    fn ticket_list_uses_effective_visible_active_projects() {
        assert!(LIST_TICKETS_SQL.contains("project.archived_at IS NULL"));
        assert!(LIST_TICKETS_SQL.contains("ticket.project_id = ANY($2)"));
        assert!(!LIST_TICKETS_SQL.contains("project_members"));
    }
}
