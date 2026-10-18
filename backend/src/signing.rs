use crate::{
    app::AppState,
    audit::log_event_in_transaction,
    auth::{self, CurrentUser},
    error::{ApiError, ApiResult},
    file_storage::resolve_storage_path,
    files::download_response,
    models::SystemRole,
    response::{ApiEnvelope, ok},
};
use axum::{
    Json,
    extract::{Multipart, Path as AxumPath, State},
    response::Response,
};
use chrono::{DateTime, Utc};
use image::{ImageFormat, ImageReader, Limits, RgbaImage, imageops};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, content::Operation, dictionary};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;
use std::{
    collections::{BTreeSet, HashMap},
    io::{Cursor, Write},
    path::Path,
    time::Duration,
};
use tokio::process::Command;
use uuid::Uuid;
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

const MAX_SEALS_PER_ORGANIZATION: i64 = 100;
const MAX_SEAL_BYTES: usize = 5 * 1024 * 1024;
const MAX_SEAL_DIMENSION: u32 = 4096;
const MAX_DOCUMENT_BYTES: usize = 30 * 1024 * 1024;
const MAX_DOCUMENT_IMAGE_DIMENSION: u32 = 8192;
const MAX_DOCUMENT_IMAGE_PIXELS: u64 = 40_000_000;
const MAX_NORMALIZED_PDF_BYTES: u64 = 60 * 1024 * 1024;
const MAX_DOCUMENT_PAGES: usize = 200;
const MAX_SINGLE_PLACEMENTS: usize = 50;
const CONVERSION_TIMEOUT: Duration = Duration::from_secs(120);
const IMAGE_EXPORT_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_IMAGE_EXPORT_BYTES: u64 = 120 * 1024 * 1024;
const IMAGE_EXPORT_DPI: &str = "300";

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct SealView {
    pub id: Uuid,
    pub name: String,
    pub width_px: i32,
    pub height_px: i32,
    pub size_bytes: i64,
    pub source_kind: String,
    pub generator_definition: Option<Value>,
    pub authorized_user_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct SealAuthorizationUserView {
    pub user_id: Uuid,
    pub display_name: String,
    pub email: String,
    pub department_name: Option<String>,
    pub authorized: bool,
}

#[derive(Debug, Serialize)]
pub struct SealAuthorizationsView {
    pub seal_id: Uuid,
    pub users: Vec<SealAuthorizationUserView>,
}

#[derive(Debug, Deserialize)]
pub struct ReplaceSealAuthorizationsRequest {
    #[serde(default)]
    pub user_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSealRequest {
    pub name: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SealTemplateKind {
    #[default]
    Round,
    Oval,
    Square,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SealGeneratorSettings {
    pub schema_version: i32,
    pub template: SealTemplateKind,
    pub color: String,
    pub wear: i32,
    pub outer_border_width: i32,
    pub organization_font_size: i32,
    pub organization_spacing: i32,
    pub organization_inset: i32,
    pub organization_height: i32,
    pub organization_width: i32,
    pub text_stroke_width: i32,
    pub star_size: i32,
    pub purpose_offset_y: i32,
    pub purpose_font_size: i32,
    pub purpose_spacing: i32,
    pub serial_font_size: i32,
    pub serial_spacing: i32,
    pub serial_inset: i32,
    pub serial_width: i32,
}

impl Default for SealGeneratorSettings {
    fn default() -> Self {
        Self {
            schema_version: 2,
            template: SealTemplateKind::Round,
            color: "#d6212e".to_string(),
            wear: 3,
            outer_border_width: 18,
            organization_font_size: 62,
            organization_spacing: 2,
            organization_inset: 6,
            organization_height: 142,
            organization_width: 92,
            text_stroke_width: 2,
            star_size: 92,
            purpose_offset_y: 0,
            purpose_font_size: 42,
            purpose_spacing: 4,
            serial_font_size: 26,
            serial_spacing: 0,
            serial_inset: 32,
            serial_width: 82,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SealGeneratorDefinition {
    pub schema_version: i32,
    pub organization: String,
    pub purpose: String,
    pub serial: String,
    pub settings: SealGeneratorSettings,
}

#[derive(Debug, Serialize)]
pub struct SealGeneratorTemplateView {
    pub settings: SealGeneratorSettings,
    pub saved: bool,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
struct SealGeneratorTemplateRow {
    settings: Value,
    updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug)]
struct SealImageOptions {
    remove_background: bool,
    red_ink_only: bool,
    ink_texture: bool,
}

impl Default for SealImageOptions {
    fn default() -> Self {
        Self {
            remove_background: true,
            red_ink_only: false,
            ink_texture: true,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DeleteResponse {
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SigningDocumentView {
    pub id: Uuid,
    pub original_filename: String,
    pub source_format: String,
    pub source_size_bytes: i64,
    pub page_count: i32,
    pub has_signed_result: bool,
    pub signed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
struct SigningDocumentRow {
    id: Uuid,
    organization_id: Uuid,
    owner_id: Uuid,
    original_filename: String,
    source_format: String,
    source_pdf_path: String,
    source_size_bytes: i64,
    page_count: i32,
    signed_pdf_path: Option<String>,
    placements: Option<Value>,
    signed_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl SigningDocumentRow {
    fn view(&self) -> SigningDocumentView {
        SigningDocumentView {
            id: self.id,
            original_filename: self.original_filename.clone(),
            source_format: self.source_format.clone(),
            source_size_bytes: self.source_size_bytes,
            page_count: self.page_count,
            has_signed_result: self.signed_pdf_path.is_some(),
            signed_at: self.signed_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

fn ensure_document_not_finalized(signed_pdf_path: Option<&str>) -> ApiResult<()> {
    if signed_pdf_path.is_some() {
        Err(ApiError::Conflict(
            "signing document has already been finalized".to_string(),
        ))
    } else {
        Ok(())
    }
}

#[derive(Debug, FromRow)]
struct SealAsset {
    id: Uuid,
    stored_path: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SigningMode {
    Single,
    Seam,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StampPlacement {
    pub seal_id: Uuid,
    pub page: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SeamPlacement {
    pub seal_id: Uuid,
    pub start_page: u32,
    pub end_page: u32,
    pub y: f32,
    pub height: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FinalizeSigningRequest {
    pub mode: SigningMode,
    #[serde(default)]
    pub placements: Vec<StampPlacement>,
    pub seam: Option<SeamPlacement>,
}

pub async fn list_seals(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<Vec<SealView>>>> {
    let rows = sqlx::query_as::<_, SealView>(
        r#"
        SELECT seal.id, seal.name, seal.width_px, seal.height_px, seal.size_bytes,
               seal.source_kind, seal.generator_definition,
               (
                 SELECT COUNT(*)
                 FROM seal_authorizations seal_grant
                 WHERE seal_grant.seal_id = seal.id
                   AND seal_grant.organization_id = seal.organization_id
               ) AS authorized_user_count,
               seal.created_at, seal.updated_at
        FROM user_seals seal
        WHERE seal.organization_id = $1
          AND (
            $3
            OR EXISTS (
              SELECT 1
              FROM seal_authorizations seal_grant
              WHERE seal_grant.seal_id = seal.id
                AND seal_grant.organization_id = seal.organization_id
                AND seal_grant.user_id = $2
            )
          )
        ORDER BY seal.created_at DESC, seal.id
        "#,
    )
    .bind(current.organization_id)
    .bind(current.id)
    .bind(current.system_role == SystemRole::SystemAdmin)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(rows))
}

pub async fn get_seal_generator_template(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<SealGeneratorTemplateView>>> {
    ensure_system_admin(&current)?;
    let row = sqlx::query_as::<_, SealGeneratorTemplateRow>(
        r#"
        SELECT settings, updated_at
        FROM user_seal_generator_preferences
        WHERE organization_id = $1 AND owner_id = $2
        "#,
    )
    .bind(current.organization_id)
    .bind(current.id)
    .fetch_optional(&state.db)
    .await?;
    let Some(row) = row else {
        return Ok(ok(SealGeneratorTemplateView {
            settings: SealGeneratorSettings::default(),
            saved: false,
            updated_at: None,
        }));
    };
    let settings: SealGeneratorSettings =
        serde_json::from_value(row.settings).map_err(|error| {
            ApiError::Internal(anyhow::anyhow!("invalid saved seal template: {error}"))
        })?;
    validate_seal_generator_settings(&settings)?;
    Ok(ok(SealGeneratorTemplateView {
        settings,
        saved: true,
        updated_at: Some(row.updated_at),
    }))
}

pub async fn update_seal_generator_template(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(settings): Json<SealGeneratorSettings>,
) -> ApiResult<Json<ApiEnvelope<SealGeneratorTemplateView>>> {
    ensure_system_admin(&current)?;
    validate_seal_generator_settings(&settings)?;
    let settings_value =
        serde_json::to_value(&settings).map_err(|error| ApiError::Internal(error.into()))?;
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    ensure_system_admin(&actor)?;
    let row = sqlx::query_as::<_, SealGeneratorTemplateRow>(
        r#"
        INSERT INTO user_seal_generator_preferences (
          organization_id, owner_id, settings, updated_at
        )
        VALUES ($1, $2, $3, now())
        ON CONFLICT (organization_id, owner_id) DO UPDATE
        SET settings = EXCLUDED.settings,
            updated_at = now()
        RETURNING settings, updated_at
        "#,
    )
    .bind(actor.organization_id)
    .bind(actor.id)
    .bind(settings_value)
    .fetch_one(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "seal.generator_template.update",
        "user_seal_generator_preference",
        None,
        json!({
            "schema_version": settings.schema_version,
            "template": settings.template,
        }),
    )
    .await?;
    transaction.commit().await?;
    let saved_settings: SealGeneratorSettings =
        serde_json::from_value(row.settings).map_err(|error| ApiError::Internal(error.into()))?;
    Ok(ok(SealGeneratorTemplateView {
        settings: saved_settings,
        saved: true,
        updated_at: Some(row.updated_at),
    }))
}

pub async fn upload_seal(
    State(state): State<AppState>,
    current: CurrentUser,
    multipart: Multipart,
) -> ApiResult<Json<ApiEnvelope<SealView>>> {
    ensure_system_admin(&current)?;
    let upload = read_seal_upload(multipart).await?;
    let name = upload.name;
    let bytes = upload.bytes;
    let image_options = upload.image_options;
    let generator_definition = upload.generator_definition;
    let name = required_name(&name)?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM user_seals WHERE organization_id = $1")
            .bind(current.organization_id)
            .fetch_one(&state.db)
            .await?;
    if count >= MAX_SEALS_PER_ORGANIZATION {
        return Err(ApiError::BadRequest(format!(
            "at most {MAX_SEALS_PER_ORGANIZATION} seals are allowed"
        )));
    }

    let normalized =
        tokio::task::spawn_blocking(move || normalize_seal_image(&bytes, image_options))
            .await
            .map_err(|error| ApiError::Internal(error.into()))??;
    let id = Uuid::new_v4();
    let relative_path = format!(
        "seals/{}/{}/{}.png",
        current.organization_id, current.id, id
    );
    let stored_path = resolve_storage_path(&state, &relative_path).await?;
    let parent = stored_path
        .parent()
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("seal path has no parent")))?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let temporary_path = stored_path.with_extension(format!("{}.uploading", Uuid::new_v4()));
    tokio::fs::write(&temporary_path, &normalized.png_bytes)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    tokio::fs::rename(&temporary_path, &stored_path)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;

    let result: ApiResult<SealView> = async {
        let mut transaction = state.db.begin().await?;
        let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
        ensure_system_admin(&actor)?;
        let current_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_seals WHERE organization_id = $1")
                .bind(actor.organization_id)
                .fetch_one(&mut *transaction)
                .await?;
        if current_count >= MAX_SEALS_PER_ORGANIZATION {
            return Err(ApiError::BadRequest(format!(
                "at most {MAX_SEALS_PER_ORGANIZATION} seals are allowed"
            )));
        }
        let (source_kind, generator_definition) = seal_source_parts(generator_definition.as_ref())?;
        let row = sqlx::query_as::<_, SealView>(
            r#"
            INSERT INTO user_seals (
              id, organization_id, owner_id, name, stored_path,
              width_px, height_px, size_bytes, source_kind, generator_definition
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING id, name, width_px, height_px, size_bytes,
                      source_kind, generator_definition,
                      0::bigint AS authorized_user_count, created_at, updated_at
            "#,
        )
        .bind(id)
        .bind(actor.organization_id)
        .bind(actor.id)
        .bind(&name)
        .bind(&relative_path)
        .bind(normalized.width as i32)
        .bind(normalized.height as i32)
        .bind(normalized.png_bytes.len() as i64)
        .bind(source_kind)
        .bind(generator_definition)
        .fetch_one(&mut *transaction)
        .await?;
        log_event_in_transaction(
            &mut transaction,
            actor.organization_id,
            Some(actor.id),
            "seal.create",
            "user_seal",
            Some(id),
            json!({
                "width": normalized.width,
                "height": normalized.height,
                "source_kind": source_kind,
            }),
        )
        .await?;
        transaction.commit().await?;
        Ok(row)
    }
    .await;
    match result {
        Ok(row) => Ok(ok(row)),
        Err(error) => {
            let _ = tokio::fs::remove_file(&stored_path).await;
            Err(error)
        }
    }
}

pub async fn seal_image(
    State(state): State<AppState>,
    current: CurrentUser,
    AxumPath(id): AxumPath<Uuid>,
) -> ApiResult<Response> {
    let row: Option<(String, String)> = sqlx::query_as(
        r#"
        SELECT name, stored_path
        FROM user_seals seal
        WHERE seal.id = $1
          AND seal.organization_id = $2
          AND (
            $4
            OR EXISTS (
              SELECT 1
              FROM seal_authorizations seal_grant
              WHERE seal_grant.seal_id = seal.id
                AND seal_grant.organization_id = seal.organization_id
                AND seal_grant.user_id = $3
            )
          )
        "#,
    )
    .bind(id)
    .bind(current.organization_id)
    .bind(current.id)
    .bind(current.system_role == SystemRole::SystemAdmin)
    .fetch_optional(&state.db)
    .await?;
    let (name, stored_path) = row.ok_or(ApiError::NotFound)?;
    let path = resolve_storage_path(&state, &stored_path).await?;
    let bytes = read_existing_file(&path).await?;
    download_response(&format!("{name}.png"), Some("image/png"), bytes)
}

pub async fn update_seal(
    State(state): State<AppState>,
    current: CurrentUser,
    AxumPath(id): AxumPath<Uuid>,
    Json(payload): Json<UpdateSealRequest>,
) -> ApiResult<Json<ApiEnvelope<SealView>>> {
    ensure_system_admin(&current)?;
    let name = required_name(&payload.name)?;
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    ensure_system_admin(&actor)?;
    let row = sqlx::query_as::<_, SealView>(
        r#"
        UPDATE user_seals
        SET name = $3, updated_at = now()
        WHERE id = $1 AND organization_id = $2
        RETURNING id, name, width_px, height_px, size_bytes,
                  source_kind, generator_definition,
                  (
                    SELECT COUNT(*)
                    FROM seal_authorizations seal_grant
                    WHERE seal_grant.seal_id = user_seals.id
                      AND seal_grant.organization_id = user_seals.organization_id
                  ) AS authorized_user_count,
                  created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .bind(name)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "seal.update",
        "user_seal",
        Some(id),
        json!({}),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(row))
}

pub async fn replace_seal(
    State(state): State<AppState>,
    current: CurrentUser,
    AxumPath(id): AxumPath<Uuid>,
    multipart: Multipart,
) -> ApiResult<Json<ApiEnvelope<SealView>>> {
    ensure_system_admin(&current)?;
    let upload = read_seal_upload(multipart).await?;
    let name = required_name(&upload.name)?;
    let (source_kind, generator_definition) =
        seal_source_parts(upload.generator_definition.as_ref())?;
    let bytes = upload.bytes;
    let image_options = upload.image_options;
    let normalized =
        tokio::task::spawn_blocking(move || normalize_seal_image(&bytes, image_options))
            .await
            .map_err(|error| ApiError::Internal(error.into()))??;

    let relative_path: String = sqlx::query_scalar(
        r#"
        SELECT stored_path
        FROM user_seals
        WHERE id = $1 AND organization_id = $2
        "#,
    )
    .bind(id)
    .bind(current.organization_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    let stored_path = resolve_storage_path(&state, &relative_path).await?;
    let parent = stored_path
        .parent()
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("seal path has no parent")))?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let temporary_path = stored_path.with_extension(format!("{}.replacing", Uuid::new_v4()));
    let backup_path = stored_path.with_extension(format!("{}.previous", Uuid::new_v4()));
    tokio::fs::write(&temporary_path, &normalized.png_bytes)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;

    let result: ApiResult<SealView> = async {
        let mut transaction = state.db.begin().await?;
        let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
        ensure_system_admin(&actor)?;
        let row = sqlx::query_as::<_, SealView>(
            r#"
            UPDATE user_seals
            SET name = $3,
                width_px = $4,
                height_px = $5,
                size_bytes = $6,
                source_kind = $7,
                generator_definition = $8,
                updated_at = now()
            WHERE id = $1 AND organization_id = $2
            RETURNING id, name, width_px, height_px, size_bytes,
                      source_kind, generator_definition,
                      (
                        SELECT COUNT(*)
                        FROM seal_authorizations seal_grant
                        WHERE seal_grant.seal_id = user_seals.id
                          AND seal_grant.organization_id = user_seals.organization_id
                      ) AS authorized_user_count,
                      created_at, updated_at
            "#,
        )
        .bind(id)
        .bind(actor.organization_id)
        .bind(&name)
        .bind(normalized.width as i32)
        .bind(normalized.height as i32)
        .bind(normalized.png_bytes.len() as i64)
        .bind(source_kind)
        .bind(generator_definition)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ApiError::NotFound)?;
        log_event_in_transaction(
            &mut transaction,
            actor.organization_id,
            Some(actor.id),
            "seal.replace",
            "user_seal",
            Some(id),
            json!({
                "width": normalized.width,
                "height": normalized.height,
                "source_kind": source_kind,
            }),
        )
        .await?;

        tokio::fs::rename(&stored_path, &backup_path)
            .await
            .map_err(|error| ApiError::Internal(error.into()))?;
        if let Err(error) = tokio::fs::rename(&temporary_path, &stored_path).await {
            let _ = tokio::fs::rename(&backup_path, &stored_path).await;
            return Err(ApiError::Internal(error.into()));
        }
        if let Err(error) = transaction.commit().await {
            let _ = tokio::fs::remove_file(&stored_path).await;
            let _ = tokio::fs::rename(&backup_path, &stored_path).await;
            return Err(error.into());
        }
        let _ = tokio::fs::remove_file(&backup_path).await;
        Ok(row)
    }
    .await;

    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary_path).await;
    }
    result.map(ok)
}

pub async fn delete_seal(
    State(state): State<AppState>,
    current: CurrentUser,
    AxumPath(id): AxumPath<Uuid>,
) -> ApiResult<Json<ApiEnvelope<DeleteResponse>>> {
    ensure_system_admin(&current)?;
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    ensure_system_admin(&actor)?;
    let stored_path: String = sqlx::query_scalar(
        r#"
        DELETE FROM user_seals
        WHERE id = $1 AND organization_id = $2
        RETURNING stored_path
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "seal.delete",
        "user_seal",
        Some(id),
        json!({}),
    )
    .await?;
    transaction.commit().await?;
    let path = resolve_storage_path(&state, &stored_path).await?;
    if let Err(error) = tokio::fs::remove_file(path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, seal_id = %id, "failed to remove deleted seal image");
    }
    Ok(ok(DeleteResponse { deleted: true }))
}

pub async fn get_seal_authorizations(
    State(state): State<AppState>,
    current: CurrentUser,
    AxumPath(id): AxumPath<Uuid>,
) -> ApiResult<Json<ApiEnvelope<SealAuthorizationsView>>> {
    ensure_system_admin(&current)?;
    ensure_seal_exists(&state, current.organization_id, id).await?;
    Ok(ok(SealAuthorizationsView {
        seal_id: id,
        users: load_seal_authorization_users(&state, current.organization_id, id).await?,
    }))
}

pub async fn replace_seal_authorizations(
    State(state): State<AppState>,
    current: CurrentUser,
    AxumPath(id): AxumPath<Uuid>,
    Json(payload): Json<ReplaceSealAuthorizationsRequest>,
) -> ApiResult<Json<ApiEnvelope<SealAuthorizationsView>>> {
    let requested = normalize_authorization_ids(payload.user_ids)?;
    let requested_ids = requested.iter().copied().collect::<Vec<_>>();
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    ensure_system_admin(&actor)?;

    sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM user_seals
        WHERE id = $1 AND organization_id = $2
        FOR UPDATE
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;

    let existing = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT user_id
        FROM seal_authorizations
        WHERE seal_id = $1 AND organization_id = $2
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .fetch_all(&mut *transaction)
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();

    if !requested_ids.is_empty() {
        let valid_ids = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT id
            FROM users
            WHERE organization_id = $1
              AND id = ANY($2)
              AND system_role = 'member'
              AND is_active = TRUE
            FOR SHARE
            "#,
        )
        .bind(actor.organization_id)
        .bind(&requested_ids)
        .fetch_all(&mut *transaction)
        .await?;
        if valid_ids.len() != requested.len() {
            return Err(ApiError::BadRequest(
                "one or more seal authorization users are unavailable".to_string(),
            ));
        }
    }

    sqlx::query(
        r#"
        DELETE FROM seal_authorizations
        WHERE seal_id = $1
          AND organization_id = $2
          AND NOT (user_id = ANY($3))
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .bind(&requested_ids)
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO seal_authorizations (
          seal_id, organization_id, user_id, granted_by
        )
        SELECT $1, $2, requested_user_id, $4
        FROM UNNEST($3::uuid[]) requested_user_id
        ON CONFLICT (seal_id, user_id) DO NOTHING
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .bind(&requested_ids)
    .bind(actor.id)
    .execute(&mut *transaction)
    .await?;

    for user_id in requested.difference(&existing) {
        log_event_in_transaction(
            &mut transaction,
            actor.organization_id,
            Some(actor.id),
            "seal.authorization.grant",
            "user_seal",
            Some(id),
            json!({ "user_id": user_id }),
        )
        .await?;
    }
    for user_id in existing.difference(&requested) {
        log_event_in_transaction(
            &mut transaction,
            actor.organization_id,
            Some(actor.id),
            "seal.authorization.revoke",
            "user_seal",
            Some(id),
            json!({ "user_id": user_id }),
        )
        .await?;
    }
    transaction.commit().await?;

    Ok(ok(SealAuthorizationsView {
        seal_id: id,
        users: load_seal_authorization_users(&state, actor.organization_id, id).await?,
    }))
}

pub async fn list_documents(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<Vec<SigningDocumentView>>>> {
    let rows = sqlx::query_as::<_, SigningDocumentRow>(&document_select(
        "WHERE organization_id = $1 AND owner_id = $2 ORDER BY created_at DESC, id LIMIT 30",
    ))
    .bind(current.organization_id)
    .bind(current.id)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(rows.iter().map(SigningDocumentRow::view).collect()))
}

pub async fn upload_document(
    State(state): State<AppState>,
    current: CurrentUser,
    multipart: Multipart,
) -> ApiResult<Json<ApiEnvelope<SigningDocumentView>>> {
    let upload = read_document_upload(multipart).await?;
    let source_format = document_format(&upload.filename, &upload.bytes)?;
    let id = Uuid::new_v4();
    let relative_dir = format!("signing/{}/{}/{}", current.organization_id, current.id, id);
    let source_relative = format!("{relative_dir}/source.pdf");
    let source_path = resolve_storage_path(&state, &source_relative).await?;
    let directory = source_path
        .parent()
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("signing path has no parent")))?
        .to_path_buf();
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;

    let prepare_result = prepare_source_pdf(
        &state,
        &directory,
        &source_path,
        source_format,
        &upload.bytes,
    )
    .await;
    if let Err(error) = prepare_result {
        let _ = tokio::fs::remove_dir_all(&directory).await;
        return Err(error);
    }
    let metadata = tokio::fs::metadata(&source_path)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    if metadata.len() > MAX_NORMALIZED_PDF_BYTES {
        let _ = tokio::fs::remove_dir_all(&directory).await;
        return Err(ApiError::BadRequest(
            "converted PDF is larger than 60 MiB".to_string(),
        ));
    }
    let inspect_path = source_path.clone();
    let page_count = tokio::task::spawn_blocking(move || inspect_pdf(&inspect_path))
        .await
        .map_err(|error| ApiError::Internal(error.into()))??;
    let original_filename = safe_filename(&upload.filename, "document.pdf");

    let result: ApiResult<SigningDocumentRow> = async {
        let mut transaction = state.db.begin().await?;
        let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
        let row = sqlx::query_as::<_, SigningDocumentRow>(
            r#"
            INSERT INTO signing_documents (
              id, organization_id, owner_id, original_filename, source_format,
              source_pdf_path, source_size_bytes, page_count
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING id, organization_id, owner_id, original_filename, source_format,
                      source_pdf_path, source_size_bytes, page_count, signed_pdf_path,
                      placements, signed_at, created_at, updated_at
            "#,
        )
        .bind(id)
        .bind(actor.organization_id)
        .bind(actor.id)
        .bind(&original_filename)
        .bind(source_format)
        .bind(&source_relative)
        .bind(upload.bytes.len() as i64)
        .bind(page_count as i32)
        .fetch_one(&mut *transaction)
        .await?;
        log_event_in_transaction(
            &mut transaction,
            actor.organization_id,
            Some(actor.id),
            "signing.document.upload",
            "signing_document",
            Some(id),
            json!({ "source_format": source_format, "page_count": page_count }),
        )
        .await?;
        transaction.commit().await?;
        Ok(row)
    }
    .await;
    match result {
        Ok(row) => Ok(ok(row.view())),
        Err(error) => {
            let _ = tokio::fs::remove_dir_all(&directory).await;
            Err(error)
        }
    }
}

pub async fn document_source(
    State(state): State<AppState>,
    current: CurrentUser,
    AxumPath(id): AxumPath<Uuid>,
) -> ApiResult<Response> {
    let row = load_document(&state, &current, id).await?;
    let path = resolve_storage_path(&state, &row.source_pdf_path).await?;
    let bytes = read_existing_file(&path).await?;
    download_response("document.pdf", Some("application/pdf"), bytes)
}

pub async fn delete_document(
    State(state): State<AppState>,
    current: CurrentUser,
    AxumPath(id): AxumPath<Uuid>,
) -> ApiResult<Json<ApiEnvelope<DeleteResponse>>> {
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    let source_path: String = sqlx::query_scalar(
        r#"
        DELETE FROM signing_documents
        WHERE id = $1 AND organization_id = $2 AND owner_id = $3
        RETURNING source_pdf_path
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .bind(actor.id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "signing.document.delete",
        "signing_document",
        Some(id),
        json!({}),
    )
    .await?;
    transaction.commit().await?;
    let source_path = resolve_storage_path(&state, &source_path).await?;
    if let Some(directory) = source_path.parent()
        && let Err(error) = tokio::fs::remove_dir_all(directory).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, document_id = %id, "failed to remove signing document files");
    }
    Ok(ok(DeleteResponse { deleted: true }))
}

pub async fn finalize_document(
    State(state): State<AppState>,
    current: CurrentUser,
    AxumPath(id): AxumPath<Uuid>,
    Json(payload): Json<FinalizeSigningRequest>,
) -> ApiResult<Json<ApiEnvelope<SigningDocumentView>>> {
    let row = load_document(&state, &current, id).await?;
    ensure_document_not_finalized(row.signed_pdf_path.as_deref())?;
    let seal_ids = validate_finalize_request(&payload, row.page_count as usize)?;
    let seal_id_list = seal_ids.iter().copied().collect::<Vec<_>>();
    let seal_rows = sqlx::query_as::<_, SealAsset>(
        r#"
        SELECT seal.id, seal.stored_path
        FROM user_seals seal
        WHERE seal.organization_id = $1
          AND seal.id = ANY($2)
          AND (
            $4
            OR EXISTS (
              SELECT 1
              FROM seal_authorizations seal_grant
              WHERE seal_grant.seal_id = seal.id
                AND seal_grant.organization_id = seal.organization_id
                AND seal_grant.user_id = $3
            )
          )
        "#,
    )
    .bind(current.organization_id)
    .bind(&seal_id_list)
    .bind(current.id)
    .bind(current.system_role == SystemRole::SystemAdmin)
    .fetch_all(&state.db)
    .await?;
    if seal_rows.len() != seal_ids.len() {
        return Err(ApiError::BadRequest(
            "one or more seals are unavailable".to_string(),
        ));
    }
    let mut seal_bytes = HashMap::new();
    for seal in seal_rows {
        let path = resolve_storage_path(&state, &seal.stored_path).await?;
        seal_bytes.insert(seal.id, read_existing_file(&path).await?);
    }
    let source_path = resolve_storage_path(&state, &row.source_pdf_path).await?;
    let result_id = Uuid::new_v4();
    let result_relative = format!(
        "signing/{}/{}/{}/signed-{result_id}.pdf",
        current.organization_id, current.id, id
    );
    let result_path = resolve_storage_path(&state, &result_relative).await?;
    let temporary_path = result_path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let request_for_worker = payload.clone();
    let source_for_worker = source_path.clone();
    let temporary_for_worker = temporary_path.clone();
    tokio::task::spawn_blocking(move || {
        stamp_pdf(
            &source_for_worker,
            &temporary_for_worker,
            &request_for_worker,
            &seal_bytes,
        )
    })
    .await
    .map_err(|error| ApiError::Internal(error.into()))??;
    tokio::fs::rename(&temporary_path, &result_path)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;

    let placements =
        serde_json::to_value(&payload).map_err(|error| ApiError::Internal(error.into()))?;
    let update_result: ApiResult<(SigningDocumentRow, Option<String>)> = async {
        let mut transaction = state.db.begin().await?;
        let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
        let available_seal_ids = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT seal.id
            FROM user_seals seal
            WHERE seal.organization_id = $1
              AND seal.id = ANY($2)
              AND (
                $4
                OR EXISTS (
                  SELECT 1
                  FROM seal_authorizations seal_grant
                  WHERE seal_grant.seal_id = seal.id
                    AND seal_grant.organization_id = seal.organization_id
                    AND seal_grant.user_id = $3
                )
              )
            FOR SHARE OF seal
            "#,
        )
        .bind(actor.organization_id)
        .bind(&seal_id_list)
        .bind(actor.id)
        .bind(actor.system_role == SystemRole::SystemAdmin)
        .fetch_all(&mut *transaction)
        .await?;
        if available_seal_ids.len() != seal_ids.len() {
            return Err(ApiError::BadRequest(
                "one or more seals are unavailable".to_string(),
            ));
        }
        let previous_path: Option<String> = sqlx::query_scalar(
            r#"
            SELECT signed_pdf_path
            FROM signing_documents
            WHERE id = $1 AND organization_id = $2 AND owner_id = $3
            FOR UPDATE
            "#,
        )
        .bind(id)
        .bind(actor.organization_id)
        .bind(actor.id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ApiError::NotFound)?;
        ensure_document_not_finalized(previous_path.as_deref())?;
        let updated = sqlx::query_as::<_, SigningDocumentRow>(
            r#"
            UPDATE signing_documents
            SET signed_pdf_path = $4, placements = $5, signed_at = now(), updated_at = now()
            WHERE id = $1 AND organization_id = $2 AND owner_id = $3
            RETURNING id, organization_id, owner_id, original_filename, source_format,
                      source_pdf_path, source_size_bytes, page_count, signed_pdf_path,
                      placements, signed_at, created_at, updated_at
            "#,
        )
        .bind(id)
        .bind(actor.organization_id)
        .bind(actor.id)
        .bind(&result_relative)
        .bind(&placements)
        .fetch_one(&mut *transaction)
        .await?;
        log_event_in_transaction(
            &mut transaction,
            actor.organization_id,
            Some(actor.id),
            "signing.document.finalize",
            "signing_document",
            Some(id),
            json!({
                "mode": match payload.mode { SigningMode::Single => "single", SigningMode::Seam => "seam" },
                "seal_count": seal_ids.len(),
                "seal_ids": seal_id_list,
            }),
        )
        .await?;
        transaction.commit().await?;
        Ok((updated, previous_path))
    }
    .await;
    match update_result {
        Ok((updated, previous_path)) => {
            if let Some(previous_path) = previous_path
                && previous_path != result_relative
                && let Ok(path) = resolve_storage_path(&state, &previous_path).await
            {
                let _ = tokio::fs::remove_file(path).await;
            }
            Ok(ok(updated.view()))
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(result_path).await;
            Err(error)
        }
    }
}

pub async fn download_signed_document(
    State(state): State<AppState>,
    current: CurrentUser,
    AxumPath(id): AxumPath<Uuid>,
) -> ApiResult<Response> {
    let row = load_document(&state, &current, id).await?;
    let signed_path = row
        .signed_pdf_path
        .ok_or_else(|| ApiError::BadRequest("document has not been finalized".to_string()))?;
    let path = resolve_storage_path(&state, &signed_path).await?;
    let bytes = read_existing_file(&path).await?;
    download_response(
        &signed_filename(&row.original_filename),
        Some("application/pdf"),
        bytes,
    )
}

pub async fn export_signed_images(
    State(state): State<AppState>,
    current: CurrentUser,
    AxumPath(id): AxumPath<Uuid>,
) -> ApiResult<Response> {
    let row = load_document(&state, &current, id).await?;
    let signed_path = row
        .signed_pdf_path
        .ok_or_else(|| ApiError::BadRequest("document has not been finalized".to_string()))?;
    let pdf_path = resolve_storage_path(&state, &signed_path).await?;
    let export_dir = pdf_path
        .parent()
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("signed PDF path has no parent")))?
        .join(format!("export-{}", Uuid::new_v4()));
    tokio::fs::create_dir(&export_dir)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let prefix = export_dir.join("page");
    let single_page = row.page_count == 1;
    let command_result = run_pdftoppm(&state, &pdf_path, &prefix, single_page).await;
    if let Err(error) = command_result {
        let _ = tokio::fs::remove_dir_all(&export_dir).await;
        return Err(error);
    }
    if single_page {
        let image_result: ApiResult<Vec<u8>> = async {
            let bytes = read_existing_file(&prefix.with_extension("png")).await?;
            if bytes.len() as u64 > MAX_IMAGE_EXPORT_BYTES {
                return Err(ApiError::BadRequest(
                    "exported page image is larger than 120 MiB".to_string(),
                ));
            }
            Ok(bytes)
        }
        .await;
        let _ = tokio::fs::remove_dir_all(&export_dir).await;
        return download_response(
            &image_export_filename(&row.original_filename, row.page_count),
            Some("image/png"),
            image_result?,
        );
    }
    let archive_result = build_image_archive(&export_dir).await;
    let _ = tokio::fs::remove_dir_all(&export_dir).await;
    let bytes = archive_result?;
    download_response(
        &image_export_filename(&row.original_filename, row.page_count),
        Some("application/zip"),
        bytes,
    )
}

struct SealUpload {
    name: String,
    bytes: Vec<u8>,
    image_options: SealImageOptions,
    generator_definition: Option<SealGeneratorDefinition>,
}

async fn read_seal_upload(mut multipart: Multipart) -> ApiResult<SealUpload> {
    let mut name = None;
    let mut bytes = None;
    let mut generator_definition = None;
    let mut image_options = SealImageOptions::default();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| ApiError::BadRequest(error.to_string()))?
    {
        match field.name() {
            Some("name") => {
                name = Some(
                    field
                        .text()
                        .await
                        .map_err(|error| ApiError::BadRequest(error.to_string()))?,
                );
            }
            Some("file") => {
                let value = field
                    .bytes()
                    .await
                    .map_err(|error| ApiError::BadRequest(error.to_string()))?;
                if value.is_empty() || value.len() > MAX_SEAL_BYTES {
                    return Err(ApiError::BadRequest(
                        "seal image must contain 1 byte to 5 MiB".to_string(),
                    ));
                }
                bytes = Some(value.to_vec());
            }
            Some("remove_background") => {
                image_options.remove_background = multipart_boolean(
                    "remove_background",
                    &field
                        .text()
                        .await
                        .map_err(|error| ApiError::BadRequest(error.to_string()))?,
                )?;
            }
            Some("red_ink_only") => {
                image_options.red_ink_only = multipart_boolean(
                    "red_ink_only",
                    &field
                        .text()
                        .await
                        .map_err(|error| ApiError::BadRequest(error.to_string()))?,
                )?;
            }
            Some("ink_texture") => {
                image_options.ink_texture = multipart_boolean(
                    "ink_texture",
                    &field
                        .text()
                        .await
                        .map_err(|error| ApiError::BadRequest(error.to_string()))?,
                )?;
            }
            Some("generator_definition") => {
                let value = field
                    .text()
                    .await
                    .map_err(|error| ApiError::BadRequest(error.to_string()))?;
                let definition: SealGeneratorDefinition =
                    serde_json::from_str(&value).map_err(|_| {
                        ApiError::BadRequest("seal generator definition is invalid".to_string())
                    })?;
                validate_seal_generator_definition(&definition)?;
                generator_definition = Some(definition);
            }
            _ => {}
        }
    }
    Ok(SealUpload {
        name: name.ok_or_else(|| ApiError::BadRequest("seal name is required".to_string()))?,
        bytes: bytes.ok_or_else(|| ApiError::BadRequest("seal image is required".to_string()))?,
        image_options,
        generator_definition,
    })
}

fn multipart_boolean(field: &str, value: &str) -> ApiResult<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "on" => Ok(true),
        "false" | "0" | "off" => Ok(false),
        _ => Err(ApiError::BadRequest(format!(
            "seal option {field} must be true or false"
        ))),
    }
}

struct DocumentUpload {
    filename: String,
    bytes: Vec<u8>,
}

async fn read_document_upload(mut multipart: Multipart) -> ApiResult<DocumentUpload> {
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| ApiError::BadRequest(error.to_string()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field
            .file_name()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "document.pdf".to_string());
        let value = field
            .bytes()
            .await
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;
        if value.is_empty() || value.len() > MAX_DOCUMENT_BYTES {
            return Err(ApiError::BadRequest(
                "document must contain 1 byte to 30 MiB".to_string(),
            ));
        }
        return Ok(DocumentUpload {
            filename,
            bytes: value.to_vec(),
        });
    }
    Err(ApiError::BadRequest(
        "document file is required".to_string(),
    ))
}

struct NormalizedSeal {
    png_bytes: Vec<u8>,
    width: u32,
    height: u32,
}

fn normalize_seal_image(bytes: &[u8], options: SealImageOptions) -> ApiResult<NormalizedSeal> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| ApiError::BadRequest("seal image format is invalid".to_string()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SEAL_DIMENSION);
    limits.max_image_height = Some(MAX_SEAL_DIMENSION);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut image = reader
        .decode()
        .map_err(|_| ApiError::BadRequest("seal image must be PNG, JPEG, or WebP".to_string()))?
        .into_rgba8();
    let (width, height) = image.dimensions();
    if width < 32 || height < 32 || width > MAX_SEAL_DIMENSION || height > MAX_SEAL_DIMENSION {
        return Err(ApiError::BadRequest(
            "seal image dimensions must be between 32 and 4096 pixels".to_string(),
        ));
    }
    process_seal_pixels(&mut image, options);
    let mut cursor = Cursor::new(Vec::new());
    image
        .write_to(&mut cursor, ImageFormat::Png)
        .map_err(|error| ApiError::Internal(error.into()))?;
    let png_bytes = cursor.into_inner();
    if png_bytes.len() > MAX_SEAL_BYTES {
        return Err(ApiError::BadRequest(
            "normalized seal image is larger than 5 MiB".to_string(),
        ));
    }
    Ok(NormalizedSeal {
        png_bytes,
        width,
        height,
    })
}

fn process_seal_pixels(image: &mut RgbaImage, options: SealImageOptions) {
    if !options.remove_background && !options.red_ink_only && !options.ink_texture {
        return;
    }
    let width = image.width();
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let original_alpha = pixel.0[3];
        if original_alpha == 0 {
            continue;
        }

        if options.remove_background || options.red_ink_only {
            let minimum = pixel.0[0].min(pixel.0[1]).min(pixel.0[2]);
            let coverage = (255.0 - f32::from(minimum)) / 255.0;
            if coverage <= 0.02 {
                pixel.0[3] = 0;
                continue;
            }
            let corrected = [
                remove_white_matte(pixel.0[0], coverage),
                remove_white_matte(pixel.0[1], coverage),
                remove_white_matte(pixel.0[2], coverage),
            ];
            let red_strength = if options.red_ink_only {
                red_ink_strength(corrected)
            } else {
                1.0
            };
            if red_strength <= 0.0 {
                pixel.0[3] = 0;
                continue;
            }
            pixel.0[..3].copy_from_slice(&corrected);
            pixel.0[3] = (f32::from(original_alpha) * coverage * red_strength)
                .round()
                .clamp(0.0, 255.0) as u8;
        }

        if options.ink_texture && pixel.0[3] > 0 {
            let coarse = texture_hash(x / 7, y / 7, width);
            let fine = texture_hash(x, y, width ^ 0x9e37_79b9);
            let variation = 0.91 + (coarse & 0xff) as f32 / 255.0 * 0.08;
            let pinhole = if fine % 5000 < 8 { 0.42 } else { 1.0 };
            pixel.0[3] = (f32::from(pixel.0[3]) * variation * pinhole)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
}

fn remove_white_matte(channel: u8, coverage: f32) -> u8 {
    ((f32::from(channel) - 255.0 * (1.0 - coverage)) / coverage)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn red_ink_strength(color: [u8; 3]) -> f32 {
    let red = f32::from(color[0]);
    let other = f32::from(color[1].max(color[2]));
    let minimum = f32::from(color[0].min(color[1]).min(color[2]));
    if red < 55.0 || red - other <= 10.0 {
        return 0.0;
    }
    let dominance = ((red - other - 10.0) / 70.0).clamp(0.0, 1.0);
    let saturation = ((red.max(other) - minimum - 12.0) / 60.0).clamp(0.0, 1.0);
    dominance.min(saturation)
}

fn texture_hash(x: u32, y: u32, seed: u32) -> u32 {
    let mut value = x
        .wrapping_mul(0x45d9_f3b)
        .wrapping_add(y.wrapping_mul(0x119d_e1f3))
        .wrapping_add(seed);
    value ^= value >> 16;
    value = value.wrapping_mul(0x45d9_f3b);
    value ^ (value >> 16)
}

fn document_format(filename: &str, bytes: &[u8]) -> ApiResult<&'static str> {
    let extension = Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "pdf" if bytes.starts_with(b"%PDF-") => Ok("pdf"),
        "doc" if bytes.starts_with(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]) => Ok("doc"),
        "docx" if valid_docx(bytes) => Ok("docx"),
        "png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => Ok("png"),
        "jpg" | "jpeg" if bytes.starts_with(&[0xff, 0xd8, 0xff]) => Ok("jpg"),
        "webp" if valid_webp(bytes) => Ok("webp"),
        "pdf" | "doc" | "docx" | "png" | "jpg" | "jpeg" | "webp" => Err(ApiError::BadRequest(
            "document content does not match its file extension".to_string(),
        )),
        _ => Err(ApiError::BadRequest(
            "only PDF, DOC, DOCX, PNG, JPEG, and WebP files are supported".to_string(),
        )),
    }
}

fn valid_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP"
}

fn valid_docx(bytes: &[u8]) -> bool {
    let Ok(mut archive) = ZipArchive::new(Cursor::new(bytes)) else {
        return false;
    };
    archive.by_name("[Content_Types].xml").is_ok() && archive.by_name("word/document.xml").is_ok()
}

async fn prepare_source_pdf(
    state: &AppState,
    directory: &Path,
    source_path: &Path,
    source_format: &str,
    bytes: &[u8],
) -> ApiResult<()> {
    if source_format == "pdf" {
        let temporary = source_path.with_extension(format!("{}.uploading", Uuid::new_v4()));
        tokio::fs::write(&temporary, bytes)
            .await
            .map_err(|error| ApiError::Internal(error.into()))?;
        tokio::fs::rename(temporary, source_path)
            .await
            .map_err(|error| ApiError::Internal(error.into()))?;
        return Ok(());
    }
    if matches!(source_format, "png" | "jpg" | "webp") {
        let temporary = source_path.with_extension(format!("{}.uploading", Uuid::new_v4()));
        let image_bytes = bytes.to_vec();
        let worker_path = temporary.clone();
        tokio::task::spawn_blocking(move || image_to_pdf(&image_bytes, &worker_path))
            .await
            .map_err(|error| ApiError::Internal(error.into()))??;
        tokio::fs::rename(temporary, source_path)
            .await
            .map_err(|error| ApiError::Internal(error.into()))?;
        return Ok(());
    }
    let input_path = directory.join(format!("source-input.{source_format}"));
    tokio::fs::write(&input_path, bytes)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let mut command = Command::new(&state.config.libreoffice_bin);
    command
        .kill_on_drop(true)
        .env("HOME", directory)
        .arg("--headless")
        .arg("--convert-to")
        .arg("pdf")
        .arg("--outdir")
        .arg(directory)
        .arg(&input_path);
    let command = command.output();
    let output = match tokio::time::timeout(CONVERSION_TIMEOUT, command).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::Unavailable("document conversion"));
        }
        Ok(Err(error)) => return Err(ApiError::Internal(error.into())),
        Err(_) => {
            return Err(ApiError::BadRequest(
                "Word document conversion timed out".to_string(),
            ));
        }
    };
    let converted_path = directory.join("source-input.pdf");
    let _ = tokio::fs::remove_file(&input_path).await;
    if !output.status.success() || tokio::fs::metadata(&converted_path).await.is_err() {
        tracing::warn!(
            status = ?output.status.code(),
            stderr = %String::from_utf8_lossy(&output.stderr).chars().take(1000).collect::<String>(),
            "Word document conversion failed"
        );
        return Err(ApiError::BadRequest(
            "Word document could not be converted to PDF".to_string(),
        ));
    }
    tokio::fs::rename(converted_path, source_path)
        .await
        .map_err(|error| ApiError::Internal(error.into()))
}

