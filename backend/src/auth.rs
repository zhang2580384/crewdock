use crate::{
    app::AppState,
    audit::log_event,
    error::{ApiError, ApiResult},
    models::{SystemRole, User, UserRole},
    response::{ApiEnvelope, ok},
};
use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Json,
    extract::{FromRef, FromRequestParts, Path, State},
    http::{HeaderMap, request::Parts},
};
use chrono::{DateTime, Duration, Utc};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

static LOGIN_PASSWORD_WORK: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub session_id: Uuid,
    pub id: Uuid,
    pub organization_id: Uuid,
    pub department_id: Option<Uuid>,
    pub email: String,
    pub display_name: String,
    pub role: UserRole,
    pub system_role: SystemRole,
}

impl<S> FromRequestParts<S> for CurrentUser
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);
        let token = bearer_token(&parts.headers).ok_or(ApiError::Unauthorized)?;
        let proxy_token = state.config.trusted_proxy_token.as_deref();
        let verified_ip = crate::proxy_identity::verified_ip(&parts.headers, proxy_token);
        let client_ip = crate::proxy_identity::client_ip(&parts.headers, proxy_token);
        let legacy_ip = verified_ip.and_then(|_| client_ip_from_headers(&parts.headers));
        user_from_token(&state, token, client_ip.as_deref(), legacy_ip.as_deref()).await
    }
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub user: User,
}

#[derive(Debug, FromRow)]
struct UserWithPassword {
    id: Uuid,
    organization_id: Uuid,
    department_id: Option<Uuid>,
    email: String,
    display_name: String,
    password_hash: String,
    role: UserRole,
    system_role: SystemRole,
    is_active: bool,
    totp_enabled: bool,
    created_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct CurrentUserRow {
    id: Uuid,
    organization_id: Uuid,
    department_id: Option<Uuid>,
    email: String,
    display_name: String,
    role: UserRole,
    system_role: SystemRole,
}

#[derive(Debug, FromRow)]
struct SessionUserRow {
    session_id: Uuid,
    last_seen_at: DateTime<Utc>,
    id: Uuid,
    organization_id: Uuid,
    department_id: Option<Uuid>,
    email: String,
    display_name: String,
    role: UserRole,
    system_role: SystemRole,
}

#[derive(Debug, PartialEq, Eq)]
enum GrantTargetLockPolicy {
    AlreadyLockedActor,
    TryShareNowait,
}

const ACCOUNT_TARGET_FOR_UPDATE_SQL: &str = r#"
    SELECT id, organization_id, department_id, email, display_name, role, system_role,
           is_active, totp_enabled, created_at
    FROM users
    WHERE id = $1 AND organization_id = $2
    FOR UPDATE NOWAIT
"#;
const EMAIL_IDENTITY_LOCK_SQL: &str = r#"
    SELECT pg_advisory_xact_lock(
      hashtextextended($1::text || ':normalized-email:' || lower($2), 0)
    )
"#;

fn grant_target_lock_policy(actor_id: Uuid, target_id: Uuid) -> GrantTargetLockPolicy {
    if actor_id == target_id {
        GrantTargetLockPolicy::AlreadyLockedActor
    } else {
        GrantTargetLockPolicy::TryShareNowait
    }
}

pub(crate) async fn lock_current_user_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    current: &CurrentUser,
) -> ApiResult<CurrentUser> {
    let row = sqlx::query_as::<_, CurrentUserRow>(
        r#"
        SELECT id, organization_id, department_id, email, display_name, role, system_role
        FROM users
        WHERE id = $1 AND organization_id = $2 AND is_active = TRUE
        FOR SHARE
        "#,
    )
    .bind(current.id)
    .bind(current.organization_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(ApiError::Unauthorized)?;

    Ok(CurrentUser {
        session_id: current.session_id,
        id: row.id,
        organization_id: row.organization_id,
        department_id: row.department_id,
        email: row.email,
        display_name: row.display_name,
        role: row.role,
        system_role: row.system_role,
    })
}

pub(crate) async fn lock_project_grant_target_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    current: &CurrentUser,
    user_id: Uuid,
    require_active: bool,
) -> ApiResult<User> {
    if grant_target_lock_policy(current.id, user_id) == GrantTargetLockPolicy::AlreadyLockedActor {
        return sqlx::query_as::<_, User>(
            r#"
            SELECT id, organization_id, department_id, email, display_name, role, system_role,
                   is_active, totp_enabled, created_at
            FROM users
            WHERE id = $1
              AND organization_id = $2
              AND (NOT $3 OR is_active = TRUE)
            "#,
        )
        .bind(user_id)
        .bind(current.organization_id)
        .bind(require_active)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(ApiError::NotFound);
    }
    sqlx::query_as::<_, User>(
        r#"
        SELECT id, organization_id, department_id, email, display_name, role, system_role,
               is_active, totp_enabled, created_at
        FROM users
        WHERE id = $1
          AND organization_id = $2
          AND (NOT $3 OR is_active = TRUE)
        FOR SHARE NOWAIT
        "#,
    )
    .bind(user_id)
    .bind(current.organization_id)
    .bind(require_active)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_user_lock_error)?
    .ok_or(ApiError::NotFound)
}

