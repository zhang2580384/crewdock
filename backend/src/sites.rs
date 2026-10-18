use crate::{
    access,
    app::AppState,
    audit::{log_event, log_event_in_transaction},
    auth::{self, CurrentUser},
    error::{ApiError, ApiResult},
    models::{Site, SiteVisibility, SystemRole},
    response::{ApiEnvelope, ok},
    site_icons,
};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct CreateSiteRequest {
    pub visibility: Option<SiteVisibility>,
    pub category: Option<String>,
    pub title: String,
    pub url: String,
    pub resource_type: Option<String>,
    pub connection_config: Option<Value>,
    pub internal_url: Option<String>,
    pub description: Option<String>,
    pub icon_url: Option<String>,
    pub icon_text: Option<String>,
    pub tags: Option<Vec<String>>,
    pub is_pinned: Option<bool>,
    pub department_ids: Option<Vec<Uuid>>,
}

#[derive(Deserialize)]
pub struct UpdateSiteRequest {
    pub visibility: Option<SiteVisibility>,
    pub category: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub resource_type: Option<String>,
    pub connection_config: Option<Value>,
    pub internal_url: Option<String>,
    pub description: Option<String>,
    pub icon_url: Option<String>,
    pub icon_text: Option<String>,
    pub tags: Option<Vec<String>>,
    pub is_pinned: Option<bool>,
    pub department_ids: Option<Vec<Uuid>>,
}

#[derive(Deserialize)]
pub struct ImportBookmarksRequest {
    pub html: String,
    pub visibility: Option<SiteVisibility>,
    pub category: Option<String>,
    pub department_ids: Option<Vec<Uuid>>,
}