fn image_to_pdf(bytes: &[u8], output_path: &Path) -> ApiResult<()> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| ApiError::BadRequest("image format is invalid".to_string()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DOCUMENT_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_DOCUMENT_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DOCUMENT_IMAGE_PIXELS * 4 + 32 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|_| ApiError::BadRequest("image must be PNG, JPEG, or WebP".to_string()))?
        .into_rgba8();
    let (width, height) = image.dimensions();
    let pixels = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || width > MAX_DOCUMENT_IMAGE_DIMENSION
        || height > MAX_DOCUMENT_IMAGE_DIMENSION
        || pixels > MAX_DOCUMENT_IMAGE_PIXELS
    {
        return Err(ApiError::BadRequest(
            "image dimensions must not exceed 8192 pixels or 40 megapixels".to_string(),
        ));
    }

    // Use CSS's 96 DPI convention so browser preview and exported PDF retain a natural scale.
    let page_width = width as f32 * 0.75;
    let page_height = height as f32 * 0.75;
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let content_id = document.add_object(Stream::new(Dictionary::new(), Vec::new()));
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "MediaBox" => vec![0.into(), 0.into(), page_width.into(), page_height.into()],
    });
    document.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        }),
    );
    let catalog_id = document.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    document.trailer.set("Root", catalog_id);
    let image_id = create_image_object(&mut document, &image)?;
    insert_image_object(
        &mut document,
        page_id,
        image_id,
        (0.0, 0.0),
        (page_width, page_height),
    )?;
    document.compress();
    document
        .save(output_path)
        .map_err(|error| ApiError::Internal(error.into()))?;
    Ok(())
}