pub(crate) async fn lock_account_target_for_update_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    current: &CurrentUser,
    user_id: Uuid,
) -> ApiResult<User> {
    sqlx::query_as::<_, User>(ACCOUNT_TARGET_FOR_UPDATE_SQL)
        .bind(user_id)
        .bind(current.organization_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_user_lock_error)?
        .ok_or(ApiError::NotFound)
}

pub(crate) fn map_user_lock_error(error: sqlx::Error) -> ApiError {
    if error
        .as_database_error()
        .and_then(|database_error| database_error.code())
        .is_some_and(|code| code == "55P03")
    {
        ApiError::Busy("user")
    } else {
        ApiError::Sqlx(error)
    }
}

#[derive(Serialize)]
pub struct MeResponse {
    pub user: CurrentUserView,
}

#[derive(Serialize)]
pub struct CurrentUserView {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub department_id: Option<Uuid>,
    pub department_ids: Vec<Uuid>,
    pub email: String,
    pub display_name: String,
    pub role: UserRole,
    pub system_role: SystemRole,
    pub can_manage_accounts: bool,
    pub can_manage_departments: bool,
    pub can_view_audit: bool,
}

pub(crate) fn map_email_identity_error(error: sqlx::Error) -> ApiError {
    if error
        .as_database_error()
        .and_then(|database_error| database_error.code())
        .is_some_and(|code| code == "23505")
    {
        ApiError::Conflict("email already exists".to_string())
    } else {
        ApiError::Sqlx(error)
    }
}

