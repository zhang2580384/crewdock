use crate::{
    app::AppState,
    crypto::Crypto,
    error::{ApiError, ApiResult},
};
use anyhow::{Context, Result};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use sqlx::{FromRow, Postgres, Transaction};
use std::time::Duration;
use tokio::sync::watch;
use uuid::Uuid;

const BATCH_SIZE: usize = 16;
const SCAN_INTERVAL: Duration = Duration::from_secs(10);
const LEASE_SECONDS: i64 = 90;
const SMTP_COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_ATTEMPTS: i32 = 5;

#[derive(Debug, FromRow)]
struct ClaimedEmail {
    id: Uuid,
    organization_id: Uuid,
    recipient: String,
    subject: String,
    body_encrypted: Vec<u8>,
    body_content_type: String,
    test_requested_by: Option<Uuid>,
    attempts: i32,
    max_attempts: i32,
}

#[derive(Debug, FromRow)]
struct DeliverySettings {
    enabled: bool,
    smtp_host: String,
    smtp_port: i32,
    tls_mode: String,
    smtp_username: Option<String>,
    smtp_password_encrypted: Option<Vec<u8>>,
    from_email: String,
    from_name: String,
    reply_to: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FailureTransition {
    attempts: i32,
    terminal: bool,
    backoff_seconds: i64,
}

pub(crate) async fn enqueue_email_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    crypto: &Crypto,
    organization_id: Uuid,
    recipient: &str,
    subject: &str,
    text_body: &str,
    idempotency_key: &str,
) -> ApiResult<()> {
    enqueue(
        transaction,
        crypto,
        organization_id,
        None,
        recipient,
        subject,
        text_body,
        "text/plain",
        idempotency_key,
    )
    .await
}

pub(crate) async fn enqueue_html_email_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    crypto: &Crypto,
    organization_id: Uuid,
    recipient: &str,
    subject: &str,
    html_body: &str,
    idempotency_key: &str,
) -> ApiResult<()> {
    enqueue(
        transaction,
        crypto,
        organization_id,
        None,
        recipient,
        subject,
        html_body,
        "text/html",
        idempotency_key,
    )
    .await
}

pub(crate) async fn enqueue_test_email_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    crypto: &Crypto,
    organization_id: Uuid,
    requested_by: Uuid,
    recipient: &str,
    subject: &str,
    text_body: &str,
    idempotency_key: &str,
) -> ApiResult<()> {
    enqueue(
        transaction,
        crypto,
        organization_id,
        Some(requested_by),
        recipient,
        subject,
        text_body,
        "text/plain",
        idempotency_key,
    )
    .await
}