fn inspect_pdf(path: &Path) -> ApiResult<usize> {
    let document = Document::load(path)
        .map_err(|_| ApiError::BadRequest("PDF document is invalid".to_string()))?;
    if document.is_encrypted() {
        return Err(ApiError::BadRequest(
            "password-protected PDFs are not supported".to_string(),
        ));
    }
    let count = document.get_pages().len();
    if count == 0 || count > MAX_DOCUMENT_PAGES {
        return Err(ApiError::BadRequest(format!(
            "PDF must contain 1 to {MAX_DOCUMENT_PAGES} pages"
        )));
    }
    Ok(count)
}

fn validate_finalize_request(
    payload: &FinalizeSigningRequest,
    page_count: usize,
) -> ApiResult<BTreeSet<Uuid>> {
    let mut seal_ids = BTreeSet::new();
    match payload.mode {
        SigningMode::Single => {
            if payload.placements.is_empty() || payload.placements.len() > MAX_SINGLE_PLACEMENTS {
                return Err(ApiError::BadRequest(format!(
                    "single signing requires 1 to {MAX_SINGLE_PLACEMENTS} placements"
                )));
            }
            if payload.seam.is_some() {
                return Err(ApiError::BadRequest(
                    "single signing cannot contain a seam placement".to_string(),
                ));
            }
            for placement in &payload.placements {
                if placement.page == 0 || placement.page as usize > page_count {
                    return Err(ApiError::BadRequest(
                        "stamp placement page is invalid".to_string(),
                    ));
                }
                validate_box(placement.x, placement.y, placement.width, placement.height)?;
                seal_ids.insert(placement.seal_id);
            }
        }
        SigningMode::Seam => {
            if !payload.placements.is_empty() {
                return Err(ApiError::BadRequest(
                    "seam signing cannot contain single placements".to_string(),
                ));
            }
            let seam = payload
                .seam
                .as_ref()
                .ok_or_else(|| ApiError::BadRequest("seam placement is required".to_string()))?;
            if seam.start_page == 0
                || seam.end_page <= seam.start_page
                || seam.end_page as usize > page_count
            {
                return Err(ApiError::BadRequest(
                    "seam signing page range is invalid".to_string(),
                ));
            }
            if !seam.y.is_finite()
                || !seam.height.is_finite()
                || seam.y < 0.0
                || seam.height < 0.03
                || seam.y + seam.height > 1.0
            {
                return Err(ApiError::BadRequest(
                    "seam signing position is invalid".to_string(),
                ));
            }
            seal_ids.insert(seam.seal_id);
        }
    }
    Ok(seal_ids)
}