pub(crate) async fn lock_email_identity_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    normalized_email: &str,
) -> ApiResult<()> {
    sqlx::query(EMAIL_IDENTITY_LOCK_SQL)
        .bind(organization_id)
        .bind(normalized_email)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

#[derive(Serialize)]
pub struct ChangePasswordResponse {
    pub changed: bool,
}

#[derive(Serialize)]
pub struct LogoutResponse {
    pub logged_out: bool,
}

#[derive(Debug, Serialize, FromRow)]
pub struct SessionView {
    pub id: Uuid,
    pub client_ip: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub current: bool,
}

#[derive(Serialize)]
pub struct RevokeSessionResponse {
    pub revoked: bool,
}

pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<LoginRequest>,
) -> ApiResult<Json<ApiEnvelope<LoginResponse>>> {
    let email = payload.email.trim().to_ascii_lowercase();
    let client_ip =
        crate::proxy_identity::client_ip(&headers, state.config.trusted_proxy_token.as_deref());
    let attempt = crate::login_rate_limit::reserve(&state, &email, client_ip.as_deref()).await?;
    if email.len() > 254 || payload.password.len() > 1024 {
        return Err(ApiError::Unauthorized);
    }
    let user_with_hash = sqlx::query_as::<_, UserWithPassword>(
        r#"
        SELECT id, organization_id, department_id, email, display_name, password_hash,
               role, system_role, is_active, totp_enabled, created_at
        FROM users
        WHERE lower(email) = lower($1) AND is_active = TRUE
        "#,
    )
    .bind(&email)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::Unauthorized)?;

    let password_hash = user_with_hash.password_hash.clone();
    let password_permit = LOGIN_PASSWORD_WORK
        .try_acquire()
        .map_err(|_| ApiError::Unavailable("login verifier"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = password_permit;
        verify_password(&payload.password, &password_hash)
    })
    .await
    .map_err(|_| ApiError::Unavailable("login verifier"))??;

    // Recheck under lock after password verification so concurrent resets,
    // disabling and role changes cannot create a stale session.
    let mut transaction = state.db.begin().await?;
    let user_with_hash = sqlx::query_as::<_, UserWithPassword>(
        r#"
        SELECT id, organization_id, department_id, email, display_name, password_hash,
               role, system_role, is_active, totp_enabled, created_at
        FROM users
        WHERE id = $1 AND password_hash = $2 AND is_active = TRUE
        FOR UPDATE
        "#,
    )
    .bind(user_with_hash.id)
    .bind(&user_with_hash.password_hash)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    ensure_two_factor_is_disabled(user_with_hash.totp_enabled)?;

    let raw_token = Uuid::new_v4().to_string() + "." + &Uuid::new_v4().to_string();
    let token_hash = hash_token(&raw_token, &state.config.session_secret);
    let expires_at = Utc::now() + Duration::days(state.config.session_ttl_days);
    sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND expires_at <= now()")
        .bind(user_with_hash.id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "INSERT INTO sessions (user_id, token_hash, expires_at, client_ip) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_with_hash.id)
    .bind(&token_hash)
    .bind(expires_at)
    .bind(&client_ip)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    attempt.succeeded().await;

    match state.redis.get_multiplexed_async_connection().await {
        Ok(mut redis) => {
            let redis_key = format!("session:{token_hash}");
            let result: redis::RedisResult<()> = redis
                .set_ex(
                    redis_key,
                    user_with_hash.id.to_string(),
                    session_ttl_seconds(&state),
                )
                .await;
            if let Err(error) = result {
                tracing::warn!(%error, "login succeeded but Redis session cache update failed");
            }
        }
        Err(error) => {
            tracing::warn!(%error, "login succeeded but Redis session cache was unavailable");
        }
    }

    log_event(
        &state,
        user_with_hash.organization_id,
        Some(user_with_hash.id),
        "auth.login",
        "user",
        Some(user_with_hash.id),
        json!({ "email": user_with_hash.email }),
    )
    .await?;

    Ok(ok(LoginResponse {
        token: raw_token,
        expires_at,
        user: User {
            id: user_with_hash.id,
            organization_id: user_with_hash.organization_id,
            department_id: user_with_hash.department_id,
            email: user_with_hash.email,
            display_name: user_with_hash.display_name,
            role: user_with_hash.role,
            system_role: user_with_hash.system_role,
            is_active: user_with_hash.is_active,
            totp_enabled: user_with_hash.totp_enabled,
            created_at: user_with_hash.created_at,
        },
    }))
}

fn management_capabilities(system_role: &SystemRole, has_department_scope: bool) -> (bool, bool) {
    let can_manage = *system_role == SystemRole::SystemAdmin || has_department_scope;
    (can_manage, can_manage)
}

pub async fn me(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<MeResponse>>> {
    let has_department_scope = if current.system_role == SystemRole::SystemAdmin {
        false
    } else {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM department_admins WHERE user_id = $1)",
        )
        .bind(current.id)
        .fetch_one(&state.db)
        .await?
    };
    let (can_manage_accounts, can_manage_departments) =
        management_capabilities(&current.system_role, has_department_scope);
    let can_view_audit = current.system_role == SystemRole::SystemAdmin;
    let department_ids = sqlx::query_scalar(
        r#"
        SELECT department_id
        FROM user_department_memberships
        WHERE user_id = $1 AND organization_id = $2
        ORDER BY is_primary DESC, created_at, department_id
        "#,
    )
    .bind(current.id)
    .bind(current.organization_id)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(MeResponse {
        user: CurrentUserView {
            id: current.id,
            organization_id: current.organization_id,
            department_id: current.department_id,
            department_ids,
            email: current.email,
            display_name: current.display_name,
            role: current.role,
            system_role: current.system_role,
            can_manage_accounts,
            can_manage_departments,
            can_view_audit,
        },
    }))
}