#[derive(Serialize)]
pub struct ImportBookmarksResponse {
    pub imported: usize,
    pub skipped: usize,
    pub sites: Vec<Site>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct SiteActivityResponse {
    pub open_count: i64,
    pub last_opened_at: chrono::DateTime<chrono::Utc>,
}

pub async fn list_sites(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<Vec<Site>>>> {
    let rows = sqlx::query_as::<_, Site>(
        r#"
        WITH RECURSIVE visible_department_scope AS (
          SELECT site_grant.site_id, site_grant.department_id, ARRAY[site_grant.department_id] AS path
          FROM site_department_grants site_grant
          WHERE site_grant.organization_id = $1

          UNION ALL

          SELECT scope.site_id, child.id, scope.path || child.id
          FROM visible_department_scope scope
          JOIN departments child ON child.parent_id = scope.department_id
          WHERE child.organization_id = $1
            AND NOT child.id = ANY(scope.path)
        )
        SELECT site.id, site.organization_id, site.owner_id, site.visibility, site.category,
               site.title, site.resource_type, site.connection_config, site.url, site.internal_url,
               site.description, site.icon_url, site.icon_text, site.auto_icon_url,
               site.icon_fetched_at, site.tags,
               site.is_pinned, site.sort_order,
               COALESCE(
                 (SELECT activity.open_count FROM site_user_activity activity
                  WHERE activity.site_id = site.id AND activity.user_id = $2),
                 0
               ) AS open_count,
               (SELECT activity.last_opened_at FROM site_user_activity activity
                WHERE activity.site_id = site.id AND activity.user_id = $2) AS last_opened_at,
               COALESCE(
                 (SELECT array_agg(site_grant.department_id ORDER BY site_grant.department_id)
                  FROM site_department_grants site_grant WHERE site_grant.site_id = site.id),
                 ARRAY[]::uuid[]
               ) AS department_ids,
               site.created_by, site.created_at, site.updated_at
        FROM sites site
        WHERE site.organization_id = $1
          AND (
            (site.visibility = 'personal' AND site.owner_id = $2)
            OR (
              site.visibility = 'public'
              AND (
                $3
                OR NOT EXISTS (SELECT 1 FROM site_department_grants site_grant WHERE site_grant.site_id = site.id)
                OR EXISTS (
                  SELECT 1
                  FROM visible_department_scope scope
                  JOIN user_department_memberships membership
                    ON membership.department_id = scope.department_id
                   AND membership.organization_id = $1
                  WHERE scope.site_id = site.id AND membership.user_id = $4
                )
              )
            )
          )
        ORDER BY site.is_pinned DESC, site.category ASC, site.sort_order ASC, site.updated_at DESC
        "#,
    )
    .bind(current.organization_id)
    .bind(current.id)
    .bind(current.system_role == SystemRole::SystemAdmin)
    .bind(current.id)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(rows))
}

pub async fn create_site(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(payload): Json<CreateSiteRequest>,
) -> ApiResult<Json<ApiEnvelope<Site>>> {
    let visibility = payload
        .visibility
        .clone()
        .unwrap_or(SiteVisibility::Personal);
    let department_ids = validate_department_scope(
        &state,
        &current,
        &visibility,
        payload.department_ids.clone().unwrap_or_default(),
    )
    .await?;
    let site = insert_site(
        &state,
        &current,
        visibility,
        payload.into(),
        &department_ids,
    )
    .await?;
    log_event(
        &state,
        current.organization_id,
        Some(current.id),
        "site.create",
        "site",
        Some(site.id),
        json!({ "title": site.title, "visibility": site.visibility }),
    )
    .await?;
    Ok(ok(site))
}

pub async fn record_site_open(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<SiteActivityResponse>>> {
    let activity = sqlx::query_as::<_, SiteActivityResponse>(
        r#"
        WITH RECURSIVE visible_department_scope AS (
          SELECT site_grant.site_id, site_grant.department_id, ARRAY[site_grant.department_id] AS path
          FROM site_department_grants site_grant
          WHERE site_grant.organization_id = $1

          UNION ALL

          SELECT scope.site_id, child.id, scope.path || child.id
          FROM visible_department_scope scope
          JOIN departments child ON child.parent_id = scope.department_id
          WHERE child.organization_id = $1
            AND NOT child.id = ANY(scope.path)
        ),
        visible_site AS (
          SELECT site.id
          FROM sites site
          WHERE site.id = $5
            AND site.organization_id = $1
            AND (
              (site.visibility = 'personal' AND site.owner_id = $2)
              OR (
                site.visibility = 'public'
                AND (
                  $3
                  OR NOT EXISTS (
                    SELECT 1 FROM site_department_grants site_grant
                    WHERE site_grant.site_id = site.id
                  )
                  OR EXISTS (
                    SELECT 1
                    FROM visible_department_scope scope
                    JOIN user_department_memberships membership
                      ON membership.department_id = scope.department_id
                     AND membership.organization_id = $1
                    WHERE scope.site_id = site.id AND membership.user_id = $4
                  )
                )
              )
            )
        )
        INSERT INTO site_user_activity (site_id, user_id, open_count, last_opened_at)
        SELECT visible_site.id, $2, 1, now()
        FROM visible_site
        ON CONFLICT (site_id, user_id) DO UPDATE
        SET open_count = site_user_activity.open_count + 1,
            last_opened_at = now()
        RETURNING open_count, last_opened_at
        "#,
    )
    .bind(current.organization_id)
    .bind(current.id)
    .bind(current.system_role == SystemRole::SystemAdmin)
    .bind(current.id)
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    Ok(ok(activity))
}

pub async fn update_site(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateSiteRequest>,
) -> ApiResult<Json<ApiEnvelope<Site>>> {
    let existing = load_site_for_write(&state, &current, id).await?;
    let visibility = payload
        .visibility
        .clone()
        .unwrap_or_else(|| existing.visibility.clone());
    let department_ids = match &visibility {
        SiteVisibility::Personal => Vec::new(),
        SiteVisibility::Public if payload.department_ids.is_some() => {
            validate_department_scope(
                &state,
                &current,
                &visibility,
                payload.department_ids.clone().unwrap_or_default(),
            )
            .await?
        }
        SiteVisibility::Public if existing.visibility == SiteVisibility::Public => {
            existing.department_ids.clone()
        }
        SiteVisibility::Public => {
            validate_department_scope(&state, &current, &visibility, Vec::new()).await?
        }
    };
    let owner_id = owner_for_visibility(&current, &visibility);
    let title = payload.title.unwrap_or_else(|| existing.title.clone());
    let resource_type = normalize_resource_type(
        payload
            .resource_type
            .as_deref()
            .unwrap_or(&existing.resource_type),
    )?;
    let raw_url = payload.url.unwrap_or_else(|| existing.url.clone());
    let raw_connection_config = payload
        .connection_config
        .unwrap_or_else(|| existing.connection_config.clone());
    let category = payload
        .category
        .unwrap_or_else(|| existing.category.clone());
    let raw_internal_url = payload
        .internal_url
        .or_else(|| existing.internal_url.clone());
    let (url, internal_url, connection_config) = normalize_site_target(
        &resource_type,
        &raw_url,
        raw_internal_url,
        raw_connection_config,
    )?;
    let clear_auto_icon = resource_type != existing.resource_type || url != existing.url;

    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    access::lock_organization_department_tree(&mut transaction, actor.organization_id).await?;
    ensure_site_write_scope_in_transaction(
        &mut transaction,
        &actor,
        &existing.visibility,
        existing.owner_id,
        &existing.department_ids,
    )
    .await?;
    ensure_site_write_scope_in_transaction(
        &mut transaction,
        &actor,
        &visibility,
        owner_id,
        &department_ids,
    )
    .await?;
    sqlx::query(
        r#"
        UPDATE sites
        SET owner_id = $1,
            visibility = $2,
            category = $3,
            title = $4,
            resource_type = $5,
            connection_config = $6,
            url = $7,
            internal_url = $8,
            description = $9,
            icon_url = $10,
            icon_text = $11,
            auto_icon_url = CASE WHEN $12 THEN NULL ELSE auto_icon_url END,
            icon_fetched_at = CASE WHEN $12 THEN NULL ELSE icon_fetched_at END,
            tags = $13,
            is_pinned = $14,
            updated_at = now()
        WHERE id = $15 AND organization_id = $16
        "#,
    )
    .bind(owner_id)
    .bind(visibility.clone())
    .bind(clean_text(&category).unwrap_or_else(|| "未分组".to_string()))
    .bind(required_text(&title, "title")?)
    .bind(resource_type)
    .bind(connection_config)
    .bind(url)
    .bind(internal_url)
    .bind(
        payload
            .description
            .or_else(|| existing.description.clone())
            .and_then(|v| clean_text(&v)),
    )
    .bind(
        payload
            .icon_url
            .or_else(|| existing.icon_url.clone())
            .and_then(|v| clean_text(&v)),
    )
    .bind(normalize_icon_text(
        payload.icon_text.or_else(|| existing.icon_text.clone()),
    )?)
    .bind(clear_auto_icon)
    .bind(clean_tags(
        payload.tags.unwrap_or_else(|| existing.tags.clone()),
    ))
    .bind(payload.is_pinned.unwrap_or(existing.is_pinned))
    .bind(id)
    .bind(actor.organization_id)
    .execute(&mut *transaction)
    .await?;
    sync_site_department_grants(&mut transaction, actor.organization_id, id, &department_ids)
        .await?;
    transaction.commit().await?;
    let row = load_site(&state, actor.organization_id, actor.id, id).await?;
    log_event(
        &state,
        actor.organization_id,
        Some(actor.id),
        "site.update",
        "site",
        Some(row.id),
        json!({ "title": row.title }),
    )
    .await?;
    Ok(ok(row))
}

pub async fn refresh_site_icon(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<Site>>> {
    let site = load_site_for_write(&state, &current, id).await?;
    if site.resource_type != "website" {
        return Err(ApiError::BadRequest(
            "only website sites have fetched icons".to_string(),
        ));
    }

    let icon = site_icons::fetch_site_icon(&site.url, &state.config.icon_private_origins).await;
    let icon_found = icon.is_some();
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    access::lock_organization_department_tree(&mut transaction, actor.organization_id).await?;
    ensure_site_write_scope_in_transaction(
        &mut transaction,
        &actor,
        &site.visibility,
        site.owner_id,
        &site.department_ids,
    )
    .await?;
    sqlx::query(
        r#"
        UPDATE sites
        SET auto_icon_url = COALESCE($1, auto_icon_url),
            icon_fetched_at = now()
        WHERE id = $2
          AND organization_id = $3
          AND resource_type = 'website'
          AND url = $4
        "#,
    )
    .bind(icon)
    .bind(id)
    .bind(actor.organization_id)
    .bind(&site.url)
    .execute(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "site.icon.refresh",
        "site",
        Some(id),
        json!({ "icon_found": icon_found }),
    )
    .await?;
    transaction.commit().await?;

    Ok(ok(
        load_site(&state, actor.organization_id, actor.id, id).await?
    ))
}

pub async fn delete_site(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<serde_json::Value>>> {
    let existing = load_site_for_write(&state, &current, id).await?;
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    access::lock_organization_department_tree(&mut transaction, actor.organization_id).await?;
    ensure_site_write_scope_in_transaction(
        &mut transaction,
        &actor,
        &existing.visibility,
        existing.owner_id,
        &existing.department_ids,
    )
    .await?;
    sqlx::query("DELETE FROM sites WHERE id = $1 AND organization_id = $2")
        .bind(id)
        .bind(actor.organization_id)
        .execute(&mut *transaction)
        .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "site.delete",
        "site",
        Some(id),
        json!({ "title": existing.title }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(json!({ "deleted": true })))
}

pub async fn import_bookmarks(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(payload): Json<ImportBookmarksRequest>,
) -> ApiResult<Json<ApiEnvelope<ImportBookmarksResponse>>> {
    let visibility = payload.visibility.unwrap_or(SiteVisibility::Personal);
    let department_ids = validate_department_scope(
        &state,
        &current,
        &visibility,
        payload.department_ids.unwrap_or_default(),
    )
    .await?;
    let default_category = payload.category.and_then(|v| clean_text(&v));
    let bookmarks = parse_bookmarks(&payload.html, default_category.as_deref())?;
    if bookmarks.is_empty() {
        return Err(ApiError::BadRequest("no bookmarks found".to_string()));
    }

    let mut imported = Vec::new();
    let mut skipped = 0usize;
    for bookmark in bookmarks.into_iter().take(500) {
        if site_exists(
            &state,
            &current,
            &visibility,
            &bookmark.url,
            &department_ids,
        )
        .await?
        {
            skipped += 1;
            continue;
        }
        let req = InsertSite {
            category: Some(bookmark.category),
            title: bookmark.title,
            url: bookmark.url,
            resource_type: "website".to_string(),
            connection_config: json!({}),
            internal_url: None,
            description: None,
            icon_url: None,
            icon_text: None,
            tags: vec!["导入".to_string()],
            is_pinned: false,
        };
        imported
            .push(insert_site(&state, &current, visibility.clone(), req, &department_ids).await?);
    }
    log_event(
        &state,
        current.organization_id,
        Some(current.id),
        "site.import_bookmarks",
        "site",
        None,
        json!({ "imported": imported.len(), "skipped": skipped, "visibility": visibility }),
    )
    .await?;
    Ok(ok(ImportBookmarksResponse {
        imported: imported.len(),
        skipped,
        sites: imported,
    }))
}

struct InsertSite {
    category: Option<String>,
    title: String,
    url: String,
    resource_type: String,
    connection_config: Value,
    internal_url: Option<String>,
    description: Option<String>,
    icon_url: Option<String>,
    icon_text: Option<String>,
    tags: Vec<String>,
    is_pinned: bool,
}

impl From<CreateSiteRequest> for InsertSite {
    fn from(value: CreateSiteRequest) -> Self {
        Self {
            category: value.category,
            title: value.title,
            url: value.url,
            resource_type: value.resource_type.unwrap_or_else(|| "website".to_string()),
            connection_config: value.connection_config.unwrap_or_else(|| json!({})),
            internal_url: value.internal_url,
            description: value.description,
            icon_url: value.icon_url,
            icon_text: value.icon_text,
            tags: value.tags.unwrap_or_default(),
            is_pinned: value.is_pinned.unwrap_or(false),
        }
    }
}

async fn insert_site(
    state: &AppState,
    current: &CurrentUser,
    visibility: SiteVisibility,
    req: InsertSite,
    department_ids: &[Uuid],
) -> ApiResult<Site> {
    let resource_type = normalize_resource_type(&req.resource_type)?;
    let (url, internal_url, connection_config) = normalize_site_target(
        &resource_type,
        &req.url,
        req.internal_url,
        req.connection_config,
    )?;
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, current).await?;
    access::lock_organization_department_tree(&mut transaction, actor.organization_id).await?;
    let owner_id = owner_for_visibility(&actor, &visibility);
    ensure_site_write_scope_in_transaction(
        &mut transaction,
        &actor,
        &visibility,
        owner_id,
        department_ids,
    )
    .await?;
    let site_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO sites
          (organization_id, owner_id, visibility, category, title, resource_type,
           connection_config, url, internal_url, description, icon_url, icon_text, tags, is_pinned, created_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
        RETURNING id
        "#,
    )
    .bind(actor.organization_id)
    .bind(owner_id)
    .bind(visibility)
    .bind(
        req.category
            .and_then(|v| clean_text(&v))
            .unwrap_or_else(|| "未分组".to_string()),
    )
    .bind(required_text(&req.title, "title")?)
    .bind(resource_type)
    .bind(connection_config)
    .bind(url)
    .bind(internal_url)
    .bind(req.description.and_then(|v| clean_text(&v)))
    .bind(req.icon_url.and_then(|v| clean_text(&v)))
    .bind(normalize_icon_text(req.icon_text)?)
    .bind(clean_tags(req.tags))
    .bind(req.is_pinned)
    .bind(actor.id)
    .fetch_one(&mut *transaction)
    .await?;
    sync_site_department_grants(
        &mut transaction,
        actor.organization_id,
        site_id,
        department_ids,
    )
    .await?;
    transaction.commit().await?;
    load_site(state, actor.organization_id, actor.id, site_id).await
}

async fn load_site(
    state: &AppState,
    organization_id: Uuid,
    user_id: Uuid,
    id: Uuid,
) -> ApiResult<Site> {
    let row = sqlx::query_as::<_, Site>(
        r#"
        SELECT site.id, site.organization_id, site.owner_id, site.visibility, site.category,
               site.title, site.resource_type, site.connection_config, site.url, site.internal_url,
               site.description, site.icon_url, site.icon_text, site.auto_icon_url,
               site.icon_fetched_at, site.tags,
               site.is_pinned, site.sort_order,
               COALESCE(
                 (SELECT activity.open_count FROM site_user_activity activity
                  WHERE activity.site_id = site.id AND activity.user_id = $3),
                 0
               ) AS open_count,
               (SELECT activity.last_opened_at FROM site_user_activity activity
                WHERE activity.site_id = site.id AND activity.user_id = $3) AS last_opened_at,
               COALESCE(
                 (SELECT array_agg(site_grant.department_id ORDER BY site_grant.department_id)
                  FROM site_department_grants site_grant WHERE site_grant.site_id = site.id),
                 ARRAY[]::uuid[]
               ) AS department_ids,
               site.created_by, site.created_at, site.updated_at
        FROM sites site
        WHERE site.id = $1 AND site.organization_id = $2
        "#,
    )
    .bind(id)
    .bind(organization_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    Ok(row)
}

fn basic_site_write_scope_allowed(
    actor_id: Uuid,
    system_role: &SystemRole,
    visibility: &SiteVisibility,
    owner_id: Option<Uuid>,
    has_department_scope: bool,
) -> bool {
    match visibility {
        SiteVisibility::Personal => owner_id == Some(actor_id),
        SiteVisibility::Public => *system_role == SystemRole::SystemAdmin || has_department_scope,
    }
}

async fn ensure_site_write_scope_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CurrentUser,
    visibility: &SiteVisibility,
    owner_id: Option<Uuid>,
    department_ids: &[Uuid],
) -> ApiResult<()> {
    if !basic_site_write_scope_allowed(
        actor.id,
        &actor.system_role,
        visibility,
        owner_id,
        !department_ids.is_empty(),
    ) {
        return Err(ApiError::Forbidden);
    }
    if *visibility == SiteVisibility::Public && actor.system_role != SystemRole::SystemAdmin {
        for department_id in department_ids {
            if !access::manages_department_in_transaction(transaction, actor, *department_id)
                .await?
            {
                return Err(ApiError::Forbidden);
            }
        }
    }
    Ok(())
}

async fn load_site_for_write(state: &AppState, current: &CurrentUser, id: Uuid) -> ApiResult<Site> {
    let row = load_site(state, current.organization_id, current.id, id).await?;
    match &row.visibility {
        SiteVisibility::Personal if row.owner_id == Some(current.id) => Ok(row),
        SiteVisibility::Public if current.system_role == SystemRole::SystemAdmin => Ok(row),
        SiteVisibility::Public if row.department_ids.is_empty() => Err(ApiError::Forbidden),
        SiteVisibility::Public => {
            for department_id in &row.department_ids {
                if !access::manages_department(state, current, *department_id).await? {
                    return Err(ApiError::Forbidden);
                }
            }
            Ok(row)
        }
        SiteVisibility::Personal => Err(ApiError::Forbidden),
    }
}

async fn load_visible_site(state: &AppState, current: &CurrentUser, id: Uuid) -> ApiResult<Site> {
    let visible = sqlx::query_scalar::<_, bool>(
        r#"
        WITH RECURSIVE visible_department_scope AS (
          SELECT site_grant.site_id, site_grant.department_id, ARRAY[site_grant.department_id] AS path
          FROM site_department_grants site_grant
          WHERE site_grant.organization_id = $1

          UNION ALL

          SELECT scope.site_id, child.id, scope.path || child.id
          FROM visible_department_scope scope
          JOIN departments child ON child.parent_id = scope.department_id
          WHERE child.organization_id = $1
            AND NOT child.id = ANY(scope.path)
        )
        SELECT EXISTS (
          SELECT 1
          FROM sites site
          WHERE site.id = $5
            AND site.organization_id = $1
            AND (
              (site.visibility = 'personal' AND site.owner_id = $2)
              OR (
                site.visibility = 'public'
                AND (
                  $3
                  OR NOT EXISTS (
                    SELECT 1 FROM site_department_grants site_grant
                    WHERE site_grant.site_id = site.id
                  )
                  OR EXISTS (
                    SELECT 1
                    FROM visible_department_scope scope
                    JOIN user_department_memberships membership
                      ON membership.department_id = scope.department_id
                     AND membership.organization_id = $1
                    WHERE scope.site_id = site.id AND membership.user_id = $4
                  )
                )
              )
            )
        )
        "#,
    )
    .bind(current.organization_id)
    .bind(current.id)
    .bind(current.system_role == SystemRole::SystemAdmin)
    .bind(current.id)
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    if !visible {
        return Err(ApiError::NotFound);
    }
    load_site(state, current.organization_id, current.id, id).await
}

async fn validate_department_scope(
    state: &AppState,
    current: &CurrentUser,
    visibility: &SiteVisibility,
    department_ids: Vec<Uuid>,
) -> ApiResult<Vec<Uuid>> {
    if *visibility == SiteVisibility::Personal {
        return Ok(Vec::new());
    }

    let mut seen = HashSet::new();
    let department_ids = department_ids
        .into_iter()
        .filter(|department_id| seen.insert(*department_id))
        .collect::<Vec<_>>();
    if department_ids.is_empty() {
        if current.system_role == SystemRole::SystemAdmin {
            return Ok(department_ids);
        }
        return Err(ApiError::BadRequest(
            "team sites require at least one department".to_string(),
        ));
    }

    let existing_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM departments WHERE organization_id = $1 AND id = ANY($2)",
    )
    .bind(current.organization_id)
    .bind(&department_ids)
    .fetch_all(&state.db)
    .await?;
    if existing_ids.len() != department_ids.len() {
        return Err(ApiError::BadRequest(
            "site department scope is invalid".to_string(),
        ));
    }

    if current.system_role != SystemRole::SystemAdmin {
        for department_id in &department_ids {
            let allowed = access::manages_department(state, current, *department_id).await?;
            if !allowed {
                return Err(ApiError::Forbidden);
            }
        }
    }
    Ok(department_ids)
}

async fn sync_site_department_grants(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    site_id: Uuid,
    department_ids: &[Uuid],
) -> ApiResult<()> {
    sqlx::query("DELETE FROM site_department_grants WHERE site_id = $1")
        .bind(site_id)
        .execute(&mut **transaction)
        .await?;
    for department_id in department_ids {
        sqlx::query(
            r#"
            INSERT INTO site_department_grants (site_id, department_id, organization_id)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(site_id)
        .bind(department_id)
        .bind(organization_id)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn site_exists(
    state: &AppState,
    current: &CurrentUser,
    visibility: &SiteVisibility,
    url: &str,
    department_ids: &[Uuid],
) -> ApiResult<bool> {
    let owner_id = owner_for_visibility(current, visibility);
    let normalized = normalize_url(url)?;
    let exists: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM sites
        WHERE organization_id = $1
          AND visibility = $2
          AND owner_id IS NOT DISTINCT FROM $3
          AND url = $4
          AND (
            visibility = 'personal'
            OR COALESCE(
              (SELECT array_agg(grants.department_id ORDER BY grants.department_id)
               FROM site_department_grants grants
               WHERE grants.site_id = sites.id AND grants.organization_id = $1),
              ARRAY[]::uuid[]
            ) = $5
          )
        LIMIT 1
        "#,
    )
    .bind(current.organization_id)
    .bind(visibility)
    .bind(owner_id)
    .bind(normalized)
    .bind(canonical_department_ids(department_ids))
    .fetch_optional(&state.db)
    .await?;
    Ok(exists.is_some())
}

fn canonical_department_ids(department_ids: &[Uuid]) -> Vec<Uuid> {
    let mut ids = department_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn owner_for_visibility(current: &CurrentUser, visibility: &SiteVisibility) -> Option<Uuid> {
    match visibility {
        SiteVisibility::Public => None,
        SiteVisibility::Personal => Some(current.id),
    }
}

fn required_text(value: &str, field: &str) -> ApiResult<String> {
    clean_text(value).ok_or_else(|| ApiError::BadRequest(format!("{field} is required")))
}

fn clean_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.chars().take(240).collect())
    }
}

fn clean_tags(tags: Vec<String>) -> Vec<String> {
    tags.into_iter()
        .filter_map(|tag| clean_text(&tag))
        .take(12)
        .collect()
}

fn normalize_icon_text(value: Option<String>) -> ApiResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().any(char::is_control) {
        return Err(ApiError::BadRequest("icon text is invalid".to_string()));
    }
    if value.chars().count() > 12 {
        return Err(ApiError::BadRequest(
            "icon text must be 12 characters or fewer".to_string(),
        ));
    }
    Ok(Some(value.to_string()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SshConnectionConfig {
    host: String,
    port: Option<u16>,
    username: String,
    auth_method: Option<String>,
    identity_file: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RdpConnectionConfig {
    host: String,
    port: Option<u16>,
    username: String,
    redirect_clipboard: Option<bool>,
    redirect_drives: Option<bool>,
}

fn normalize_resource_type(value: &str) -> ApiResult<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "website" => Ok("website".to_string()),
        "ssh" => Ok("ssh".to_string()),
        "rdp" => Ok("rdp".to_string()),
        _ => Err(ApiError::BadRequest(
            "site resource type is invalid".to_string(),
        )),
    }
}

fn normalize_site_target(
    resource_type: &str,
    url: &str,
    internal_url: Option<String>,
    connection_config: Value,
) -> ApiResult<(String, Option<String>, Value)> {
    match resource_type {
        "website" => Ok((
            normalize_url(url)?,
            normalize_optional_internal_url(internal_url)?,
            json!({}),
        )),
        "ssh" => {
            let config =
                serde_json::from_value::<SshConnectionConfig>(connection_config).map_err(|_| {
                    ApiError::BadRequest("ssh connection config is invalid".to_string())
                })?;
            let host = normalize_connection_host(&config.host)?;
            let port = config.port.unwrap_or(22);
            validate_connection_port(port)?;
            let username = normalize_connection_username(&config.username)?;
            let auth_method = config.auth_method.unwrap_or_else(|| "password".to_string());
            if auth_method != "password" && auth_method != "key" {
                return Err(ApiError::BadRequest(
                    "ssh authentication method is invalid".to_string(),
                ));
            }
            let identity_file = config
                .identity_file
                .and_then(|value| clean_connection_text(&value, 520));
            if auth_method == "key" && identity_file.is_none() {
                return Err(ApiError::BadRequest(
                    "ssh identity file is required".to_string(),
                ));
            }
            let display_host = connection_url_host(&host);
            Ok((
                format!("ssh://{display_host}:{port}"),
                None,
                json!({
                    "host": host,
                    "port": port,
                    "username": username,
                    "auth_method": auth_method,
                    "identity_file": if auth_method == "key" { identity_file } else { None },
                }),
            ))
        }
        "rdp" => {
            let config =
                serde_json::from_value::<RdpConnectionConfig>(connection_config).map_err(|_| {
                    ApiError::BadRequest("rdp connection config is invalid".to_string())
                })?;
            let host = normalize_connection_host(&config.host)?;
            let port = config.port.unwrap_or(3389);
            validate_connection_port(port)?;
            let username = normalize_connection_username(&config.username)?;
            let display_host = connection_url_host(&host);
            Ok((
                format!("rdp://{display_host}:{port}"),
                None,
                json!({
                    "host": host,
                    "port": port,
                    "username": username,
                    "redirect_clipboard": config.redirect_clipboard.unwrap_or(true),
                    "redirect_drives": config.redirect_drives.unwrap_or(false),
                }),
            ))
        }
        _ => Err(ApiError::BadRequest(
            "site resource type is invalid".to_string(),
        )),
    }
}

fn normalize_connection_host(value: &str) -> ApiResult<String> {
    let host = value.trim().trim_start_matches('[').trim_end_matches(']');
    let valid = !host.is_empty()
        && host.len() <= 255
        && host.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | ':')
        });
    if !valid {
        return Err(ApiError::BadRequest(
            "connection host is invalid".to_string(),
        ));
    }
    Ok(host.to_string())
}