fn validate_box(x: f32, y: f32, width: f32, height: f32) -> ApiResult<()> {
    if !x.is_finite()
        || !y.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || x < 0.0
        || y < 0.0
        || width < 0.02
        || height < 0.02
        || x + width > 1.0
        || y + height > 1.0
    {
        return Err(ApiError::BadRequest(
            "stamp placement coordinates are invalid".to_string(),
        ));
    }
    Ok(())
}

fn stamp_pdf(
    source_path: &Path,
    output_path: &Path,
    payload: &FinalizeSigningRequest,
    seal_bytes: &HashMap<Uuid, Vec<u8>>,
) -> ApiResult<()> {
    let mut document = Document::load(source_path)
        .map_err(|_| ApiError::BadRequest("PDF document is invalid".to_string()))?;
    if document.is_encrypted() {
        return Err(ApiError::BadRequest(
            "password-protected PDFs are not supported".to_string(),
        ));
    }
    let pages = document.get_pages();
    match payload.mode {
        SigningMode::Single => {
            let mut image_ids = HashMap::new();
            for placement in &payload.placements {
                let page_id = *pages.get(&placement.page).ok_or_else(|| {
                    ApiError::BadRequest("stamp placement page is invalid".to_string())
                })?;
                let image_id = if let Some(image_id) = image_ids.get(&placement.seal_id) {
                    *image_id
                } else {
                    let image = decode_normalized_seal(seal_bytes.get(&placement.seal_id))?;
                    let image_id = create_image_object(&mut document, &image)?;
                    image_ids.insert(placement.seal_id, image_id);
                    image_id
                };
                let page_box = inherited_page_box(&document, page_id)?;
                let page_width = page_box[2] - page_box[0];
                let page_height = page_box[3] - page_box[1];
                insert_image_object(
                    &mut document,
                    page_id,
                    image_id,
                    (
                        page_box[0] + placement.x * page_width,
                        page_box[1] + page_height - (placement.y + placement.height) * page_height,
                    ),
                    (placement.width * page_width, placement.height * page_height),
                )?;
            }
        }
        SigningMode::Seam => {
            let seam = payload.seam.as_ref().expect("validated seam placement");
            let seal = decode_normalized_seal(seal_bytes.get(&seam.seal_id))?;
            let page_total = seam.end_page - seam.start_page + 1;
            for (index, page_number) in (seam.start_page..=seam.end_page).enumerate() {
                let page_id = *pages.get(&page_number).ok_or_else(|| {
                    ApiError::BadRequest("seam signing page range is invalid".to_string())
                })?;
                let start_x = ((seal.width() as u64 * index as u64) / page_total as u64) as u32;
                let end_x = ((seal.width() as u64 * (index as u64 + 1)) / page_total as u64) as u32;
                let slice_width = end_x.saturating_sub(start_x).max(1);
                let slice =
                    imageops::crop_imm(&seal, start_x, 0, slice_width, seal.height()).to_image();
                let image_id = create_image_object(&mut document, &slice)?;
                let page_box = inherited_page_box(&document, page_id)?;
                let page_width = page_box[2] - page_box[0];
                let page_height = page_box[3] - page_box[1];
                let display_height = seam.height * page_height;
                let display_width = display_height * seal.width() as f32 / seal.height() as f32
                    * slice_width as f32
                    / seal.width() as f32;
                if display_width >= page_width {
                    return Err(ApiError::BadRequest(
                        "seam stamp is too large for the selected pages".to_string(),
                    ));
                }
                insert_image_object(
                    &mut document,
                    page_id,
                    image_id,
                    (
                        page_box[2] - display_width,
                        page_box[1] + page_height - (seam.y + seam.height) * page_height,
                    ),
                    (display_width, display_height),
                )?;
            }
        }
    }
    document.compress();
    document
        .save(output_path)
        .map_err(|error| ApiError::Internal(error.into()))?;
    Ok(())
}

