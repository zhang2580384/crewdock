use crate::{
    access,
    app::AppState,
    audit::log_event_in_transaction,
    auth::{self, CurrentUser, hash_password, hash_token, normalize_email, validate_password},
    email_templates::{self, TemplateKey},
    error::{ApiError, ApiResult},
    mail,
    models::SystemRole,
    public_url::base_url_from_request,
    response::{ApiEnvelope, ok},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use rand::Rng;
use redis::Script;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use sqlx::{FromRow, Postgres, Transaction};
use std::collections::{BTreeMap, HashSet};
use uuid::Uuid;

const REGISTRATION_COLUMNS: &str = r#"
    id, organization_id, email, display_name, department_interest, reason,
    status::text AS status, email_verified_at, reviewed_at, rejection_reason,
    approved_department_ids, primary_department_id, activated_user_id, created_at, updated_at
"#;
const VERIFICATION_TTL_MINUTES: i64 = 15;
const VERIFICATION_RESEND_COOLDOWN_SECONDS: i64 = 60;
const VERIFICATION_MAX_ATTEMPTS: i32 = 5;
const ACTIVATION_TTL_HOURS: i64 = 72;
const REGISTRATION_IP_LIMIT: i64 = 8;
const REGISTRATION_EMAIL_LIMIT: i64 = 4;
const REFRESH_EXPIRED_VERIFICATION_SQL: &str = r#"
    UPDATE registration_requests
    SET display_name = $2, department_interest = $3, reason = $4,
        verification_token_hash = $5, verification_expires_at = $6,
        verification_attempts = 0, verification_sent_at = now(), updated_at = now()
    WHERE organization_id = $1 AND lower(email) = lower($7)
      AND status = 'pending_verification' AND verification_expires_at <= now()
    RETURNING id
"#;
const REFRESH_ACTIVE_INVITATION_SQL: &str = r#"
    UPDATE registration_requests
    SET display_name = $3, activation_token_hash = $4, activation_expires_at = $5,
        reviewed_by = $6, reviewed_at = now(), approved_department_ids = $7,
        primary_department_id = $8, updated_at = now()
    WHERE organization_id = $1 AND lower(email) = lower($2)
      AND status = 'approved'
    RETURNING id
"#;

#[derive(Debug, Serialize)]
pub struct RegistrationPolicyView {
    pub registration_mode: String,
    pub allow_all_email_domains: bool,
    pub allowed_email_domains: Vec<String>,
    pub email_ready: bool,
    pub verification_expires_in_minutes: i64,
    pub resend_cooldown_seconds: i64,
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub display_name: String,
    pub department_interest: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AcceptedResponse {
    pub accepted: bool,
    pub verification_expires_in_minutes: i64,
    pub resend_cooldown_seconds: i64,
}

#[derive(Debug, Deserialize)]
pub struct VerifyEmailRequest {
    pub email: String,
    pub code: String,
}

#[derive(Debug, Serialize)]
pub struct VerifyEmailResponse {
    pub verified: bool,
}

#[derive(Debug, Deserialize)]
pub struct ResendVerificationRequest {
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct ActivateRequest {
    pub token: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct ActivateResponse {
    pub activated: bool,
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct ApproveRegistrationRequest {
    pub department_ids: Vec<Uuid>,
    pub primary_department_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct RejectRegistrationRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateInvitationRequest {
    pub email: String,
    pub display_name: String,
    pub department_ids: Vec<Uuid>,
    pub primary_department_id: Uuid,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct RegistrationRequestView {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub department_interest: Option<String>,
    pub reason: Option<String>,
    pub status: String,
    pub email_verified_at: Option<DateTime<Utc>>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub rejection_reason: Option<String>,
    pub approved_department_ids: Vec<Uuid>,
    pub primary_department_id: Option<Uuid>,
    pub activated_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct RegistrationPolicyRow {
    organization_id: Uuid,
    registration_mode: String,
    allow_all_email_domains: bool,
    allowed_email_domains: Vec<String>,
}

#[derive(Debug, FromRow)]
struct ActivationRow {
    id: Uuid,
    organization_id: Uuid,
    email: String,
    display_name: String,
    approved_department_ids: Vec<Uuid>,
    primary_department_id: Option<Uuid>,
}

pub async fn registration_policy(
    State(state): State<AppState>,
) -> ApiResult<Json<ApiEnvelope<RegistrationPolicyView>>> {
    let policy = load_registration_policy(&state).await?;
    let email_ready = email_is_ready(&state, policy.organization_id).await?;
    Ok(ok(RegistrationPolicyView {
        registration_mode: policy.registration_mode,
        allow_all_email_domains: policy.allow_all_email_domains,
        allowed_email_domains: policy.allowed_email_domains,
        email_ready,
        verification_expires_in_minutes: VERIFICATION_TTL_MINUTES,
        resend_cooldown_seconds: VERIFICATION_RESEND_COOLDOWN_SECONDS,
    }))
}

pub async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<RegisterRequest>,
) -> ApiResult<(StatusCode, Json<ApiEnvelope<AcceptedResponse>>)> {
    let email = normalize_email(&payload.email)?;
    let display_name = required_text(&payload.display_name, "display_name", 120)?;
    let department_interest =
        optional_text(payload.department_interest, "department_interest", 120)?;
    let reason = optional_text(payload.reason, "reason", 1000)?;

    let policy = load_registration_policy(&state).await?;
    if policy.registration_mode != "application" {
        return Err(ApiError::Forbidden);
    }
    if !email_is_ready(&state, policy.organization_id).await? {
        return Err(ApiError::Unavailable("email delivery"));
    }
    if !email_domain_is_allowed(
        &email,
        policy.allow_all_email_domains,
        &policy.allowed_email_domains,
    ) {
        return Err(ApiError::BadRequest(
            "email domain is not allowed".to_string(),
        ));
    }
    enforce_registration_rate_limit(&state, &headers, &email).await?;

    let accepted = || {
        (
            StatusCode::ACCEPTED,
            ok(AcceptedResponse {
                accepted: true,
                verification_expires_in_minutes: VERIFICATION_TTL_MINUTES,
                resend_cooldown_seconds: VERIFICATION_RESEND_COOLDOWN_SECONDS,
            }),
        )
    };
    let existing_user: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE lower(email) = lower($1))")
            .bind(&email)
            .fetch_one(&state.db)
            .await?;
    if existing_user {
        return Ok(accepted());
    }

    let verification_code = random_verification_code();
    let code_hash =
        verification_code_hash(&email, &verification_code, &state.config.session_secret);
    let expires_at = Utc::now() + Duration::minutes(VERIFICATION_TTL_MINUTES);
    let mut transaction = state.db.begin().await?;
    access::lock_organization_department_tree(&mut transaction, policy.organization_id).await?;
    auth::lock_email_identity_in_transaction(&mut transaction, policy.organization_id, &email)
        .await?;
    let existing_user: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE lower(email) = lower($1))")
            .bind(&email)
            .fetch_one(&mut *transaction)
            .await?;
    if existing_user {
        transaction.rollback().await?;
        return Ok(accepted());
    }
    let refreshed_request_id: Option<Uuid> = sqlx::query_scalar(REFRESH_EXPIRED_VERIFICATION_SQL)
        .bind(policy.organization_id)
        .bind(&display_name)
        .bind(&department_interest)
        .bind(&reason)
        .bind(&code_hash)
        .bind(expires_at)
        .bind(&email)
        .fetch_optional(&mut *transaction)
        .await?;
    let request_id: Option<Uuid> = if refreshed_request_id.is_some() {
        refreshed_request_id
    } else {
        sqlx::query_scalar(
            r#"
        INSERT INTO registration_requests (
          organization_id, email, display_name, department_interest, reason, status,
          verification_token_hash, verification_expires_at,
          verification_attempts, verification_sent_at
        )
        VALUES ($1, $2, $3, $4, $5, 'pending_verification', $6, $7, 0, now())
        ON CONFLICT DO NOTHING
        RETURNING id
        "#,
        )
        .bind(policy.organization_id)
        .bind(&email)
        .bind(&display_name)
        .bind(department_interest)
        .bind(reason)
        .bind(&code_hash)
        .bind(expires_at)
        .fetch_optional(&mut *transaction)
        .await?
    };
    let Some(request_id) = request_id else {
        transaction.rollback().await?;
        return Ok(accepted());
    };

    let site_name =
        organization_name_in_transaction(&mut transaction, policy.organization_id).await?;
    let template = email_templates::render_in_transaction(
        &mut transaction,
        policy.organization_id,
        TemplateKey::RegistrationVerification,
        &BTreeMap::from([
            ("recipient_name", display_name.clone()),
            ("verification_code", verification_code.clone()),
            ("expires_in_minutes", VERIFICATION_TTL_MINUTES.to_string()),
            ("site_name", site_name),
        ]),
    )
    .await?;
    mail::enqueue_html_email_in_transaction(
        &mut transaction,
        state.crypto.as_ref(),
        policy.organization_id,
        &email,
        &template.subject,
        &template.html_body,
        &format!("registration.verify:{request_id}:{}", &code_hash[..12]),
    )
    .await?;
    log_event_in_transaction(
        &mut transaction,
        policy.organization_id,
        None,
        "registration.request",
        "registration_request",
        Some(request_id),
        json!({ "email_domain": email_domain(&email) }),
    )
    .await?;
    transaction.commit().await?;
    Ok(accepted())
}

pub async fn verify_email(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<VerifyEmailRequest>,
) -> ApiResult<Json<ApiEnvelope<VerifyEmailResponse>>> {
    let email = normalize_email(&payload.email)?;
    let code = validate_verification_code(&payload.code)?;
    enforce_verification_rate_limit(&state, &headers, &email).await?;
    let mut transaction = state.db.begin().await?;
    let request: Option<(Uuid, Uuid, String, i32)> = sqlx::query_as(
        r#"
        SELECT id, organization_id, verification_token_hash, verification_attempts
        FROM registration_requests
        WHERE lower(email) = lower($1) AND status = 'pending_verification'
          AND verification_expires_at > now()
        FOR UPDATE
        "#,
    )
    .bind(&email)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some((request_id, organization_id, stored_hash, attempts)) = request else {
        return Err(ApiError::BadRequest(
            "verification code is invalid or expired".to_string(),
        ));
    };
    if attempts >= VERIFICATION_MAX_ATTEMPTS
        || !verify_verification_code_hash(&email, code, &state.config.session_secret, &stored_hash)
    {
        sqlx::query(
            r#"
            UPDATE registration_requests
            SET verification_attempts = LEAST(verification_attempts + 1, $2),
                verification_expires_at = CASE
                  WHEN verification_attempts + 1 >= $2 THEN now()
                  ELSE verification_expires_at
                END,
                updated_at = now()
            WHERE id = $1 AND status = 'pending_verification'
            "#,
        )
        .bind(request_id)
        .bind(VERIFICATION_MAX_ATTEMPTS)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        return Err(ApiError::BadRequest(
            "verification code is invalid or expired".to_string(),
        ));
    }
    sqlx::query(
        r#"
        UPDATE registration_requests
        SET status = 'pending_review', email_verified_at = now(),
            verification_token_hash = NULL, verification_expires_at = NULL,
            verification_attempts = 0, verification_sent_at = NULL, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(request_id)
    .execute(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        organization_id,
        None,
        "registration.verify_email",
        "registration_request",
        Some(request_id),
        json!({ "email_domain": email_domain(&email) }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(VerifyEmailResponse { verified: true }))
}

pub async fn resend_verification(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<ResendVerificationRequest>,
) -> ApiResult<(StatusCode, Json<ApiEnvelope<AcceptedResponse>>)> {
    let email = normalize_email(&payload.email)?;
    let policy = load_registration_policy(&state).await?;
    if policy.registration_mode != "application" {
        return Err(ApiError::Forbidden);
    }
    if !email_is_ready(&state, policy.organization_id).await? {
        return Err(ApiError::Unavailable("email delivery"));
    }
    enforce_resend_rate_limit(&state, &headers, &email).await?;
    let accepted = || {
        (
            StatusCode::ACCEPTED,
            ok(AcceptedResponse {
                accepted: true,
                verification_expires_in_minutes: VERIFICATION_TTL_MINUTES,
                resend_cooldown_seconds: VERIFICATION_RESEND_COOLDOWN_SECONDS,
            }),
        )
    };
    let mut transaction = state.db.begin().await?;
    let request: Option<(Uuid, String, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT id, display_name, verification_sent_at
        FROM registration_requests
        WHERE organization_id = $1 AND lower(email) = lower($2)
          AND status = 'pending_verification'
        FOR UPDATE
        "#,
    )
    .bind(policy.organization_id)
    .bind(&email)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some((request_id, display_name, sent_at)) = request else {
        transaction.rollback().await?;
        return Ok(accepted());
    };
    if sent_at > Utc::now() - Duration::seconds(VERIFICATION_RESEND_COOLDOWN_SECONDS) {
        return Err(ApiError::RateLimited);
    }
    let verification_code = random_verification_code();
    let code_hash =
        verification_code_hash(&email, &verification_code, &state.config.session_secret);
    let expires_at = Utc::now() + Duration::minutes(VERIFICATION_TTL_MINUTES);
    sqlx::query(
        r#"
        UPDATE registration_requests
        SET verification_token_hash = $2, verification_expires_at = $3,
            verification_attempts = 0, verification_sent_at = now(), updated_at = now()
        WHERE id = $1 AND status = 'pending_verification'
        "#,
    )
    .bind(request_id)
    .bind(&code_hash)
    .bind(expires_at)
    .execute(&mut *transaction)
    .await?;
    let site_name =
        organization_name_in_transaction(&mut transaction, policy.organization_id).await?;
    let template = email_templates::render_in_transaction(
        &mut transaction,
        policy.organization_id,
        TemplateKey::RegistrationVerification,
        &BTreeMap::from([
            ("recipient_name", display_name),
            ("verification_code", verification_code),
            ("expires_in_minutes", VERIFICATION_TTL_MINUTES.to_string()),
            ("site_name", site_name),
        ]),
    )
    .await?;
    mail::enqueue_html_email_in_transaction(
        &mut transaction,
        state.crypto.as_ref(),
        policy.organization_id,
        &email,
        &template.subject,
        &template.html_body,
        &format!("registration.verify:{request_id}:{}", &code_hash[..12]),
    )
    .await?;
    log_event_in_transaction(
        &mut transaction,
        policy.organization_id,
        None,
        "registration.verification_resend",
        "registration_request",
        Some(request_id),
        json!({ "email_domain": email_domain(&email) }),
    )
    .await?;
    transaction.commit().await?;
    Ok(accepted())
}

pub async fn activate(
    State(state): State<AppState>,
    Json(payload): Json<ActivateRequest>,
) -> ApiResult<Json<ApiEnvelope<ActivateResponse>>> {
    let token = validate_token(&payload.token)?;
    let token_hash = hash_token(token, &state.config.session_secret);
    let candidate = activation_candidate(&state, &token_hash).await?;
    let password = validate_password(&payload.password)?;
    let password_hash = hash_password(&password).map_err(ApiError::Internal)?;
    let mut transaction = state.db.begin().await?;
    access::lock_organization_department_tree(&mut transaction, candidate.organization_id).await?;
    auth::lock_email_identity_in_transaction(
        &mut transaction,
        candidate.organization_id,
        &candidate.email,
    )
    .await?;
    let request = sqlx::query_as::<_, ActivationRow>(
        r#"
        SELECT id, organization_id, email, display_name,
               approved_department_ids, primary_department_id
        FROM registration_requests
        WHERE id = $1 AND activation_token_hash = $2
          AND status = 'approved' AND activation_expires_at > now()
        FOR UPDATE
        "#,
    )
    .bind(candidate.id)
    .bind(&token_hash)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| ApiError::BadRequest("activation token is invalid or expired".to_string()))?;
    let primary_department_id = request.primary_department_id.ok_or_else(|| {
        ApiError::Conflict("approved registration has no primary department".to_string())
    })?;
    let department_ids = validate_departments(
        &mut transaction,
        request.organization_id,
        request.approved_department_ids,
        primary_department_id,
    )
    .await
    .map_err(|error| match error {
        ApiError::BadRequest(_) => ApiError::Conflict(
            "approved departments are no longer available; contact an administrator".to_string(),
        ),
        error => error,
    })?;
    let existing: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE lower(email) = lower($1))")
            .bind(&request.email)
            .fetch_one(&mut *transaction)
            .await?;
    if existing {
        return Err(ApiError::Conflict("account already exists".to_string()));
    }

    let user_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO users (
          organization_id, department_id, email, display_name, password_hash, system_role
        )
        VALUES ($1, $2, $3, $4, $5, 'member')
        RETURNING id
        "#,
    )
    .bind(request.organization_id)
    .bind(primary_department_id)
    .bind(&request.email)
    .bind(&request.display_name)
    .bind(password_hash)
    .fetch_one(&mut *transaction)
    .await
    .map_err(auth::map_email_identity_error)?;
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
    .bind(request.organization_id)
    .bind(primary_department_id)
    .bind(&department_ids)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE registration_requests
        SET status = 'activated', activated_user_id = $2,
            activation_token_hash = NULL, activation_expires_at = NULL,
            approved_department_ids = '{}', primary_department_id = NULL, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(request.id)
    .bind(user_id)
    .execute(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        request.organization_id,
        None,
        "registration.activate",
        "user",
        Some(user_id),
        json!({
            "registration_request_id": request.id,
            "department_ids": department_ids,
            "primary_department_id": primary_department_id,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(ActivateResponse {
        activated: true,
        email: request.email,
    }))
}

pub async fn list_pending_requests(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<Vec<RegistrationRequestView>>>> {
    require_system_admin(&current)?;
    let rows = sqlx::query_as::<_, RegistrationRequestView>(&format!(
        r#"
        SELECT {REGISTRATION_COLUMNS}
        FROM registration_requests
        WHERE organization_id = $1 AND status = 'pending_review'
        ORDER BY created_at ASC, id
        "#
    ))
    .bind(current.organization_id)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(rows))
}

pub async fn approve_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    current: CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<ApproveRegistrationRequest>,
) -> ApiResult<Json<ApiEnvelope<RegistrationRequestView>>> {
    require_system_admin(&current)?;
    let public_base_url = base_url_from_request(&headers, &state.config.public_base_url);
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    require_system_admin(&actor)?;
    access::lock_organization_department_tree(&mut transaction, actor.organization_id).await?;
    let department_ids = validate_departments(
        &mut transaction,
        actor.organization_id,
        payload.department_ids,
        payload.primary_department_id,
    )
    .await?;
    let (request_id, email, display_name): (Uuid, String, String) = sqlx::query_as(
        r#"
        SELECT id, email, display_name
        FROM registration_requests
        WHERE id = $1 AND organization_id = $2 AND status = 'pending_review'
        FOR UPDATE NOWAIT
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    auth::lock_email_identity_in_transaction(&mut transaction, actor.organization_id, &email)
        .await?;
    let existing_user: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE lower(email) = lower($1))")
            .bind(&email)
            .fetch_one(&mut *transaction)
            .await?;
    if existing_user {
        return Err(ApiError::Conflict("email already exists".to_string()));
    }
    if !email_is_ready_in_transaction(
        &mut transaction,
        actor.organization_id,
        state.config.master_key != [0u8; 32],
    )
    .await?
    {
        return Err(ApiError::Unavailable("email delivery"));
    }
    let raw_token = random_token();
    let token_hash = hash_token(&raw_token, &state.config.session_secret);
    let activation_expires_at = Utc::now() + Duration::hours(ACTIVATION_TTL_HOURS);
    sqlx::query(
        r#"
        UPDATE registration_requests
        SET status = 'approved', reviewed_by = $2, reviewed_at = now(),
            rejection_reason = NULL, approved_department_ids = $3,
            primary_department_id = $4, activation_token_hash = $5,
            activation_expires_at = $6, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(request_id)
    .bind(actor.id)
    .bind(&department_ids)
    .bind(payload.primary_department_id)
    .bind(token_hash)
    .bind(activation_expires_at)
    .execute(&mut *transaction)
    .await?;
    enqueue_activation_email(
        &state,
        &public_base_url,
        &mut transaction,
        actor.organization_id,
        request_id,
        &email,
        &display_name,
        &actor.display_name,
        &raw_token,
        TemplateKey::RegistrationApproved,
    )
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "registration.approve",
        "registration_request",
        Some(request_id),
        json!({
            "department_ids": department_ids,
            "primary_department_id": payload.primary_department_id,
        }),
    )
    .await?;
    let view = registration_view_in_transaction(&mut transaction, request_id).await?;
    transaction.commit().await?;
    Ok(ok(view))
}

pub async fn reject_request(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<RejectRegistrationRequest>,
) -> ApiResult<Json<ApiEnvelope<RegistrationRequestView>>> {
    require_system_admin(&current)?;
    let rejection_reason = optional_text(payload.reason, "reason", 500)?;
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    require_system_admin(&actor)?;
    access::lock_organization_department_tree(&mut transaction, actor.organization_id).await?;
    let updated: Option<(Uuid, String, String, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"
        UPDATE registration_requests
        SET status = 'rejected', reviewed_by = $3, reviewed_at = now(),
            rejection_reason = $4, verification_token_hash = NULL,
            verification_expires_at = NULL, verification_attempts = 0,
            verification_sent_at = NULL, activation_token_hash = NULL,
            activation_expires_at = NULL, updated_at = now()
        WHERE id = $1 AND organization_id = $2 AND status = 'pending_review'
        RETURNING id, email, display_name, email_verified_at
        "#,
    )
    .bind(id)
    .bind(actor.organization_id)
    .bind(actor.id)
    .bind(&rejection_reason)
    .fetch_optional(&mut *transaction)
    .await?;
    let (request_id, email, display_name, email_verified_at) = updated.ok_or(ApiError::NotFound)?;
    let notification_queued = email_verified_at.is_some()
        && email_is_ready_in_transaction(
            &mut transaction,
            actor.organization_id,
            state.config.master_key != [0u8; 32],
        )
        .await?;
    if notification_queued {
        let site_name =
            organization_name_in_transaction(&mut transaction, actor.organization_id).await?;
        let template = email_templates::render_in_transaction(
            &mut transaction,
            actor.organization_id,
            TemplateKey::RegistrationRejected,
            &BTreeMap::from([
                ("recipient_name", display_name),
                (
                    "rejection_reason",
                    rejection_reason
                        .clone()
                        .unwrap_or_else(|| "管理员未提供具体原因".to_string()),
                ),
                ("site_name", site_name),
            ]),
        )
        .await?;
        mail::enqueue_html_email_in_transaction(
            &mut transaction,
            state.crypto.as_ref(),
            actor.organization_id,
            &email,
            &template.subject,
            &template.html_body,
            &format!("registration.reject:{request_id}"),
        )
        .await?;
    }
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "registration.reject",
        "registration_request",
        Some(request_id),
        json!({
            "reason_supplied": rejection_reason.is_some(),
            "notification_queued": notification_queued,
        }),
    )
    .await?;
    let view = registration_view_in_transaction(&mut transaction, request_id).await?;
    transaction.commit().await?;
    Ok(ok(view))
}

pub async fn create_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    current: CurrentUser,
    Json(payload): Json<CreateInvitationRequest>,
) -> ApiResult<Json<ApiEnvelope<RegistrationRequestView>>> {
    require_system_admin(&current)?;
    let public_base_url = base_url_from_request(&headers, &state.config.public_base_url);
    let email = normalize_email(&payload.email)?;
    let display_name = required_text(&payload.display_name, "display_name", 120)?;
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    require_system_admin(&actor)?;
    access::lock_organization_department_tree(&mut transaction, actor.organization_id).await?;
    let department_ids = validate_departments(
        &mut transaction,
        actor.organization_id,
        payload.department_ids,
        payload.primary_department_id,
    )
    .await?;
    if !email_is_ready_in_transaction(
        &mut transaction,
        actor.organization_id,
        state.config.master_key != [0u8; 32],
    )
    .await?
    {
        return Err(ApiError::Unavailable("email delivery"));
    }
    auth::lock_email_identity_in_transaction(&mut transaction, actor.organization_id, &email)
        .await?;
    let existing_user: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE lower(email) = lower($1))")
            .bind(&email)
            .fetch_one(&mut *transaction)
            .await?;
    if existing_user {
        return Err(ApiError::Conflict("email already exists".to_string()));
    }
    let raw_token = random_token();
    let token_hash = hash_token(&raw_token, &state.config.session_secret);
    let activation_expires_at = Utc::now() + Duration::hours(ACTIVATION_TTL_HOURS);
    let refreshed_request_id: Option<Uuid> = sqlx::query_scalar(REFRESH_ACTIVE_INVITATION_SQL)
        .bind(actor.organization_id)
        .bind(&email)
        .bind(&display_name)
        .bind(&token_hash)
        .bind(activation_expires_at)
        .bind(actor.id)
        .bind(&department_ids)
        .bind(payload.primary_department_id)
        .fetch_optional(&mut *transaction)
        .await?;
    let request_id: Option<Uuid> = if refreshed_request_id.is_some() {
        refreshed_request_id
    } else {
        sqlx::query_scalar(
            r#"
        INSERT INTO registration_requests (
          organization_id, email, display_name, status, email_verified_at,
          activation_token_hash, activation_expires_at, reviewed_by, reviewed_at,
          approved_department_ids, primary_department_id
        )
        VALUES ($1, $2, $3, 'approved', now(), $4, $5, $6, now(), $7, $8)
        ON CONFLICT DO NOTHING
        RETURNING id
        "#,
        )
        .bind(actor.organization_id)
        .bind(&email)
        .bind(&display_name)
        .bind(&token_hash)
        .bind(activation_expires_at)
        .bind(actor.id)
        .bind(&department_ids)
        .bind(payload.primary_department_id)
        .fetch_optional(&mut *transaction)
        .await?
    };
    let request_id = request_id
        .ok_or_else(|| ApiError::Conflict("email already has an active request".to_string()))?;
    enqueue_activation_email(
        &state,
        &public_base_url,
        &mut transaction,
        actor.organization_id,
        request_id,
        &email,
        &display_name,
        &actor.display_name,
        &raw_token,
        TemplateKey::AccountInvitation,
    )
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "registration.invite",
        "registration_request",
        Some(request_id),
        json!({
            "department_ids": department_ids,
            "primary_department_id": payload.primary_department_id,
        }),
    )
    .await?;
    let view = registration_view_in_transaction(&mut transaction, request_id).await?;
    transaction.commit().await?;
    Ok(ok(view))
}

async fn load_registration_policy(state: &AppState) -> ApiResult<RegistrationPolicyRow> {
    let organization_id: Uuid =
        sqlx::query_scalar("SELECT id FROM organizations ORDER BY created_at ASC, id ASC LIMIT 1")
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("no organization configured")))?;
    Ok(sqlx::query_as::<_, RegistrationPolicyRow>(
        r#"
        SELECT $1 AS organization_id,
               COALESCE(settings.registration_mode::text, 'closed') AS registration_mode,
               COALESCE(settings.allow_all_email_domains, false) AS allow_all_email_domains,
               COALESCE(settings.allowed_email_domains, ARRAY[]::text[]) AS allowed_email_domains
        FROM (SELECT 1) singleton
        LEFT JOIN organization_auth_settings settings ON settings.organization_id = $1
        "#,
    )
    .bind(organization_id)
    .fetch_one(&state.db)
    .await?)
}