fn normalize_connection_username(value: &str) -> ApiResult<String> {
    let username = value.trim();
    let valid = !username.is_empty()
        && username.len() <= 128
        && username.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '.' | '-' | '_' | '@' | '\\' | '+' | '$')
        });
    if !valid {
        return Err(ApiError::BadRequest(
            "connection username is invalid".to_string(),
        ));
    }
    Ok(username.to_string())
}

fn validate_connection_port(port: u16) -> ApiResult<()> {
    if port == 0 {
        return Err(ApiError::BadRequest(
            "connection port is invalid".to_string(),
        ));
    }
    Ok(())
}

fn clean_connection_text(value: &str, limit: usize) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        None
    } else {
        Some(value.chars().take(limit).collect())
    }
}

fn connection_url_host(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn normalize_url(value: &str) -> ApiResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ApiError::BadRequest("url is required".to_string()));
    }
    let lower = trimmed.to_lowercase();
    if lower.starts_with("javascript:")
        || lower.starts_with("data:")
        || lower.starts_with("file:")
        || lower.starts_with("chrome:")
        || lower.starts_with("edge:")
    {
        return Err(ApiError::BadRequest(
            "url scheme is not allowed".to_string(),
        ));
    }
    if lower.contains("://") && !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err(ApiError::BadRequest(
            "url must use http or https".to_string(),
        ));
    }
    let url = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    if url.chars().any(char::is_whitespace) {
        return Err(ApiError::BadRequest(
            "url must not contain whitespace".to_string(),
        ));
    }
    let host_part = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or("")
        .split('/')
        .next()
        .unwrap_or("");
    if host_part.is_empty() {
        return Err(ApiError::BadRequest("url host is required".to_string()));
    }
    Ok(url.chars().take(2048).collect())
}