fn decode_normalized_seal(bytes: Option<&Vec<u8>>) -> ApiResult<RgbaImage> {
    let bytes = bytes.ok_or_else(|| ApiError::BadRequest("seal is unavailable".to_string()))?;
    image::load_from_memory_with_format(bytes, ImageFormat::Png)
        .map(|image| image.into_rgba8())
        .map_err(|_| ApiError::BadRequest("seal image is invalid".to_string()))
}

fn create_image_object(document: &mut Document, image: &RgbaImage) -> ApiResult<ObjectId> {
    let mut rgb = Vec::with_capacity((image.width() * image.height() * 3) as usize);
    let mut alpha = Vec::with_capacity((image.width() * image.height()) as usize);
    let mut has_transparency = false;
    for pixel in image.pixels() {
        rgb.extend_from_slice(&pixel.0[..3]);
        alpha.push(pixel.0[3]);
        has_transparency |= pixel.0[3] != 255;
    }
    let mut dictionary = Dictionary::new();
    dictionary.set("Type", Object::Name(b"XObject".to_vec()));
    dictionary.set("Subtype", Object::Name(b"Image".to_vec()));
    dictionary.set("Width", i64::from(image.width()));
    dictionary.set("Height", i64::from(image.height()));
    dictionary.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
    dictionary.set("BitsPerComponent", 8_i64);
    if has_transparency {
        let mut mask_dictionary = Dictionary::new();
        mask_dictionary.set("Type", Object::Name(b"XObject".to_vec()));
        mask_dictionary.set("Subtype", Object::Name(b"Image".to_vec()));
        mask_dictionary.set("Width", i64::from(image.width()));
        mask_dictionary.set("Height", i64::from(image.height()));
        mask_dictionary.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
        mask_dictionary.set("BitsPerComponent", 8_i64);
        let mut mask = Stream::new(mask_dictionary, alpha);
        mask.compress()
            .map_err(|error| ApiError::Internal(error.into()))?;
        let mask_id = document.add_object(mask);
        dictionary.set("SMask", Object::Reference(mask_id));
    }
    let mut stream = Stream::new(dictionary, rgb);
    stream
        .compress()
        .map_err(|error| ApiError::Internal(error.into()))?;
    Ok(document.add_object(stream))
}