async fn email_is_ready(state: &AppState, organization_id: Uuid) -> ApiResult<bool> {
    let database_ready: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
          SELECT 1 FROM organization_email_settings
          WHERE organization_id = $1 AND enabled = TRUE
            AND length(trim(smtp_host)) > 0 AND length(trim(from_email)) > 0
            AND smtp_password_encrypted IS NOT NULL
        )
        "#,
    )
    .bind(organization_id)
    .fetch_one(&state.db)
    .await?;
    Ok(database_ready && state.config.master_key != [0u8; 32])
}

async fn email_is_ready_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    master_key_configured: bool,
) -> ApiResult<bool> {
    let database_ready: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
          SELECT 1 FROM organization_email_settings
          WHERE organization_id = $1 AND enabled = TRUE
            AND length(trim(smtp_host)) > 0 AND length(trim(from_email)) > 0
            AND smtp_password_encrypted IS NOT NULL
        )
        "#,
    )
    .bind(organization_id)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(database_ready && master_key_configured)
}

async fn activation_candidate(state: &AppState, token_hash: &str) -> ApiResult<ActivationRow> {
    sqlx::query_as::<_, ActivationRow>(
        r#"
        SELECT id, organization_id, email, display_name,
               approved_department_ids, primary_department_id
        FROM registration_requests
        WHERE activation_token_hash = $1
          AND status = 'approved' AND activation_expires_at > now()
        "#,
    )
    .bind(token_hash)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::BadRequest("activation token is invalid or expired".to_string()))
}