async fn enqueue(
    transaction: &mut Transaction<'_, Postgres>,
    crypto: &Crypto,
    organization_id: Uuid,
    test_requested_by: Option<Uuid>,
    recipient: &str,
    subject: &str,
    body: &str,
    body_content_type: &str,
    idempotency_key: &str,
) -> ApiResult<()> {
    validate_outbox_message(recipient, subject, body, body_content_type, idempotency_key)?;
    let body_encrypted = crypto
        .encrypt(body.as_bytes())
        .map_err(ApiError::Internal)?;
    sqlx::query(
        r#"
        INSERT INTO email_outbox (
          organization_id, recipient, subject, body_encrypted, body_content_type,
          idempotency_key, test_requested_by, max_attempts
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (organization_id, idempotency_key) DO NOTHING
        "#,
    )
    .bind(organization_id)
    .bind(recipient.trim())
    .bind(subject.trim())
    .bind(body_encrypted)
    .bind(body_content_type)
    .bind(idempotency_key)
    .bind(test_requested_by)
    .bind(DEFAULT_MAX_ATTEMPTS)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub async fn run_worker(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut interval = tokio::time::interval(SCAN_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            _ = interval.tick() => {
                if let Err(error) = run_due_batch(&state).await {
                    tracing::error!(%error, "email outbox scan failed");
                }
            }
        }
    }
    tracing::info!("email outbox worker stopped");
}

async fn run_due_batch(state: &AppState) -> ApiResult<()> {
    for _ in 0..BATCH_SIZE {
        let lease_owner = Uuid::new_v4();
        let Some(email) = claim_due_email(state, lease_owner).await? else {
            break;
        };
        if let Err(error) = process_claimed_email(state, &email, lease_owner).await {
            tracing::error!(%error, email_id = %email.id, "email outbox transition failed");
        }
    }
    Ok(())
}

async fn claim_due_email(state: &AppState, lease_owner: Uuid) -> ApiResult<Option<ClaimedEmail>> {
    sqlx::query_as::<_, ClaimedEmail>(
        r#"
        WITH candidate AS (
          SELECT id
          FROM email_outbox
          WHERE sent_at IS NULL
            AND failed_at IS NULL
            AND attempts < max_attempts
            AND next_attempt_at <= now()
            AND (lease_until IS NULL OR lease_until <= now())
          ORDER BY next_attempt_at, created_at, id
          FOR UPDATE SKIP LOCKED
          LIMIT 1
        )
        UPDATE email_outbox outbox
        SET lease_owner = $1,
            lease_until = now() + ($2::double precision * interval '1 second'),
            updated_at = now()
        FROM candidate
        WHERE outbox.id = candidate.id
        RETURNING outbox.id, outbox.organization_id, outbox.recipient,
                  outbox.subject, outbox.body_encrypted, outbox.body_content_type,
                  outbox.test_requested_by, outbox.attempts, outbox.max_attempts
        "#,
    )
    .bind(lease_owner)
    .bind(LEASE_SECONDS as f64)
    .fetch_optional(&state.db)
    .await
    .map_err(Into::into)
}

async fn process_claimed_email(
    state: &AppState,
    email: &ClaimedEmail,
    lease_owner: Uuid,
) -> ApiResult<()> {
    let result = tokio::time::timeout(DELIVERY_TIMEOUT, deliver_email(state, email)).await;
    match result {
        Ok(Ok(())) => record_success(state, email, lease_owner).await,
        Ok(Err(error)) => record_failure(state, email, lease_owner, &bounded_error(&error)).await,
        Err(_) => {
            record_failure(
                state,
                email,
                lease_owner,
                "SMTP delivery timed out after 30 seconds",
            )
            .await
        }
    }
}

async fn deliver_email(state: &AppState, email: &ClaimedEmail) -> Result<()> {
    let settings = sqlx::query_as::<_, DeliverySettings>(
        r#"
        SELECT enabled, smtp_host, smtp_port, tls_mode::text AS tls_mode,
               smtp_username, smtp_password_encrypted, from_email, from_name, reply_to
        FROM organization_email_settings
        WHERE organization_id = $1
        "#,
    )
    .bind(email.organization_id)
    .fetch_optional(&state.db)
    .await?
    .context("email settings are not configured")?;
    validate_delivery_settings(&settings)?;

    let password_encrypted = settings
        .smtp_password_encrypted
        .as_deref()
        .context("SMTP password is not configured")?;
    let password = String::from_utf8(
        state
            .crypto
            .decrypt(password_encrypted)
            .context("decrypt SMTP password")?,
    )
    .context("SMTP password is not valid UTF-8")?;

    let from_address = settings
        .from_email
        .parse()
        .context("parse configured sender address")?;
    let from = if settings.from_name.is_empty() {
        Mailbox::new(None, from_address)
    } else {
        Mailbox::new(Some(settings.from_name.clone()), from_address)
    };
    let recipient = email
        .recipient
        .parse()
        .context("parse outbox recipient address")?;
    let content_type = match email.body_content_type.as_str() {
        "text/plain" => ContentType::TEXT_PLAIN,
        "text/html" => ContentType::TEXT_HTML,
        _ => anyhow::bail!("unsupported outbox body content type"),
    };
    let mut message = Message::builder()
        .from(from)
        .to(Mailbox::new(None, recipient))
        .subject(&email.subject)
        .header(content_type);
    if let Some(reply_to) = settings.reply_to.as_deref() {
        message = message.reply_to(
            reply_to
                .parse::<Mailbox>()
                .context("parse configured reply-to address")?,
        );
    }
    let text_body = String::from_utf8(
        state
            .crypto
            .decrypt(&email.body_encrypted)
            .context("decrypt email body")?,
    )
    .context("email body is not valid UTF-8")?;
    let message = message.body(text_body).context("build email message")?;

    let mut transport = match settings.tls_mode.as_str() {
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&settings.smtp_host)
            .context("configure SMTP TLS")?,
        "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&settings.smtp_host)
            .context("configure SMTP STARTTLS")?,
        "none" => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&settings.smtp_host),
        _ => anyhow::bail!("unsupported SMTP TLS mode"),
    }
    .port(u16::try_from(settings.smtp_port).context("invalid configured SMTP port")?)
    .timeout(Some(SMTP_COMMAND_TIMEOUT));
    let username = settings
        .smtp_username
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| settings.from_email.clone());
    transport = transport.credentials(Credentials::new(username, password));
    transport
        .build()
        .send(message)
        .await
        .context("send SMTP message")?;
    Ok(())
}