fn insert_image_object(
    document: &mut Document,
    page_id: ObjectId,
    image_id: ObjectId,
    position: (f32, f32),
    size: (f32, f32),
) -> ApiResult<()> {
    let name = format!("TmSeal{}", image_id.0);
    document
        .add_xobject(page_id, name.as_bytes(), image_id)
        .map_err(|error| ApiError::Internal(error.into()))?;
    let mut content = document
        .get_and_decode_page_content(page_id)
        .map_err(|error| ApiError::Internal(error.into()))?;
    content.operations.push(Operation::new("q", vec![]));
    content.operations.push(Operation::new(
        "cm",
        vec![
            size.0.into(),
            0.into(),
            0.into(),
            size.1.into(),
            position.0.into(),
            position.1.into(),
        ],
    ));
    content.operations.push(Operation::new(
        "Do",
        vec![Object::Name(name.as_bytes().to_vec())],
    ));
    content.operations.push(Operation::new("Q", vec![]));
    document
        .change_page_content(
            page_id,
            content
                .encode()
                .map_err(|error| ApiError::Internal(error.into()))?,
        )
        .map_err(|error| ApiError::Internal(error.into()))
}

fn inherited_page_box(document: &Document, page_id: ObjectId) -> ApiResult<[f32; 4]> {
    let crop_box = inherited_array(document, page_id, b"CropBox")
        .map_err(|_| ApiError::BadRequest("PDF page tree is invalid".to_string()))?;
    let media_box = if crop_box.is_none() {
        inherited_array(document, page_id, b"MediaBox")
            .map_err(|_| ApiError::BadRequest("PDF page tree is invalid".to_string()))?
    } else {
        None
    };
    let values = crop_box
        .or(media_box)
        .ok_or_else(|| ApiError::BadRequest("PDF page has no media box".to_string()))?;
    if values.len() != 4 {
        return Err(ApiError::BadRequest(
            "PDF page media box is invalid".to_string(),
        ));
    }
    let mut result = [0.0; 4];
    for (index, value) in values.iter().enumerate() {
        result[index] = value
            .as_float()
            .map_err(|_| ApiError::BadRequest("PDF page media box is invalid".to_string()))?;
    }
    if result[2] <= result[0] || result[3] <= result[1] {
        return Err(ApiError::BadRequest(
            "PDF page media box is invalid".to_string(),
        ));
    }
    Ok(result)
}

fn inherited_array(
    document: &Document,
    start: ObjectId,
    key: &[u8],
) -> Result<Option<Vec<Object>>, lopdf::Error> {
    let mut current = start;
    for _ in 0..32 {
        let dictionary = document.get_object(current)?.as_dict()?;
        if let Ok(value) = dictionary.get(key) {
            let value = match value {
                Object::Reference(id) => document.get_object(*id)?,
                value => value,
            };
            return Ok(Some(value.as_array()?.clone()));
        }
        match dictionary.get(b"Parent") {
            Ok(Object::Reference(parent)) => current = *parent,
            _ => return Ok(None),
        }
    }
    Ok(None)
}

async fn run_pdftoppm(
    state: &AppState,
    pdf_path: &Path,
    prefix: &Path,
    single_page: bool,
) -> ApiResult<()> {
    let mut command = Command::new(&state.config.pdftoppm_bin);
    command
        .kill_on_drop(true)
        .arg("-png")
        .arg("-r")
        .arg(IMAGE_EXPORT_DPI);
    if single_page {
        command.arg("-singlefile");
    }
    command.arg(pdf_path).arg(prefix);
    let command = command.output();
    let output = match tokio::time::timeout(IMAGE_EXPORT_TIMEOUT, command).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::Unavailable("PDF image export"));
        }
        Ok(Err(error)) => return Err(ApiError::Internal(error.into())),
        Err(_) => {
            return Err(ApiError::BadRequest(
                "PDF image export timed out".to_string(),
            ));
        }
    };
    if !output.status.success() {
        tracing::warn!(
            status = ?output.status.code(),
            stderr = %String::from_utf8_lossy(&output.stderr).chars().take(1000).collect::<String>(),
            "PDF image export failed"
        );
        return Err(ApiError::BadRequest(
            "signed PDF could not be exported as images".to_string(),
        ));
    }
    Ok(())
}

async fn build_image_archive(directory: &Path) -> ApiResult<Vec<u8>> {
    let mut entries = tokio::fs::read_dir(directory)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let mut paths = Vec::new();
    let mut total_size = 0_u64;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| ApiError::Internal(error.into()))?
    {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("png") {
            continue;
        }
        let metadata = entry
            .metadata()
            .await
            .map_err(|error| ApiError::Internal(error.into()))?;
        total_size = total_size.saturating_add(metadata.len());
        paths.push(path);
    }
    paths.sort();
    if paths.is_empty() {
        return Err(ApiError::BadRequest(
            "PDF image export produced no pages".to_string(),
        ));
    }
    if paths.len() > MAX_DOCUMENT_PAGES || total_size > MAX_IMAGE_EXPORT_BYTES {
        return Err(ApiError::BadRequest(
            "exported page images are larger than 120 MiB".to_string(),
        ));
    }
    tokio::task::spawn_blocking(move || {
        let cursor = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (index, path) in paths.iter().enumerate() {
            let bytes = std::fs::read(path).map_err(|error| ApiError::Internal(error.into()))?;
            writer
                .start_file(format!("page-{:03}.png", index + 1), options)
                .map_err(|error| ApiError::Internal(error.into()))?;
            writer
                .write_all(&bytes)
                .map_err(|error| ApiError::Internal(error.into()))?;
        }
        writer
            .finish()
            .map(|cursor| cursor.into_inner())
            .map_err(|error| ApiError::Internal(error.into()))
    })
    .await
    .map_err(|error| ApiError::Internal(error.into()))?
}