async fn registration_view_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> ApiResult<RegistrationRequestView> {
    Ok(sqlx::query_as::<_, RegistrationRequestView>(&format!(
        "SELECT {REGISTRATION_COLUMNS} FROM registration_requests WHERE id = $1"
    ))
    .bind(id)
    .fetch_one(&mut **transaction)
    .await?)
}

async fn organization_name_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
) -> ApiResult<String> {
    sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
        .bind(organization_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(ApiError::NotFound)
}

async fn enqueue_activation_email(
    state: &AppState,
    public_base_url: &str,
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    request_id: Uuid,
    email: &str,
    recipient_name: &str,
    inviter_name: &str,
    raw_token: &str,
    template_key: TemplateKey,
) -> ApiResult<()> {
    let activation_url = email_token_url(public_base_url, "activate", raw_token);
    let site_name = organization_name_in_transaction(transaction, organization_id).await?;
    let template = email_templates::render_in_transaction(
        transaction,
        organization_id,
        template_key,
        &BTreeMap::from([
            ("recipient_name", recipient_name.to_string()),
            ("activation_url", activation_url),
            ("expires_in_hours", ACTIVATION_TTL_HOURS.to_string()),
            ("inviter_name", inviter_name.to_string()),
            ("site_name", site_name),
        ]),
    )
    .await?;
    mail::enqueue_html_email_in_transaction(
        transaction,
        state.crypto.as_ref(),
        organization_id,
        email,
        &template.subject,
        &template.html_body,
        &format!(
            "registration.activate:{request_id}:{}",
            &hash_token(raw_token, &state.config.session_secret)[..12]
        ),
    )
    .await
}

async fn validate_departments(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    department_ids: Vec<Uuid>,
    primary_department_id: Uuid,
) -> ApiResult<Vec<Uuid>> {
    let mut seen = HashSet::new();
    let department_ids = department_ids
        .into_iter()
        .filter(|id| seen.insert(*id))
        .collect::<Vec<_>>();
    if department_ids.is_empty() || !department_ids.contains(&primary_department_id) {
        return Err(ApiError::BadRequest(
            "at least one department and a matching primary department are required".to_string(),
        ));
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM departments WHERE organization_id = $1 AND id = ANY($2)",
    )
    .bind(organization_id)
    .bind(&department_ids)
    .fetch_one(&mut **transaction)
    .await?;
    if count != department_ids.len() as i64 {
        return Err(ApiError::BadRequest(
            "one or more departments are invalid".to_string(),
        ));
    }
    Ok(department_ids)
}

async fn enforce_registration_rate_limit(
    state: &AppState,
    headers: &HeaderMap,
    email: &str,
) -> ApiResult<()> {
    let client_ip =
        crate::proxy_identity::client_ip(headers, state.config.trusted_proxy_token.as_deref())
            .unwrap_or_else(|| "unknown".to_string());
    let email_key = hash_token(email, &state.config.session_secret);
    let mut redis = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|_| ApiError::Unavailable("registration rate limiter"))?;
    check_rate_limit(
        &mut redis,
        &format!("rate:registration:ip:{client_ip}"),
        REGISTRATION_IP_LIMIT,
        15 * 60,
    )
    .await?;
    check_rate_limit(
        &mut redis,
        &format!("rate:registration:email:{email_key}"),
        REGISTRATION_EMAIL_LIMIT,
        60 * 60,
    )
    .await
}