fn normalize_optional_internal_url(value: Option<String>) -> ApiResult<Option<String>> {
    let Some(value) = value.and_then(|value| clean_text(&value)) else {
        return Ok(None);
    };
    let value = if value.contains("://") {
        value
    } else {
        format!("http://{value}")
    };
    normalize_url(&value).map(Some)
}

#[derive(Debug)]
struct ParsedBookmark {
    category: String,
    title: String,
    url: String,
}

fn parse_bookmarks(html: &str, default_category: Option<&str>) -> ApiResult<Vec<ParsedBookmark>> {
    if html.len() > 5 * 1024 * 1024 {
        return Err(ApiError::BadRequest(
            "bookmark file is too large".to_string(),
        ));
    }
    let mut current_category = default_category.unwrap_or("未分组").to_string();
    let mut bookmarks = Vec::new();
    for line in html.lines() {
        let lower = line.to_lowercase();
        if let Some(folder) = extract_between(&lower, line, "<h3", "</h3>") {
            if default_category.is_none() {
                if let Some(name) = clean_text(&decode_html(&strip_tags(&folder))) {
                    current_category = name;
                }
            }
            continue;
        }
        if !lower.contains("<a ") {
            continue;
        }
        let Some(url) = extract_href(line) else {
            continue;
        };
        let Some(title) = extract_between(&lower, line, "<a ", "</a>") else {
            continue;
        };
        let title = decode_html(&strip_tags(&title));
        let url = decode_html(&url);
        if let (Some(title), Ok(url)) = (clean_text(&title), normalize_url(&url)) {
            bookmarks.push(ParsedBookmark {
                category: current_category.clone(),
                title,
                url,
            });
        }
    }
    Ok(bookmarks)
}