fn ensure_system_admin(current: &CurrentUser) -> ApiResult<()> {
    if current.system_role != SystemRole::SystemAdmin {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

fn normalize_authorization_ids(user_ids: Vec<Uuid>) -> ApiResult<BTreeSet<Uuid>> {
    let user_ids = user_ids.into_iter().collect::<BTreeSet<_>>();
    if user_ids.len() > 500 {
        return Err(ApiError::BadRequest(
            "at most 500 seal authorization users are allowed".to_string(),
        ));
    }
    Ok(user_ids)
}

async fn ensure_seal_exists(
    state: &AppState,
    organization_id: Uuid,
    seal_id: Uuid,
) -> ApiResult<()> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_seals WHERE id = $1 AND organization_id = $2)",
    )
    .bind(seal_id)
    .bind(organization_id)
    .fetch_one(&state.db)
    .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

async fn load_seal_authorization_users(
    state: &AppState,
    organization_id: Uuid,
    seal_id: Uuid,
) -> ApiResult<Vec<SealAuthorizationUserView>> {
    sqlx::query_as::<_, SealAuthorizationUserView>(
        r#"
        SELECT account.id AS user_id,
               account.display_name,
               account.email,
               department.name AS department_name,
               EXISTS (
                 SELECT 1
                 FROM seal_authorizations seal_grant
                 WHERE seal_grant.seal_id = $2
                   AND seal_grant.organization_id = account.organization_id
                   AND seal_grant.user_id = account.id
               ) AS authorized
        FROM users account
        LEFT JOIN departments department
          ON department.id = account.department_id
         AND department.organization_id = account.organization_id
        WHERE account.organization_id = $1
          AND account.system_role = 'member'
          AND account.is_active = TRUE
        ORDER BY authorized DESC, lower(account.display_name), lower(account.email), account.id
        "#,
    )
    .bind(organization_id)
    .bind(seal_id)
    .fetch_all(&state.db)
    .await
    .map_err(Into::into)
}

async fn load_document(
    state: &AppState,
    current: &CurrentUser,
    id: Uuid,
) -> ApiResult<SigningDocumentRow> {
    sqlx::query_as::<_, SigningDocumentRow>(&document_select(
        "WHERE id = $1 AND organization_id = $2 AND owner_id = $3",
    ))
    .bind(id)
    .bind(current.organization_id)
    .bind(current.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)
}

fn document_select(suffix: &str) -> String {
    format!(
        r#"
        SELECT id, organization_id, owner_id, original_filename, source_format,
               source_pdf_path, source_size_bytes, page_count, signed_pdf_path,
               placements, signed_at, created_at, updated_at
        FROM signing_documents
        {suffix}
        "#
    )
}

async fn read_existing_file(path: &Path) -> ApiResult<Vec<u8>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(ApiError::NotFound),
        Err(error) => Err(ApiError::Internal(error.into())),
    }
}

fn validate_seal_generator_settings(settings: &SealGeneratorSettings) -> ApiResult<()> {
    if settings.schema_version != 2 {
        return Err(ApiError::BadRequest(
            "seal template schema version is invalid".to_string(),
        ));
    }
    if !matches!(settings.color.as_str(), "#d6212e" | "#ef2a2d" | "#b91f2b") {
        return Err(ApiError::BadRequest(
            "seal template color is invalid".to_string(),
        ));
    }
    let ranges = [
        (settings.wear, 0, 30),
        (settings.outer_border_width, 8, 24),
        (settings.organization_font_size, 38, 68),
        (settings.organization_spacing, -12, 18),
        (settings.organization_inset, 0, 70),
        (settings.organization_height, 90, 180),
        (settings.organization_width, 65, 115),
        (settings.text_stroke_width, 0, 6),
        (settings.star_size, 70, 140),
        (settings.purpose_offset_y, -70, 70),
        (settings.purpose_font_size, 28, 64),
        (settings.purpose_spacing, -6, 18),
        (settings.serial_font_size, 18, 36),
        (settings.serial_spacing, -4, 16),
        (settings.serial_inset, 18, 90),
        (settings.serial_width, 60, 110),
    ];
    if ranges
        .into_iter()
        .any(|(value, minimum, maximum)| value < minimum || value > maximum)
    {
        return Err(ApiError::BadRequest(
            "seal template setting is out of range".to_string(),
        ));
    }
    Ok(())
}

fn validate_seal_generator_definition(definition: &SealGeneratorDefinition) -> ApiResult<()> {
    if definition.schema_version != 1 {
        return Err(ApiError::BadRequest(
            "seal generator definition version is invalid".to_string(),
        ));
    }
    let organization_length = definition.organization.trim().chars().count();
    if !(1..=30).contains(&organization_length)
        || definition.purpose.trim().chars().count() > 14
        || definition.serial.trim().chars().count() > 32
    {
        return Err(ApiError::BadRequest(
            "seal generator text is invalid".to_string(),
        ));
    }
    validate_seal_generator_settings(&definition.settings)
}

fn seal_source_parts(
    definition: Option<&SealGeneratorDefinition>,
) -> ApiResult<(&'static str, Option<Value>)> {
    let Some(definition) = definition else {
        return Ok(("upload", None));
    };
    validate_seal_generator_definition(definition)?;
    let value =
        serde_json::to_value(definition).map_err(|error| ApiError::Internal(error.into()))?;
    Ok(("generated", Some(value)))
}