async fn enforce_verification_rate_limit(
    state: &AppState,
    headers: &HeaderMap,
    email: &str,
) -> ApiResult<()> {
    let client_ip =
        crate::proxy_identity::client_ip(headers, state.config.trusted_proxy_token.as_deref())
            .unwrap_or_else(|| "unknown".to_string());
    let email_key = hash_token(email, &state.config.session_secret);
    let mut redis = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|_| ApiError::Unavailable("registration rate limiter"))?;
    check_rate_limit(
        &mut redis,
        &format!("rate:verification:ip:{client_ip}"),
        30,
        15 * 60,
    )
    .await?;
    check_rate_limit(
        &mut redis,
        &format!("rate:verification:email:{email_key}"),
        10,
        15 * 60,
    )
    .await
}

async fn enforce_resend_rate_limit(
    state: &AppState,
    headers: &HeaderMap,
    email: &str,
) -> ApiResult<()> {
    let client_ip =
        crate::proxy_identity::client_ip(headers, state.config.trusted_proxy_token.as_deref())
            .unwrap_or_else(|| "unknown".to_string());
    let email_key = hash_token(email, &state.config.session_secret);
    let mut redis = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|_| ApiError::Unavailable("registration rate limiter"))?;
    check_rate_limit(
        &mut redis,
        &format!("rate:verification-resend:ip:{client_ip}"),
        20,
        60 * 60,
    )
    .await?;
    check_rate_limit(
        &mut redis,
        &format!("rate:verification-resend:email:{email_key}"),
        6,
        60 * 60,
    )
    .await
}