pub async fn change_password(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(payload): Json<ChangePasswordRequest>,
) -> ApiResult<Json<ApiEnvelope<ChangePasswordResponse>>> {
    let new_password = validate_password(&payload.new_password)?;
    let password_hash = hash_password(&new_password)?;
    let mut transaction = state.db.begin().await?;
    let current_hash: String =
        sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1 FOR UPDATE")
            .bind(current.id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(ApiError::Unauthorized)?;

    if verify_password(&payload.current_password, &current_hash).is_err() {
        return Err(ApiError::BadRequest(
            "current password is incorrect".to_string(),
        ));
    }

    sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
        .bind(password_hash)
        .bind(current.id)
        .execute(&mut *transaction)
        .await?;
    let token_hashes = sqlx::query_scalar::<_, String>(
        "DELETE FROM sessions WHERE user_id = $1 RETURNING token_hash",
    )
    .bind(current.id)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;

    clear_session_cache(&state, &token_hashes).await;
    if let Err(error) = log_event(
        &state,
        current.organization_id,
        Some(current.id),
        "auth.change_password",
        "user",
        Some(current.id),
        json!({ "sessions_revoked": token_hashes.len() }),
    )
    .await
    {
        tracing::warn!(%error, "password changed but audit logging failed");
    }

    Ok(ok(ChangePasswordResponse { changed: true }))
}

pub async fn logout(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<LogoutResponse>>> {
    let token_hash: Option<String> = sqlx::query_scalar(
        "DELETE FROM sessions WHERE id = $1 AND user_id = $2 RETURNING token_hash",
    )
    .bind(current.session_id)
    .bind(current.id)
    .fetch_optional(&state.db)
    .await?;
    if let Some(token_hash) = token_hash {
        clear_session_cache(&state, &[token_hash]).await;
    }
    if let Err(error) = log_event(
        &state,
        current.organization_id,
        Some(current.id),
        "auth.logout",
        "session",
        Some(current.session_id),
        json!({}),
    )
    .await
    {
        tracing::warn!(%error, "session revoked but logout audit logging failed");
    }
    Ok(ok(LogoutResponse { logged_out: true }))
}

pub async fn list_sessions(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<Vec<SessionView>>>> {
    let sessions = sqlx::query_as::<_, SessionView>(
        r#"
        SELECT id, client_ip, created_at, last_seen_at, expires_at, id = $2 AS "current"
        FROM sessions
        WHERE user_id = $1 AND expires_at > now()
        ORDER BY "current" DESC, last_seen_at DESC, created_at DESC
        "#,
    )
    .bind(current.id)
    .bind(current.session_id)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(sessions))
}

pub async fn revoke_session(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(session_id): Path<Uuid>,
) -> ApiResult<Json<ApiEnvelope<RevokeSessionResponse>>> {
    let token_hash: String = sqlx::query_scalar(
        "DELETE FROM sessions WHERE id = $1 AND user_id = $2 RETURNING token_hash",
    )
    .bind(session_id)
    .bind(current.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    clear_session_cache(&state, &[token_hash]).await;
    if let Err(error) = log_event(
        &state,
        current.organization_id,
        Some(current.id),
        "auth.session_revoke",
        "session",
        Some(session_id),
        json!({ "current": session_id == current.session_id }),
    )
    .await
    {
        tracing::warn!(%error, "session revoked but audit logging failed");
    }
    Ok(ok(RevokeSessionResponse { revoked: true }))
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .to_string())
}

pub fn validate_password(value: &str) -> ApiResult<String> {
    let length = value.chars().count();
    if !(12..=128).contains(&length) {
        return Err(ApiError::BadRequest(
            "password must be between 12 and 128 characters".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn ensure_two_factor_is_disabled(enabled: bool) -> ApiResult<()> {
    if enabled {
        return Err(ApiError::BadRequest(
            "two-factor authentication is not available; contact an administrator".to_string(),
        ));
    }
    Ok(())
}

fn verify_password(password: &str, hash: &str) -> ApiResult<()> {
    let parsed = PasswordHash::new(hash).map_err(|_| ApiError::Unauthorized)?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| ApiError::Unauthorized)
}

pub async fn clear_session_cache(state: &AppState, token_hashes: &[String]) {
    if token_hashes.is_empty() {
        return;
    }

    let keys = token_hashes
        .iter()
        .map(|token_hash| format!("session:{token_hash}"))
        .collect::<Vec<_>>();
    let mut redis = match state.redis.get_multiplexed_async_connection().await {
        Ok(redis) => redis,
        Err(error) => {
            tracing::warn!(%error, "password changed but Redis session cache cleanup failed");
            return;
        }
    };
    let result: redis::RedisResult<usize> = redis.del(keys).await;
    if let Err(error) = result {
        tracing::warn!(%error, "password changed but Redis session cache cleanup failed");
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("Authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

pub(crate) fn client_ip_from_headers(headers: &HeaderMap) -> Option<String> {
    crate::proxy_identity::direct_ip(headers)
}

pub fn hash_token(token: &str, secret: &str) -> String {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hasher.update(b":");
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    URL_SAFE_NO_PAD.encode(digest)
}

pub fn normalize_email(value: &str) -> ApiResult<String> {
    let email = value.trim().to_ascii_lowercase();
    if email.is_empty() || email.len() > 254 || !email.is_ascii() {
        return Err(ApiError::BadRequest("email is invalid".to_string()));
    }
    let (local, domain) = email
        .split_once('@')
        .ok_or_else(|| ApiError::BadRequest("email is invalid".to_string()))?;
    if local.is_empty()
        || local.len() > 64
        || domain.is_empty()
        || domain.len() > 253
        || domain.contains('@')
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'/'
                        | b'='
                        | b'?'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'{'
                        | b'|'
                        | b'}'
                        | b'~'
                )
        })
    {
        return Err(ApiError::BadRequest("email is invalid".to_string()));
    }
    if domain.split('.').any(|label| {
        label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) {
        return Err(ApiError::BadRequest("email is invalid".to_string()));
    }
    Ok(email)
}

async fn user_from_token(
    state: &AppState,
    token: &str,
    client_ip: Option<&str>,
    legacy_proxy_ip: Option<&str>,
) -> ApiResult<CurrentUser> {
    let token_hash = hash_token(token, &state.config.session_secret);
    // Only a verified edge request may narrow a legacy session's binding from
    // its actual previous proxy IP to the real client. Never remove IP binding.
    if let (Some(client), Some(proxy)) = (client_ip, legacy_proxy_ip) {
        if client != proxy {
            sqlx::query(
                "UPDATE sessions SET client_ip = $2 WHERE token_hash = $1 AND client_ip = $3 AND expires_at > now()",
            )
            .bind(&token_hash).bind(client).bind(proxy).execute(&state.db).await?;
        }
    }
    let row = sqlx::query_as::<_, SessionUserRow>(
        r#"
        SELECT s.id AS session_id, s.last_seen_at,
               u.id, u.organization_id, u.department_id, u.email, u.display_name,
               u.role, u.system_role
        FROM sessions s
        JOIN users u ON u.id = s.user_id
        WHERE s.token_hash = $1
          AND s.expires_at > now()
          AND u.is_active = TRUE
          AND (s.client_ip IS NULL OR s.client_ip = $2)
        "#,
    )
    .bind(&token_hash)
    .bind(client_ip)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::Unauthorized)?;

    refresh_session_if_needed(state, &row, &token_hash).await?;

    Ok(CurrentUser {
        session_id: row.session_id,
        id: row.id,
        organization_id: row.organization_id,
        department_id: row.department_id,
        email: row.email,
        display_name: row.display_name,
        role: row.role,
        system_role: row.system_role,
    })
}

async fn refresh_session_if_needed(
    state: &AppState,
    row: &SessionUserRow,
    token_hash: &str,
) -> ApiResult<()> {
    let now = Utc::now();
    let refresh_before = now - Duration::hours(state.config.session_refresh_hours);
    if !session_should_refresh(row.last_seen_at, now, state.config.session_refresh_hours) {
        return Ok(());
    }

    let expires_at = now + Duration::days(state.config.session_ttl_days);
    let result = sqlx::query(
        r#"
        UPDATE sessions
        SET last_seen_at = $2, expires_at = $3
        WHERE id = $1 AND last_seen_at <= $4
        "#,
    )
    .bind(row.session_id)
    .bind(now)
    .bind(expires_at)
    .bind(refresh_before)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Ok(());
    }

    match state.redis.get_multiplexed_async_connection().await {
        Ok(mut redis) => {
            let redis_key = format!("session:{token_hash}");
            let result: redis::RedisResult<()> = redis
                .set_ex(redis_key, row.id.to_string(), session_ttl_seconds(state))
                .await;
            if let Err(error) = result {
                tracing::warn!(%error, "session renewed but Redis cache update failed");
            }
        }
        Err(error) => {
            tracing::warn!(%error, "session renewed but Redis cache was unavailable");
        }
    }
    Ok(())
}

fn session_ttl_seconds(state: &AppState) -> u64 {
    state.config.session_ttl_days as u64 * 24 * 60 * 60
}

fn session_should_refresh(
    last_seen_at: DateTime<Utc>,
    now: DateTime<Utc>,
    refresh_hours: i64,
) -> bool {
    last_seen_at <= now - Duration::hours(refresh_hours)
}

#[cfg(test)]
mod tests {
    use super::{
        ACCOUNT_TARGET_FOR_UPDATE_SQL, EMAIL_IDENTITY_LOCK_SQL, GrantTargetLockPolicy,
        client_ip_from_headers, ensure_two_factor_is_disabled, grant_target_lock_policy,
        hash_password, management_capabilities, normalize_email, session_should_refresh,
        validate_password, verify_password,
    };
    use axum::http::{HeaderMap, HeaderValue};
    use chrono::{Duration, TimeZone, Utc};
    use uuid::Uuid;

    #[test]
    fn grant_target_lock_reuses_the_actor_row_for_self_grants() {
        let actor_id = Uuid::new_v4();

        assert_eq!(
            grant_target_lock_policy(actor_id, actor_id),
            GrantTargetLockPolicy::AlreadyLockedActor
        );
        assert_eq!(
            grant_target_lock_policy(actor_id, Uuid::new_v4()),
            GrantTargetLockPolicy::TryShareNowait
        );
    }

    #[test]
    fn account_mutation_target_lock_is_nowait() {
        assert!(ACCOUNT_TARGET_FOR_UPDATE_SQL.contains("FOR UPDATE NOWAIT"));
        assert!(ACCOUNT_TARGET_FOR_UPDATE_SQL.contains("organization_id = $2"));
    }

    #[test]
    fn management_capabilities_use_system_role_or_scoped_assignment() {
        assert_eq!(
            management_capabilities(&crate::models::SystemRole::SystemAdmin, false),
            (true, true)
        );
        assert_eq!(
            management_capabilities(&crate::models::SystemRole::Member, true),
            (true, true)
        );
        assert_eq!(
            management_capabilities(&crate::models::SystemRole::Member, false),
            (false, false)
        );
    }

    #[test]
    fn client_ip_uses_only_the_trusted_reverse_proxy_header() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("192.168.50.23"));
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.9, 192.168.50.23"),
        );

        assert_eq!(
            client_ip_from_headers(&headers).as_deref(),
            Some("192.168.50.23")
        );
    }

    #[test]
    fn client_ip_rejects_invalid_or_untrusted_values() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("not-an-ip"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("192.168.50.23"));

        assert_eq!(client_ip_from_headers(&headers), None);
    }

    #[test]
    fn active_sessions_refresh_once_the_window_has_elapsed() {
        let now = Utc.with_ymd_and_hms(2026, 7, 20, 12, 0, 0).unwrap();

        assert!(!session_should_refresh(now - Duration::hours(23), now, 24));
        assert!(session_should_refresh(now - Duration::hours(24), now, 24));
    }

    #[test]
    fn login_rejects_accounts_with_unimplemented_two_factor_enabled() {
        let error = ensure_two_factor_is_disabled(true)
            .expect_err("an enabled but unimplemented second factor must fail closed");

        assert_eq!(
            error.to_string(),
            "bad request: two-factor authentication is not available; contact an administrator"
        );
    }

    #[test]
    fn password_rule_enforces_twelve_to_one_hundred_twenty_eight_characters() {
        let error = validate_password("12345678901").expect_err("short password must be rejected");

        assert_eq!(
            error.to_string(),
            "bad request: password must be between 12 and 128 characters"
        );
        assert!(validate_password(&"x".repeat(129)).is_err());
    }

    #[test]
    fn normalized_email_mutations_use_a_transaction_scoped_lock() {
        assert!(EMAIL_IDENTITY_LOCK_SQL.contains("pg_advisory_xact_lock"));
        assert!(EMAIL_IDENTITY_LOCK_SQL.contains("lower($2)"));
        assert!(EMAIL_IDENTITY_LOCK_SQL.contains("$1::text"));
    }

    #[test]
    fn password_rule_accepts_boundary_lengths() {
        assert_eq!(validate_password("123456789012").unwrap(), "123456789012");
        assert!(validate_password(&"x".repeat(128)).is_ok());
    }

    #[test]
    fn email_normalization_rejects_truncation_and_malformed_domains() {
        assert_eq!(
            normalize_email(" Employee@Example.COM ").unwrap(),
            "employee@example.com"
        );
        assert!(normalize_email(&format!("{}@example.com", "a".repeat(65))).is_err());
        assert!(normalize_email("employee@-example.com").is_err());
        assert!(normalize_email(&format!("employee@{}", "x".repeat(250))).is_err());
    }

    #[test]
    fn password_verification_accepts_matching_password() {
        let hash = hash_password("correct-password").unwrap();

        assert!(verify_password("correct-password", &hash).is_ok());
    }

    #[test]
    fn password_verification_rejects_wrong_password() {
        let hash = hash_password("correct-password").unwrap();

        assert!(verify_password("wrong-password", &hash).is_err());
    }
}