async fn record_success(
    state: &AppState,
    email: &ClaimedEmail,
    lease_owner: Uuid,
) -> ApiResult<()> {
    let mut transaction = state.db.begin().await?;
    let updated = sqlx::query(
        r#"
        UPDATE email_outbox
        SET attempts = attempts + 1,
            sent_at = now(),
            body_encrypted = NULL,
            last_error = NULL,
            lease_owner = NULL,
            lease_until = NULL,
            updated_at = now()
        WHERE id = $1 AND lease_owner = $2
        "#,
    )
    .bind(email.id)
    .bind(lease_owner)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() > 0 && email.test_requested_by.is_some() {
        update_test_result(&mut transaction, email.organization_id, true, None).await?;
    }
    transaction.commit().await?;
    tracing::info!(email_id = %email.id, "email delivered");
    Ok(())
}

async fn record_failure(
    state: &AppState,
    email: &ClaimedEmail,
    lease_owner: Uuid,
    error: &str,
) -> ApiResult<()> {
    let transition = failure_transition(email.attempts, email.max_attempts);
    let mut transaction = state.db.begin().await?;
    let updated = sqlx::query(
        r#"
        UPDATE email_outbox
        SET attempts = $3,
            next_attempt_at = now() + ($4::double precision * interval '1 second'),
            last_error = $5,
            failed_at = CASE WHEN $6 THEN now() ELSE NULL END,
            body_encrypted = CASE WHEN $6 THEN NULL ELSE body_encrypted END,
            lease_owner = NULL,
            lease_until = NULL,
            updated_at = now()
        WHERE id = $1 AND lease_owner = $2
        "#,
    )
    .bind(email.id)
    .bind(lease_owner)
    .bind(transition.attempts)
    .bind(transition.backoff_seconds as f64)
    .bind(error)
    .bind(transition.terminal)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() > 0 && email.test_requested_by.is_some() {
        update_test_result(&mut transaction, email.organization_id, false, Some(error)).await?;
    }
    transaction.commit().await?;
    tracing::warn!(
        email_id = %email.id,
        attempts = transition.attempts,
        terminal = transition.terminal,
        %error,
        "email delivery failed"
    );
    Ok(())
}

async fn update_test_result(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    succeeded: bool,
    error: Option<&str>,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        UPDATE organization_email_settings
        SET last_test_at = now(),
            last_test_succeeded = $2,
            last_test_error = $3,
            updated_at = now()
        WHERE organization_id = $1
        "#,
    )
    .bind(organization_id)
    .bind(succeeded)
    .bind(error)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn validate_outbox_message(
    recipient: &str,
    subject: &str,
    body: &str,
    body_content_type: &str,
    idempotency_key: &str,
) -> ApiResult<()> {
    let recipient = recipient.trim();
    if recipient.len() > 254 || recipient.parse::<lettre::Address>().is_err() {
        return Err(ApiError::BadRequest(
            "email recipient is invalid".to_string(),
        ));
    }
    let subject = subject.trim();
    if subject.is_empty()
        || subject.len() > 500
        || subject.chars().any(|character| character.is_control())
    {
        return Err(ApiError::BadRequest("email subject is invalid".to_string()));
    }
    if body.is_empty() || body.len() > 1024 * 1024 {
        return Err(ApiError::BadRequest("email body is too large".to_string()));
    }
    if !matches!(body_content_type, "text/plain" | "text/html") {
        return Err(ApiError::BadRequest(
            "email body content type is invalid".to_string(),
        ));
    }
    if idempotency_key.is_empty()
        || idempotency_key.len() > 240
        || idempotency_key.chars().any(char::is_whitespace)
    {
        return Err(ApiError::BadRequest(
            "email idempotency key is invalid".to_string(),
        ));
    }
    Ok(())
}

fn validate_delivery_settings(settings: &DeliverySettings) -> Result<()> {
    if !settings.enabled {
        anyhow::bail!("email delivery is disabled");
    }
    if settings.smtp_host.trim().is_empty() {
        anyhow::bail!("SMTP host is not configured");
    }
    if settings.from_email.trim().is_empty() {
        anyhow::bail!("sender address is not configured");
    }
    if settings.tls_mode == "none" && !plaintext_smtp_host_allowed(&settings.smtp_host) {
        anyhow::bail!("unencrypted SMTP is limited to loopback relays");
    }
    Ok(())
}