async fn check_rate_limit(
    redis: &mut redis::aio::MultiplexedConnection,
    key: &str,
    limit: i64,
    window_seconds: i64,
) -> ApiResult<()> {
    let count: i64 = Script::new(
        r#"
        local count = redis.call('INCR', KEYS[1])
        if count == 1 then redis.call('EXPIRE', KEYS[1], ARGV[1]) end
        return count
        "#,
    )
    .key(key)
    .arg(window_seconds)
    .invoke_async(redis)
    .await
    .map_err(|_| ApiError::Unavailable("registration rate limiter"))?;
    if count > limit {
        Err(ApiError::RateLimited)
    } else {
        Ok(())
    }
}

fn require_system_admin(current: &CurrentUser) -> ApiResult<()> {
    if current.system_role != SystemRole::SystemAdmin {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

fn email_domain(email: &str) -> &str {
    email
        .rsplit_once('@')
        .map(|(_, domain)| domain)
        .unwrap_or("")
}

fn email_domain_is_allowed(
    email: &str,
    allow_all_email_domains: bool,
    allowed_domains: &[String],
) -> bool {
    allow_all_email_domains
        || allowed_domains
            .iter()
            .any(|domain| domain.trim().eq_ignore_ascii_case(email_domain(email)))
}

fn required_text(value: &str, field: &str, max: usize) -> ApiResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ApiError::BadRequest(format!("{field} is required")));
    }
    if value.chars().count() > max {
        return Err(ApiError::BadRequest(format!(
            "{field} must be at most {max} characters"
        )));
    }
    Ok(value.to_string())
}