fn required_name(value: &str) -> ApiResult<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 120 {
        return Err(ApiError::BadRequest(
            "seal name must contain 1 to 120 characters".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn safe_filename(value: &str, fallback: &str) -> String {
    let value = value
        .chars()
        .filter(|character| !character.is_control())
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '.' | '-' | '_' | ' ') {
                character
            } else {
                '_'
            }
        })
        .take(240)
        .collect::<String>();
    let mut value = value;
    while value.contains("..") {
        value = value.replace("..", "");
    }
    let mut collapsed = String::with_capacity(value.len());
    for character in value.chars() {
        if character == '_' && collapsed.ends_with('_') {
            continue;
        }
        collapsed.push(character);
    }
    let value = collapsed.trim_matches([' ', '.']);
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn file_stem(filename: &str) -> String {
    Path::new(filename)
        .file_stem()
        .and_then(|value| value.to_str())
        .map(|value| safe_filename(value, "signed-document"))
        .unwrap_or_else(|| "signed-document".to_string())
}

fn signed_filename(filename: &str) -> String {
    format!("{}-signed.pdf", file_stem(filename))
}

fn image_export_filename(filename: &str, page_count: i32) -> String {
    if page_count == 1 {
        format!("{}-signed.png", file_stem(filename))
    } else {
        format!("{}-images.zip", file_stem(filename))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FinalizeSigningRequest, IMAGE_EXPORT_DPI, SealGeneratorDefinition, SealGeneratorSettings,
        SealImageOptions, SeamPlacement, SigningMode, StampPlacement, document_format,
        ensure_document_not_finalized, ensure_system_admin, image_export_filename, image_to_pdf,
        inherited_page_box, normalize_authorization_ids, normalize_seal_image, safe_filename,
        seal_source_parts, stamp_pdf, validate_finalize_request,
        validate_seal_generator_definition, validate_seal_generator_settings,
    };
    use crate::{
        auth::CurrentUser,
        error::ApiError,
        models::{SystemRole, UserRole},
    };
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use lopdf::{Dictionary, Document, Object, Stream, dictionary};
    use std::{collections::HashMap, fs, io::Cursor, process::Command};
    use uuid::Uuid;

    fn current_user(system_role: SystemRole) -> CurrentUser {
        CurrentUser {
            session_id: Uuid::new_v4(),
            id: Uuid::new_v4(),
            organization_id: Uuid::new_v4(),
            department_id: None,
            email: "employee@example.com".to_string(),
            display_name: "Employee".to_string(),
            role: UserRole::ProjectMember,
            system_role,
        }
    }

    #[test]
    fn only_system_administrators_can_manage_seals() {
        assert!(ensure_system_admin(&current_user(SystemRole::SystemAdmin)).is_ok());
        assert!(matches!(
            ensure_system_admin(&current_user(SystemRole::Member)),
            Err(ApiError::Forbidden)
        ));
    }

    #[test]
    fn seal_authorization_ids_are_deduplicated_and_bounded() {
        let user_id = Uuid::new_v4();
        let normalized = normalize_authorization_ids(vec![user_id, user_id]).unwrap();
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized.first(), Some(&user_id));
        assert!(normalize_authorization_ids((0..501).map(|_| Uuid::new_v4()).collect()).is_err());
    }

    #[test]
    fn seal_authorization_migration_enforces_organization_boundaries() {
        let migration = include_str!("../migrations/202607250019_seal_authorizations.sql");
        assert!(migration.contains("UNIQUE (id, organization_id)"));
        assert!(migration.contains("FOREIGN KEY (seal_id, organization_id)"));
        assert!(migration.contains("FOREIGN KEY (user_id, organization_id)"));
        assert!(migration.contains("REFERENCES user_seals(id, organization_id) ON DELETE CASCADE"));
        assert!(migration.contains("WHERE owner.system_role = 'member'"));
    }

    #[test]
    fn image_exports_use_print_quality_and_direct_png_for_single_pages() {
        assert_eq!(IMAGE_EXPORT_DPI, "300");
        assert_eq!(
            image_export_filename("contract.pdf", 1),
            "contract-signed.png"
        );
        assert_eq!(
            image_export_filename("contract.pdf", 2),
            "contract-images.zip"
        );
    }

    #[test]
    fn document_magic_must_match_extension() {
        assert_eq!(document_format("sample.pdf", b"%PDF-1.7\n").unwrap(), "pdf");
        assert_eq!(
            document_format("sample.png", b"\x89PNG\r\n\x1a\n").unwrap(),
            "png"
        );
        assert_eq!(
            document_format("sample.jpeg", &[0xff, 0xd8, 0xff, 0xe0]).unwrap(),
            "jpg"
        );
        assert_eq!(
            document_format("sample.webp", b"RIFF\x04\0\0\0WEBP").unwrap(),
            "webp"
        );
        assert!(document_format("sample.docx", b"not-a-zip").is_err());
        assert!(document_format("sample.png", b"not-a-png").is_err());
        assert!(document_format("sample.svg", b"<svg/>").is_err());
    }

    #[test]
    fn image_sources_become_single_page_pdfs_with_matching_aspect_ratio() {
        let directory = std::env::temp_dir().join(format!("crewdock-image-pdf-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let source = RgbaImage::from_pixel(640, 480, Rgba([244, 246, 248, 255]));
        for (extension, format) in [
            ("png", ImageFormat::Png),
            ("jpg", ImageFormat::Jpeg),
            ("webp", ImageFormat::WebP),
        ] {
            let output_path = directory.join(format!("source-{extension}.pdf"));
            let mut encoded = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(source.clone())
                .write_to(&mut encoded, format)
                .unwrap();
            let bytes = encoded.into_inner();
            assert_eq!(
                document_format(&format!("source.{extension}"), &bytes).unwrap(),
                extension
            );
            image_to_pdf(&bytes, &output_path).unwrap();

            let document = Document::load(&output_path).unwrap();
            let pages = document.get_pages();
            assert_eq!(pages.len(), 1);
            let page_id = *pages.get(&1).unwrap();
            let page_box = inherited_page_box(&document, page_id).unwrap();
            assert_eq!(page_box, [0.0, 0.0, 480.0, 360.0]);
            assert_eq!(document.get_page_images(page_id).unwrap().len(), 1);
        }

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn signing_image_source_formats_are_migrated() {
        let migration = include_str!("../migrations/202607250017_signing_image_sources.sql");
        assert!(migration.contains("DROP CONSTRAINT signing_documents_source_format_valid"));
        assert!(migration.contains("'png', 'jpg', 'webp'"));
    }

    #[test]
    fn signing_coordinates_and_modes_are_strict() {
        let seal_id = Uuid::new_v4();
        let single = FinalizeSigningRequest {
            mode: SigningMode::Single,
            placements: vec![StampPlacement {
                seal_id,
                page: 1,
                x: 0.7,
                y: 0.7,
                width: 0.2,
                height: 0.2,
            }],
            seam: None,
        };
        assert!(validate_finalize_request(&single, 2).is_ok());
        let invalid = FinalizeSigningRequest {
            mode: SigningMode::Single,
            placements: vec![StampPlacement {
                x: 0.9,
                ..single.placements[0].clone()
            }],
            seam: None,
        };
        assert!(validate_finalize_request(&invalid, 2).is_err());
        let seam = FinalizeSigningRequest {
            mode: SigningMode::Seam,
            placements: vec![],
            seam: Some(SeamPlacement {
                seal_id,
                start_page: 1,
                end_page: 2,
                y: 0.35,
                height: 0.3,
            }),
        };
        assert!(validate_finalize_request(&seam, 2).is_ok());
    }

    #[test]
    fn finalized_signing_documents_cannot_be_overwritten() {
        assert!(ensure_document_not_finalized(None).is_ok());
        assert!(matches!(
            ensure_document_not_finalized(Some("signing/result.pdf")),
            Err(ApiError::Conflict(message))
                if message == "signing document has already been finalized"
        ));
    }

    #[test]
    fn seal_generator_template_defaults_and_ranges_are_strict() {
        let defaults = SealGeneratorSettings::default();
        assert!(validate_seal_generator_settings(&defaults).is_ok());

        let invalid_color = SealGeneratorSettings {
            color: "#ff0000".to_string(),
            ..defaults.clone()
        };
        assert!(validate_seal_generator_settings(&invalid_color).is_err());

        let invalid_spacing = SealGeneratorSettings {
            serial_spacing: 17,
            ..defaults
        };
        assert!(validate_seal_generator_settings(&invalid_spacing).is_err());
    }

    #[test]
    fn seal_generator_preferences_are_scoped_to_owner_and_organization() {
        let migration = include_str!("../migrations/202607250016_seal_generator_preferences.sql");
        assert!(migration.contains("PRIMARY KEY (organization_id, owner_id)"));
        assert!(migration.contains("FOREIGN KEY (owner_id, organization_id)"));
        assert!(migration.contains("REFERENCES users(id, organization_id) ON DELETE CASCADE"));
        assert!(migration.contains("CHECK (jsonb_typeof(settings) = 'object')"));
    }

    #[test]
    fn generated_seals_require_a_valid_editable_definition() {
        let definition = SealGeneratorDefinition {
            schema_version: 1,
            organization: "示例网络科技有限公司".to_string(),
            purpose: "合同专用章".to_string(),
            serial: "1234567890123".to_string(),
            settings: SealGeneratorSettings::default(),
        };
        assert!(validate_seal_generator_definition(&definition).is_ok());
        let (source_kind, value) = seal_source_parts(Some(&definition)).unwrap();
        assert_eq!(source_kind, "generated");
        assert_eq!(value.unwrap()["organization"], definition.organization);

        let invalid = SealGeneratorDefinition {
            organization: String::new(),
            ..definition
        };
        assert!(validate_seal_generator_definition(&invalid).is_err());
    }

    #[test]
    fn editable_seal_migration_guards_sources_and_upgrades_templates() {
        let migration = include_str!("../migrations/202607250018_editable_seal_sources.sql");
        assert!(migration.contains("source_kind IN ('upload', 'generated', 'legacy_generated')"));
        assert!(
            migration.contains("source_kind <> 'generated' OR generator_definition IS NOT NULL")
        );
        assert!(migration.contains("'schema_version', 2"));
        assert!(migration.contains("size_bytes >= 32768"));
    }

    #[test]
    fn filenames_cannot_escape_storage_or_headers() {
        assert_eq!(
            safe_filename("../../合同?.pdf", "fallback.pdf"),
            "_合同_.pdf"
        );
        assert_eq!(safe_filename("..", "fallback.pdf"), "fallback.pdf");
    }

    fn encode_png(image: RgbaImage) -> Vec<u8> {
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        encoded.into_inner()
    }

    #[test]
    fn seal_cleanup_removes_white_background_without_losing_red_ink() {
        let mut source = RgbaImage::from_pixel(64, 64, Rgba([255, 255, 255, 255]));
        for y in 20..44 {
            for x in 18..46 {
                source.put_pixel(x, y, Rgba([218, 28, 40, 255]));
            }
        }
        let normalized = normalize_seal_image(
            &encode_png(source),
            SealImageOptions {
                remove_background: true,
                red_ink_only: false,
                ink_texture: false,
            },
        )
        .unwrap();
        let result = image::load_from_memory_with_format(&normalized.png_bytes, ImageFormat::Png)
            .unwrap()
            .into_rgba8();
        assert_eq!(result.get_pixel(0, 0).0[3], 0);
        assert!(result.get_pixel(30, 30).0[3] > 220);
        assert!(result.get_pixel(30, 30).0[0] > result.get_pixel(30, 30).0[1]);
    }

    #[test]
    fn red_only_cleanup_removes_black_overlay() {
        let mut source = RgbaImage::from_pixel(64, 64, Rgba([255, 255, 255, 255]));
        for y in 12..52 {
            for x in 10..30 {
                source.put_pixel(x, y, Rgba([210, 22, 34, 255]));
            }
            for x in 36..54 {
                source.put_pixel(x, y, Rgba([25, 25, 25, 255]));
            }
        }
        let normalized = normalize_seal_image(
            &encode_png(source),
            SealImageOptions {
                remove_background: true,
                red_ink_only: true,
                ink_texture: false,
            },
        )
        .unwrap();
        let result = image::load_from_memory_with_format(&normalized.png_bytes, ImageFormat::Png)
            .unwrap()
            .into_rgba8();
        assert!(result.get_pixel(20, 30).0[3] > 220);
        assert_eq!(result.get_pixel(45, 30).0[3], 0);
    }

    #[test]
    fn single_stamp_produces_a_readable_pdf_with_an_image() {
        let directory = std::env::temp_dir().join(format!("crewdock-signing-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let source_path = directory.join("source.pdf");
        let output_path = directory.join("signed.pdf");

        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let content_id = document.add_object(Stream::new(Dictionary::new(), Vec::new()));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1,
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);
        document.save(&source_path).unwrap();

        let seal_id = Uuid::new_v4();
        let seal = RgbaImage::from_pixel(64, 64, Rgba([210, 24, 36, 160]));
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(seal)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        let seal_bytes = HashMap::from([(seal_id, encoded.into_inner())]);
        let request = FinalizeSigningRequest {
            mode: SigningMode::Single,
            placements: vec![StampPlacement {
                seal_id,
                page: 1,
                x: 0.65,
                y: 0.65,
                width: 0.2,
                height: 0.2,
            }],
            seam: None,
        };

        stamp_pdf(&source_path, &output_path, &request, &seal_bytes).unwrap();
        let signed = Document::load(&output_path).unwrap();
        let signed_page_id = *signed.get_pages().get(&1).unwrap();
        assert_eq!(signed.get_page_images(signed_page_id).unwrap().len(), 1);
        assert!(signed.objects.values().any(|object| {
            let Object::Stream(stream) = object else {
                return false;
            };
            stream.dict.get(b"Subtype").ok() == Some(&Object::Name(b"Image".to_vec()))
                && stream.dict.get(b"SMask").is_ok()
        }));
        assert!(
            signed
                .get_page_content(signed_page_id)
                .unwrap()
                .windows(2)
                .any(|value| value == b"Do")
        );

        if let Ok(renderer) = std::env::var("CREWDOCK_TEST_PDFTOPPM_BIN") {
            let render_prefix = directory.join("rendered");
            let status = Command::new(renderer)
                .args(["-f", "1", "-l", "1", "-singlefile", "-r", "72", "-png"])
                .arg(&output_path)
                .arg(&render_prefix)
                .status()
                .unwrap();
            assert!(status.success());
            let rendered = image::open(render_prefix.with_extension("png"))
                .unwrap()
                .into_rgba8();
            let outside = rendered.get_pixel(24, 24).0;
            assert!(outside[0] > 248 && outside[1] > 248 && outside[2] > 248);
            let inside = rendered.get_pixel(446, 632).0;
            assert!(inside[0] > inside[1] + 40 && inside[0] > inside[2] + 40);
        }

        fs::remove_dir_all(directory).unwrap();
    }
}