fn extract_between(
    lower_line: &str,
    original_line: &str,
    start: &str,
    end: &str,
) -> Option<String> {
    let start_index = lower_line.find(start)?;
    let after_start = lower_line[start_index..].find('>')? + start_index + 1;
    let end_index = lower_line[after_start..].find(end)? + after_start;
    Some(original_line[after_start..end_index].to_string())
}

fn extract_href(line: &str) -> Option<String> {
    let lower = line.to_lowercase();
    let href_index = lower.find("href")?;
    let after_href = &line[href_index + 4..];
    let equals_index = after_href.find('=')?;
    let after_equals = after_href[equals_index + 1..].trim_start();
    let quote = after_equals.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value_start = quote.len_utf8();
    let value_end = after_equals[value_start..].find(quote)? + value_start;
    Some(after_equals[value_start..value_end].to_string())
}

fn strip_tags(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut in_tag = false;
    for ch in value.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result
}

fn decode_html(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::{
        basic_site_write_scope_allowed, canonical_department_ids, normalize_optional_internal_url,
        normalize_site_target,
    };
    use crate::models::{SiteVisibility, SystemRole};
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn bookmark_department_scope_is_an_order_independent_set() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        assert_eq!(canonical_department_ids(&[b, a, a]), vec![a, b]);
        assert_ne!(
            canonical_department_ids(&[a]),
            canonical_department_ids(&[b])
        );
        assert_ne!(
            canonical_department_ids(&[]),
            canonical_department_ids(&[a])
        );
    }

    #[test]
    fn optional_internal_url_accepts_blank_as_none() {
        assert_eq!(
            normalize_optional_internal_url(Some("  ".to_string())).unwrap(),
            None
        );
    }

    #[test]
    fn optional_internal_url_normalizes_private_http_address() {
        assert_eq!(
            normalize_optional_internal_url(Some("192.168.50.20:5000".to_string())).unwrap(),
            Some("http://192.168.50.20:5000".to_string())
        );
    }

    #[test]
    fn ssh_connection_requires_a_local_key_path_for_key_authentication() {
        let error = normalize_site_target(
            "ssh",
            "",
            None,
            json!({
                "host": "192.168.50.10",
                "port": 22,
                "username": "root",
                "auth_method": "key"
            }),
        )
        .unwrap_err();
        assert!(error.to_string().contains("ssh identity file is required"));
    }

    #[test]
    fn ssh_connection_rejects_embedded_passwords() {
        assert!(
            normalize_site_target(
                "ssh",
                "",
                None,
                json!({
                    "host": "server.local",
                    "username": "operator",
                    "auth_method": "password",
                    "password": "do-not-store"
                }),
            )
            .is_err()
        );
    }

    #[test]
    fn rdp_connection_defaults_to_clipboard_without_drive_mapping() {
        let (_, _, config) = normalize_site_target(
            "rdp",
            "",
            None,
            json!({ "host": "10.0.0.12", "username": "DOMAIN\\user" }),
        )
        .unwrap();
        assert_eq!(config["port"], 3389);
        assert_eq!(config["redirect_clipboard"], true);
        assert_eq!(config["redirect_drives"], false);
    }

    #[test]
    fn server_connection_rejects_zero_port() {
        assert!(
            normalize_site_target(
                "ssh",
                "",
                None,
                json!({
                    "host": "server.local",
                    "port": 0,
                    "username": "operator",
                    "auth_method": "password"
                }),
            )
            .is_err()
        );
    }

    #[test]
    fn team_site_writes_require_system_or_department_management_scope() {
        let actor_id = Uuid::new_v4();
        assert!(basic_site_write_scope_allowed(
            actor_id,
            &SystemRole::SystemAdmin,
            &SiteVisibility::Public,
            None,
            false,
        ));
        assert!(!basic_site_write_scope_allowed(
            actor_id,
            &SystemRole::Member,
            &SiteVisibility::Public,
            None,
            false,
        ));
        assert!(basic_site_write_scope_allowed(
            actor_id,
            &SystemRole::Member,
            &SiteVisibility::Public,
            None,
            true,
        ));
        assert!(basic_site_write_scope_allowed(
            actor_id,
            &SystemRole::Member,
            &SiteVisibility::Personal,
            Some(actor_id),
            false,
        ));
        assert!(!basic_site_write_scope_allowed(
            actor_id,
            &SystemRole::Member,
            &SiteVisibility::Personal,
            Some(Uuid::new_v4()),
            false,
        ));
    }
}