fn optional_text(value: Option<String>, field: &str, max: usize) -> ApiResult<Option<String>> {
    value
        .map(|value| required_text(&value, field, max))
        .transpose()
}

fn validate_token(value: &str) -> ApiResult<&str> {
    let value = value.trim();
    if value.len() < 32 || value.len() > 200 || !value.is_ascii() {
        return Err(ApiError::BadRequest("token is invalid".to_string()));
    }
    Ok(value)
}

fn validate_verification_code(value: &str) -> ApiResult<&str> {
    let value = value.trim();
    if value.len() != 6 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ApiError::BadRequest(
            "verification code must contain six digits".to_string(),
        ));
    }
    Ok(value)
}

fn random_verification_code() -> String {
    format!("{:06}", rand::thread_rng().gen_range(0..1_000_000_u32))
}

fn verification_code_hash(email: &str, code: &str, secret: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key");
    mac.update(b"crewdock:registration-email-code\0");
    mac.update(email.as_bytes());
    mac.update(b"\0");
    mac.update(code.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn verify_verification_code_hash(email: &str, code: &str, secret: &str, stored_hash: &str) -> bool {
    let Ok(expected) = URL_SAFE_NO_PAD.decode(stored_hash) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(b"crewdock:registration-email-code\0");
    mac.update(email.as_bytes());
    mac.update(b"\0");
    mac.update(code.as_bytes());
    mac.verify_slice(&expected).is_ok()
}

fn random_token() -> String {
    format!("{}.{}", Uuid::new_v4(), Uuid::new_v4())
}

fn email_token_url(base_url: &str, path: &str, token: &str) -> String {
    format!("{base_url}/{path}#token={token}")
}

#[cfg(test)]
mod tests {
    use super::{
        REFRESH_ACTIVE_INVITATION_SQL, REFRESH_EXPIRED_VERIFICATION_SQL, email_domain_is_allowed,
        email_token_url, optional_text, required_text, validate_token, validate_verification_code,
        verification_code_hash, verify_verification_code_hash,
    };

    #[test]
    fn allowed_domains_are_case_insensitive() {
        assert!(email_domain_is_allowed(
            "employee@example.com",
            false,
            &["EXAMPLE.COM".to_string()]
        ));
        assert!(!email_domain_is_allowed(
            "employee@outside.test",
            false,
            &["example.com".to_string()]
        ));
        assert!(!email_domain_is_allowed(
            "employee@outside.test",
            false,
            &[]
        ));
        assert!(email_domain_is_allowed("employee@outside.test", true, &[]));
    }

    #[test]
    fn onboarding_text_is_rejected_instead_of_truncated() {
        assert!(required_text("", "display_name", 10).is_err());
        assert!(required_text("12345678901", "display_name", 10).is_err());
        assert_eq!(
            required_text(" Employee ", "display_name", 10).unwrap(),
            "Employee"
        );
        assert_eq!(
            optional_text(Some("   ".to_string()), "reason", 10).is_err(),
            true
        );
    }

    #[test]
    fn activation_tokens_have_bounded_ascii_shape() {
        assert!(validate_token("short").is_err());
        assert!(validate_token(&"x".repeat(201)).is_err());
        assert!(validate_token(&"a".repeat(72)).is_ok());
    }

    #[test]
    fn verification_codes_are_six_digits_and_hmac_bound_to_email() {
        let secret = "a sufficiently long test-only session secret";
        let hash = verification_code_hash("employee@example.com", "012345", secret);
        assert!(verify_verification_code_hash(
            "employee@example.com",
            "012345",
            secret,
            &hash
        ));
        assert!(!verify_verification_code_hash(
            "other@example.com",
            "012345",
            secret,
            &hash
        ));
        assert!(!verify_verification_code_hash(
            "employee@example.com",
            "012346",
            secret,
            &hash
        ));
        assert!(validate_verification_code("012345").is_ok());
        assert!(validate_verification_code("12345").is_err());
        assert!(validate_verification_code("12345a").is_err());
    }

    #[test]
    fn expired_tokens_are_refreshable_without_adding_a_hidden_status() {
        assert!(REFRESH_EXPIRED_VERIFICATION_SQL.contains("status = 'pending_verification'"));
        assert!(REFRESH_EXPIRED_VERIFICATION_SQL.contains("verification_expires_at <= now()"));
        assert!(REFRESH_ACTIVE_INVITATION_SQL.contains("status = 'approved'"));
        assert!(REFRESH_ACTIVE_INVITATION_SQL.contains("activation_token_hash = $4"));
    }

    #[test]
    fn email_tokens_use_fragments_instead_of_server_visible_queries() {
        let url = email_token_url("https://work.example.com", "activate", "secret-token");
        assert_eq!(url, "https://work.example.com/activate#token=secret-token");
        assert!(!url.contains("?token="));
    }
}