pub(crate) fn plaintext_smtp_host_allowed(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn failure_transition(attempts: i32, max_attempts: i32) -> FailureTransition {
    let attempts = attempts.saturating_add(1);
    FailureTransition {
        attempts,
        terminal: attempts >= max_attempts,
        backoff_seconds: backoff_seconds(attempts),
    }
}

fn backoff_seconds(attempts: i32) -> i64 {
    let exponent = attempts.saturating_sub(1).clamp(0, 5) as u32;
    (30_i64.saturating_mul(1_i64 << exponent)).min(15 * 60)
}

fn bounded_error(error: &anyhow::Error) -> String {
    error.to_string().chars().take(1000).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        DELIVERY_TIMEOUT, LEASE_SECONDS, backoff_seconds, failure_transition,
        plaintext_smtp_host_allowed, validate_outbox_message,
    };

    #[test]
    fn outbox_failures_stop_after_the_configured_limit() {
        let retry = failure_transition(3, 5);
        assert_eq!(retry.attempts, 4);
        assert!(!retry.terminal);

        let terminal = failure_transition(4, 5);
        assert_eq!(terminal.attempts, 5);
        assert!(terminal.terminal);
    }

    #[test]
    fn outbox_backoff_is_bounded() {
        assert_eq!(backoff_seconds(1), 30);
        assert_eq!(backoff_seconds(2), 60);
        assert_eq!(backoff_seconds(20), 900);
        assert!(LEASE_SECONDS > DELIVERY_TIMEOUT.as_secs() as i64);
    }

    #[test]
    fn outbox_rejects_header_injection_and_invalid_recipients() {
        assert!(
            validate_outbox_message("admin@example.com", "Test", "body", "text/plain", "test:1")
                .is_ok()
        );
        assert!(
            validate_outbox_message("not-an-email", "Test", "body", "text/plain", "test:2")
                .is_err()
        );
        assert!(
            validate_outbox_message(
                "admin@example.com",
                "Test\r\nBcc: x@y.com",
                "body",
                "text/plain",
                "test:3"
            )
            .is_err()
        );
        assert!(
            validate_outbox_message(
                "admin@example.com",
                "Test",
                "<p>body</p>",
                "text/html",
                "test:4"
            )
            .is_ok()
        );
    }

    #[test]
    fn plaintext_smtp_is_limited_to_loopback_relays() {
        assert!(plaintext_smtp_host_allowed("localhost"));
        assert!(plaintext_smtp_host_allowed("127.0.0.1"));
        assert!(plaintext_smtp_host_allowed("::1"));
        assert!(!plaintext_smtp_host_allowed("smtp.example.com"));
    }

    #[test]
    fn onboarding_migration_contains_bounded_outbox_and_no_subquery_check() {
        let migration = include_str!("../migrations/202607200013_account_onboarding.sql");

        assert!(migration.contains("CREATE TABLE registration_requests"));
        assert!(migration.contains("CREATE TABLE organization_auth_settings"));
        assert!(migration.contains("CREATE TABLE organization_email_settings"));
        assert!(migration.contains("CREATE TABLE email_outbox"));
        assert!(migration.contains("body_encrypted bytea"));
        assert!(!migration.contains("text_body text"));
        assert!(migration.contains("email_outbox_body_lifecycle"));
        assert!(migration.contains("BETWEEN 29 AND 1048604"));
        assert!(migration.contains(
            "body_encrypted IS NOT NULL OR sent_at IS NOT NULL OR failed_at IS NOT NULL"
        ));
        assert!(migration.contains("smtp_password_encrypted bytea"));
        assert!(migration.contains("max_attempts integer"));
        assert!(migration.contains("lease_owner uuid"));
        assert!(migration.contains("failed_at timestamptz"));
        assert!(migration.contains("ON registration_requests (lower(email))"));
        assert!(migration.contains("case-insensitive duplicates exist"));
        assert!(migration.contains("registration_requests_status_tokens"));
        assert!(migration.contains("registration_requests_primary_department_organization_fkey"));
        assert!(migration.contains("status <> 'approved'"));
        assert!(!migration.contains("status NOT IN ('approved', 'activated')"));
        assert!(!migration.contains("'expired'"));
        assert!(!migration.contains("ARRAY(\n      SELECT"));
    }
}
