use crate::{
    app::AppState,
    audit::log_event_in_transaction,
    auth::{self, CurrentUser},
    error::{ApiError, ApiResult},
    mail,
    models::SystemRole,
    response::{ApiEnvelope, ok},
};
use axum::{Json, extract::State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use std::collections::BTreeSet;
use uuid::Uuid;

const TEST_EMAIL_SUBJECT: &str = "crewdock 邮件设置测试";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationMode {
    Closed,
    Application,
}

impl RegistrationMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Application => "application",
        }
    }

    fn from_db(value: &str) -> ApiResult<Self> {
        match value {
            "closed" => Ok(Self::Closed),
            "application" => Ok(Self::Application),
            _ => Err(ApiError::Internal(anyhow::anyhow!(
                "unknown registration mode"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SmtpTlsMode {
    None,
    Starttls,
    Tls,
}

impl SmtpTlsMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Starttls => "starttls",
            Self::Tls => "tls",
        }
    }

    fn from_db(value: &str) -> ApiResult<Self> {
        match value {
            "none" => Ok(Self::None),
            "starttls" => Ok(Self::Starttls),
            "tls" => Ok(Self::Tls),
            _ => Err(ApiError::Internal(anyhow::anyhow!("unknown SMTP TLS mode"))),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateAuthSettingsRequest {
    pub registration_mode: RegistrationMode,
    #[serde(default)]
    pub allow_all_email_domains: bool,
    #[serde(default)]
    pub allowed_email_domains: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AuthSettingsResponse {
    pub registration_mode: RegistrationMode,
    pub allow_all_email_domains: bool,
    pub allowed_email_domains: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct AuthSettingsRow {
    registration_mode: String,
    allow_all_email_domains: bool,
    allowed_email_domains: Vec<String>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateEmailSettingsRequest {
    pub enabled: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub tls_mode: SmtpTlsMode,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    #[serde(default)]
    pub clear_smtp_password: bool,
    pub from_email: String,
    pub from_name: String,
    pub reply_to: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EmailSettingsResponse {
    pub enabled: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub tls_mode: SmtpTlsMode,
    pub smtp_username: Option<String>,
    pub from_email: String,
    pub from_name: String,
    pub reply_to: Option<String>,
    pub has_password: bool,
    pub email_ready: bool,
    pub last_test_at: Option<DateTime<Utc>>,
    pub last_test_succeeded: Option<bool>,
    pub last_test_error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct EmailSettingsRow {
    enabled: bool,
    smtp_host: String,
    smtp_port: i32,
    tls_mode: String,
    smtp_username: Option<String>,
    from_email: String,
    from_name: String,
    reply_to: Option<String>,
    has_password: bool,
    last_test_at: Option<DateTime<Utc>>,
    last_test_succeeded: Option<bool>,
    last_test_error: Option<String>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct TestEmailResponse {
    pub queued: bool,
    pub recipient: String,
}

pub async fn get_auth_settings(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<AuthSettingsResponse>>> {
    ensure_system_admin(&current)?;
    let row = fetch_auth_settings(&state, current.organization_id).await?;
    Ok(ok(auth_settings_response(row)?))
}

pub async fn update_auth_settings(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(payload): Json<UpdateAuthSettingsRequest>,
) -> ApiResult<Json<ApiEnvelope<AuthSettingsResponse>>> {
    ensure_system_admin(&current)?;
    let allowed_email_domains = normalize_allowed_domains(payload.allowed_email_domains)?;
    validate_registration_domain_policy(
        payload.registration_mode,
        payload.allow_all_email_domains,
        &allowed_email_domains,
    )?;
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    ensure_system_admin(&actor)?;
    let row = sqlx::query_as::<_, AuthSettingsRow>(
        r#"
        INSERT INTO organization_auth_settings (
          organization_id, registration_mode, allow_all_email_domains,
          allowed_email_domains, updated_by, updated_at
        )
        VALUES ($1, $2::registration_mode, $3, $4, $5, now())
        ON CONFLICT (organization_id) DO UPDATE
        SET registration_mode = EXCLUDED.registration_mode,
            allow_all_email_domains = EXCLUDED.allow_all_email_domains,
            allowed_email_domains = EXCLUDED.allowed_email_domains,
            updated_by = EXCLUDED.updated_by,
            updated_at = now()
        RETURNING registration_mode::text AS registration_mode,
                  allow_all_email_domains, allowed_email_domains, updated_at
        "#,
    )
    .bind(actor.organization_id)
    .bind(payload.registration_mode.as_str())
    .bind(payload.allow_all_email_domains)
    .bind(&allowed_email_domains)
    .bind(actor.id)
    .fetch_one(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "settings.auth.update",
        "organization_auth_settings",
        Some(actor.organization_id),
        json!({
            "registration_mode": payload.registration_mode,
            "allow_all_email_domains": payload.allow_all_email_domains,
            "allowed_email_domains": allowed_email_domains,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(auth_settings_response(row)?))
}

pub async fn get_email_settings(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<EmailSettingsResponse>>> {
    ensure_system_admin(&current)?;
    let row = fetch_email_settings(&state, current.organization_id).await?;
    Ok(ok(email_settings_response(
        row,
        state.config.master_key != [0u8; 32],
    )?))
}

pub async fn update_email_settings(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(payload): Json<UpdateEmailSettingsRequest>,
) -> ApiResult<Json<ApiEnvelope<EmailSettingsResponse>>> {
    ensure_system_admin(&current)?;
    let smtp_host = clean_smtp_host(&payload.smtp_host)?;
    let smtp_username = optional_text(payload.smtp_username.as_deref(), 320, "smtp_username")?;
    let from_email = optional_email(&payload.from_email, "from_email")?.unwrap_or_default();
    let from_name = bounded_text(&payload.from_name, 240, "from_name")?;
    let reply_to = match payload.reply_to.as_deref() {
        Some(value) => optional_email(value, "reply_to")?,
        None => None,
    };
    if payload.clear_smtp_password && payload.smtp_password.is_some() {
        return Err(ApiError::BadRequest(
            "smtp_password and clear_smtp_password cannot be used together".to_string(),
        ));
    }
    if payload.smtp_port == 0 {
        return Err(ApiError::BadRequest(
            "smtp_port must be between 1 and 65535".to_string(),
        ));
    }
    if payload.enabled
        && payload.tls_mode == SmtpTlsMode::None
        && !mail::plaintext_smtp_host_allowed(&smtp_host)
    {
        return Err(ApiError::BadRequest(
            "unencrypted SMTP is limited to loopback relays".to_string(),
        ));
    }
    ensure_email_master_key(
        &state.config.master_key,
        payload.enabled || payload.smtp_password.is_some(),
    )?;

    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    ensure_system_admin(&actor)?;
    let existing_password = sqlx::query_scalar::<_, Option<Vec<u8>>>(
        r#"
        SELECT smtp_password_encrypted
        FROM organization_email_settings
        WHERE organization_id = $1
        FOR UPDATE
        "#,
    )
    .bind(actor.organization_id)
    .fetch_optional(&mut *transaction)
    .await?
    .flatten();
    let smtp_password_encrypted = if payload.clear_smtp_password {
        None
    } else if let Some(password) = payload.smtp_password.as_deref() {
        if password.is_empty() || password.len() > 1024 {
            return Err(ApiError::BadRequest(
                "smtp_password must be between 1 and 1024 bytes".to_string(),
            ));
        }
        Some(
            state
                .crypto
                .encrypt(password.as_bytes())
                .map_err(ApiError::Internal)?,
        )
    } else {
        existing_password
    };

    if payload.enabled
        && (smtp_host.is_empty() || from_email.is_empty() || smtp_password_encrypted.is_none())
    {
        return Err(ApiError::BadRequest(
            "enabled email delivery requires smtp_host, from_email, and smtp_password".to_string(),
        ));
    }

    let row = sqlx::query_as::<_, EmailSettingsRow>(
        r#"
        INSERT INTO organization_email_settings (
          organization_id, enabled, smtp_host, smtp_port, tls_mode, smtp_username,
          smtp_password_encrypted, from_email, from_name, reply_to,
          last_test_at, last_test_succeeded, last_test_error, updated_by, updated_at
        )
        VALUES ($1, $2, $3, $4, $5::smtp_tls_mode, $6, $7, $8, $9, $10,
                NULL, NULL, NULL, $11, now())
        ON CONFLICT (organization_id) DO UPDATE
        SET enabled = EXCLUDED.enabled,
            smtp_host = EXCLUDED.smtp_host,
            smtp_port = EXCLUDED.smtp_port,
            tls_mode = EXCLUDED.tls_mode,
            smtp_username = EXCLUDED.smtp_username,
            smtp_password_encrypted = EXCLUDED.smtp_password_encrypted,
            from_email = EXCLUDED.from_email,
            from_name = EXCLUDED.from_name,
            reply_to = EXCLUDED.reply_to,
            last_test_at = NULL,
            last_test_succeeded = NULL,
            last_test_error = NULL,
            updated_by = EXCLUDED.updated_by,
            updated_at = now()
        RETURNING enabled, smtp_host, smtp_port, tls_mode::text AS tls_mode,
                  smtp_username, from_email, from_name, reply_to,
                  smtp_password_encrypted IS NOT NULL AS has_password,
                  last_test_at, last_test_succeeded, last_test_error, updated_at
        "#,
    )
    .bind(actor.organization_id)
    .bind(payload.enabled)
    .bind(&smtp_host)
    .bind(i32::from(payload.smtp_port))
    .bind(payload.tls_mode.as_str())
    .bind(&smtp_username)
    .bind(&smtp_password_encrypted)
    .bind(&from_email)
    .bind(&from_name)
    .bind(&reply_to)
    .bind(actor.id)
    .fetch_one(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "settings.email.update",
        "organization_email_settings",
        Some(actor.organization_id),
        json!({
            "enabled": payload.enabled,
            "smtp_host": smtp_host,
            "smtp_port": payload.smtp_port,
            "tls_mode": payload.tls_mode,
            "smtp_username_configured": smtp_username.is_some(),
            "smtp_password_configured": smtp_password_encrypted.is_some(),
            "from_email": from_email,
            "reply_to_configured": reply_to.is_some(),
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(email_settings_response(
        row,
        state.config.master_key != [0u8; 32],
    )?))
}

pub async fn test_email_settings(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<TestEmailResponse>>> {
    ensure_system_admin(&current)?;
    ensure_email_master_key(&state.config.master_key, true)?;
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    ensure_system_admin(&actor)?;
    let ready = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT enabled
           AND trim(smtp_host) <> ''
           AND trim(from_email) <> ''
           AND smtp_password_encrypted IS NOT NULL
        FROM organization_email_settings
        WHERE organization_id = $1
        FOR SHARE
        "#,
    )
    .bind(actor.organization_id)
    .fetch_optional(&mut *transaction)
    .await?
    .unwrap_or(false);
    if !ready {
        return Err(ApiError::BadRequest(
            "email delivery must be fully configured and enabled before testing".to_string(),
        ));
    }

    let idempotency_key = format!("settings-test:{}", Uuid::new_v4());
    mail::enqueue_test_email_in_transaction(
        &mut transaction,
        state.crypto.as_ref(),
        actor.organization_id,
        actor.id,
        &actor.email,
        TEST_EMAIL_SUBJECT,
        &test_email_body(&actor.display_name),
        &idempotency_key,
    )
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "settings.email.test_queued",
        "organization_email_settings",
        Some(actor.organization_id),
        json!({ "recipient_user_id": actor.id }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(TestEmailResponse {
        queued: true,
        recipient: actor.email,
    }))
}

async fn fetch_auth_settings(
    state: &AppState,
    organization_id: Uuid,
) -> ApiResult<AuthSettingsRow> {
    sqlx::query_as::<_, AuthSettingsRow>(
        r#"
        SELECT COALESCE(settings.registration_mode, 'closed'::registration_mode)::text
                 AS registration_mode,
               COALESCE(settings.allow_all_email_domains, false)
                 AS allow_all_email_domains,
               COALESCE(settings.allowed_email_domains, '{}'::text[])
                 AS allowed_email_domains,
               COALESCE(settings.updated_at, organization.created_at) AS updated_at
        FROM organizations organization
        LEFT JOIN organization_auth_settings settings
          ON settings.organization_id = organization.id
        WHERE organization.id = $1
        "#,
    )
    .bind(organization_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)
}

async fn fetch_email_settings(
    state: &AppState,
    organization_id: Uuid,
) -> ApiResult<EmailSettingsRow> {
    sqlx::query_as::<_, EmailSettingsRow>(
        r#"
        SELECT COALESCE(settings.enabled, false) AS enabled,
               COALESCE(settings.smtp_host, '') AS smtp_host,
               COALESCE(settings.smtp_port, 587) AS smtp_port,
               COALESCE(settings.tls_mode, 'starttls'::smtp_tls_mode)::text AS tls_mode,
               settings.smtp_username,
               COALESCE(settings.from_email, '') AS from_email,
               COALESCE(settings.from_name, '') AS from_name,
               settings.reply_to,
               settings.smtp_password_encrypted IS NOT NULL AS has_password,
               settings.last_test_at,
               settings.last_test_succeeded,
               settings.last_test_error,
               COALESCE(settings.updated_at, organization.created_at) AS updated_at
        FROM organizations organization
        LEFT JOIN organization_email_settings settings
          ON settings.organization_id = organization.id
        WHERE organization.id = $1
        "#,
    )
    .bind(organization_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)
}

fn auth_settings_response(row: AuthSettingsRow) -> ApiResult<AuthSettingsResponse> {
    Ok(AuthSettingsResponse {
        registration_mode: RegistrationMode::from_db(&row.registration_mode)?,
        allow_all_email_domains: row.allow_all_email_domains,
        allowed_email_domains: row.allowed_email_domains,
        updated_at: row.updated_at,
    })
}

fn email_settings_response(
    row: EmailSettingsRow,
    master_key_configured: bool,
) -> ApiResult<EmailSettingsResponse> {
    let smtp_port = u16::try_from(row.smtp_port)
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid SMTP port in database")))?;
    let email_ready = email_delivery_ready(
        master_key_configured,
        row.enabled,
        &row.smtp_host,
        &row.from_email,
        row.has_password,
    );
    Ok(EmailSettingsResponse {
        enabled: row.enabled,
        smtp_host: row.smtp_host,
        smtp_port,
        tls_mode: SmtpTlsMode::from_db(&row.tls_mode)?,
        smtp_username: row.smtp_username,
        from_email: row.from_email,
        from_name: row.from_name,
        reply_to: row.reply_to,
        has_password: row.has_password,
        email_ready,
        last_test_at: row.last_test_at,
        last_test_succeeded: row.last_test_succeeded,
        last_test_error: row.last_test_error,
        updated_at: row.updated_at,
    })
}

fn email_delivery_ready(
    master_key_configured: bool,
    enabled: bool,
    smtp_host: &str,
    from_email: &str,
    has_password: bool,
) -> bool {
    master_key_configured
        && enabled
        && !smtp_host.is_empty()
        && !from_email.is_empty()
        && has_password
}

fn ensure_system_admin(current: &CurrentUser) -> ApiResult<()> {
    if current.system_role != SystemRole::SystemAdmin {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

fn ensure_email_master_key(master_key: &[u8; 32], required: bool) -> ApiResult<()> {
    if required && *master_key == [0u8; 32] {
        return Err(ApiError::BadRequest(
            "CREWDOCK_MASTER_KEY must be configured before enabling email delivery or storing an SMTP password"
                .to_string(),
        ));
    }
    Ok(())
}

fn test_email_body(display_name: &str) -> String {
    format!(
        "你好，{display_name}：\n\n这是一封 crewdock 邮件设置测试邮件。收到此邮件表示当前 SMTP 配置可用。\n"
    )
}

fn normalize_allowed_domains(domains: Vec<String>) -> ApiResult<Vec<String>> {
    let mut normalized = BTreeSet::new();
    for value in domains {
        let domain = value.trim().to_ascii_lowercase();
        if !valid_domain(&domain) {
            return Err(ApiError::BadRequest(format!(
                "allowed_email_domains contains an invalid domain: {value}"
            )));
        }
        normalized.insert(domain);
    }
    Ok(normalized.into_iter().collect())
}

fn validate_registration_domain_policy(
    registration_mode: RegistrationMode,
    allow_all_email_domains: bool,
    allowed_email_domains: &[String],
) -> ApiResult<()> {
    if registration_mode == RegistrationMode::Application
        && !allow_all_email_domains
        && allowed_email_domains.is_empty()
    {
        return Err(ApiError::BadRequest(
            "application registration requires at least one allowed email domain".to_string(),
        ));
    }
    Ok(())
}

fn valid_domain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.is_ascii()
        && value.contains('.')
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        })
}

fn clean_smtp_host(value: &str) -> ApiResult<String> {
    let host = value.trim();
    if host.len() > 253
        || host.chars().any(char::is_whitespace)
        || host.contains('/')
        || host.contains("//")
    {
        return Err(ApiError::BadRequest("smtp_host is invalid".to_string()));
    }
    Ok(host.to_string())
}

fn optional_email(value: &str, field: &str) -> ApiResult<Option<String>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 254 || value.parse::<lettre::Address>().is_err() {
        return Err(ApiError::BadRequest(format!("{field} is invalid")));
    }
    Ok(Some(value.to_ascii_lowercase()))
}

fn optional_text(value: Option<&str>, max_len: usize, field: &str) -> ApiResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > max_len {
        return Err(ApiError::BadRequest(format!(
            "{field} must be at most {max_len} bytes"
        )));
    }
    Ok(Some(value.to_string()))
}

fn bounded_text(value: &str, max_len: usize, field: &str) -> ApiResult<String> {
    let value = value.trim();
    if value.len() > max_len {
        return Err(ApiError::BadRequest(format!(
            "{field} must be at most {max_len} bytes"
        )));
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        EmailSettingsResponse, RegistrationMode, SmtpTlsMode, TEST_EMAIL_SUBJECT,
        email_delivery_ready, ensure_email_master_key, normalize_allowed_domains, optional_email,
        test_email_body, valid_domain, validate_registration_domain_policy,
    };
    use chrono::Utc;

    #[test]
    fn allowed_domains_are_normalized_deduplicated_and_sorted() {
        let domains = normalize_allowed_domains(vec![
            "Example.COM".to_string(),
            "mail.example.com".to_string(),
            "example.com".to_string(),
        ])
        .unwrap();

        assert_eq!(domains, vec!["example.com", "mail.example.com"]);
    }

    #[test]
    fn malformed_domains_are_rejected() {
        assert!(!valid_domain("example"));
        assert!(!valid_domain("-mail.example.com"));
        assert!(!valid_domain("mail..example.com"));
    }

    #[test]
    fn application_registration_requires_domains_unless_all_email_is_allowed() {
        assert!(
            validate_registration_domain_policy(RegistrationMode::Application, false, &[]).is_err()
        );
        assert!(
            validate_registration_domain_policy(RegistrationMode::Application, true, &[]).is_ok()
        );
        assert!(validate_registration_domain_policy(RegistrationMode::Closed, false, &[]).is_ok());
        assert!(
            validate_registration_domain_policy(
                RegistrationMode::Application,
                false,
                &["example.com".to_string()],
            )
            .is_ok()
        );
    }

    #[test]
    fn auth_settings_migration_adds_explicit_allow_all_flag() {
        let migration = include_str!("../migrations/202607240015_allow_all_email_domains.sql");
        assert!(migration.contains("allow_all_email_domains boolean NOT NULL DEFAULT false"));
    }

    #[test]
    fn email_fields_accept_empty_only_as_unconfigured() {
        assert_eq!(optional_email("", "from_email").unwrap(), None);
        assert_eq!(
            optional_email("Admin@Example.com", "from_email").unwrap(),
            Some("admin@example.com".to_string())
        );
        assert!(optional_email("not-an-email", "from_email").is_err());
    }

    #[test]
    fn tls_mode_serializes_to_api_contract() {
        assert_eq!(
            serde_json::to_value(SmtpTlsMode::Starttls).unwrap(),
            "starttls"
        );
    }

    #[test]
    fn default_master_key_cannot_store_or_enable_email_credentials() {
        let error = ensure_email_master_key(&[0u8; 32], true).unwrap_err();
        assert!(error.to_string().contains("CREWDOCK_MASTER_KEY"));

        assert!(ensure_email_master_key(&[0u8; 32], false).is_ok());
        assert!(ensure_email_master_key(&[7u8; 32], true).is_ok());
        assert!(!email_delivery_ready(
            false,
            true,
            "smtp.example.com",
            "mailer@example.com",
            true,
        ));
        assert!(email_delivery_ready(
            true,
            true,
            "smtp.example.com",
            "mailer@example.com",
            true,
        ));
    }

    #[test]
    fn test_email_uses_localized_content() {
        assert!(TEST_EMAIL_SUBJECT.contains("邮件设置测试"));
        let body = test_email_body("张三");
        assert!(body.contains("你好，张三"));
        assert!(body.contains("SMTP 配置可用"));
    }

    #[test]
    fn email_settings_response_never_serializes_a_password() {
        let response = EmailSettingsResponse {
            enabled: true,
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: 587,
            tls_mode: SmtpTlsMode::Starttls,
            smtp_username: Some("mailer@example.com".to_string()),
            from_email: "mailer@example.com".to_string(),
            from_name: "crewdock".to_string(),
            reply_to: None,
            has_password: true,
            email_ready: true,
            last_test_at: None,
            last_test_succeeded: None,
            last_test_error: None,
            updated_at: Utc::now(),
        };
        let value = serde_json::to_value(response).unwrap();

        assert_eq!(value["has_password"], true);
        assert!(value.get("smtp_password").is_none());
        assert!(value.get("smtp_password_encrypted").is_none());
    }
}
