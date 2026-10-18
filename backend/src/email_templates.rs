use crate::{
    app::AppState,
    audit::log_event_in_transaction,
    auth::{self, CurrentUser},
    error::{ApiError, ApiResult},
    models::SystemRole,
    response::{ApiEnvelope, ok},
};
use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{DateTime, Utc};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{FromRow, Postgres, Transaction};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

const REGISTRATION_VERIFICATION_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <style>
    body { margin: 0; padding: 24px; background: #f3f5f4; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: #1b2521; }
    .container { max-width: 640px; margin: 0 auto; background: #ffffff; border: 1px solid #dfe6e2; border-radius: 8px; overflow: hidden; }
    .header { background: #176b4d; color: #ffffff; padding: 28px 32px; }
    .header h1 { margin: 0; font-size: 24px; line-height: 1.25; }
    .content { padding: 32px; font-size: 15px; line-height: 1.7; }
    .code { margin: 22px 0; padding: 18px; border: 1px solid #b8d4c8; background: #f3faf6; color: #145a42; font-family: ui-monospace, SFMono-Regular, Consolas, monospace; font-size: 32px; font-weight: 700; text-align: center; letter-spacing: 0; }
    .muted { color: #66736d; font-size: 13px; }
    .footer { padding: 18px 32px; background: #f8faf9; color: #7b8882; font-size: 12px; }
  </style>
</head>
<body>
  <div class="container">
    <div class="header"><h1>邮箱验证码</h1></div>
    <div class="content">
      <p>{{recipient_name}}，您好：</p>
      <p>请输入以下验证码，完成工作台访问申请的邮箱验证：</p>
      <div class="code">{{verification_code}}</div>
      <p>验证码将在 <strong>{{expires_in_minutes}}</strong> 分钟后失效。若不是您本人操作，请忽略此邮件。</p>
      <p class="muted">为保护账号安全，请勿把验证码转发给其他人。</p>
    </div>
    <div class="footer">此邮件由 {{site_name}} 自动发送，请勿直接回复。</div>
  </div>
</body>
</html>"#;

const REGISTRATION_APPROVED_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <style>
    body { margin: 0; padding: 24px; background: #f3f5f4; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: #1b2521; }
    .container { max-width: 640px; margin: 0 auto; background: #ffffff; border: 1px solid #dfe6e2; border-radius: 8px; overflow: hidden; }
    .header { background: #176b4d; color: #ffffff; padding: 28px 32px; }
    .header h1 { margin: 0; font-size: 24px; line-height: 1.25; }
    .content { padding: 32px; font-size: 15px; line-height: 1.7; }
    .button { display: inline-block; margin: 12px 0; padding: 11px 18px; border-radius: 6px; background: #176b4d; color: #ffffff !important; text-decoration: none; font-weight: 600; }
    .footer { padding: 18px 32px; background: #f8faf9; color: #7b8882; font-size: 12px; }
  </style>
</head>
<body>
  <div class="container">
    <div class="header"><h1>申请已通过</h1></div>
    <div class="content">
      <p>{{recipient_name}}，您好：</p>
      <p>您的工作台访问申请已通过审核。请在 <strong>{{expires_in_hours}}</strong> 小时内设置密码并激活账号。</p>
      <p><a class="button" href="{{activation_url}}">设置密码并激活</a></p>
      <p>若按钮无法打开，请复制以下地址到浏览器：<br>{{activation_url}}</p>
    </div>
    <div class="footer">此邮件由 {{site_name}} 自动发送，请勿直接回复。</div>
  </div>
</body>
</html>"#;

const REGISTRATION_REJECTED_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <style>
    body { margin: 0; padding: 24px; background: #f3f5f4; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: #1b2521; }
    .container { max-width: 640px; margin: 0 auto; background: #ffffff; border: 1px solid #dfe6e2; border-radius: 8px; overflow: hidden; }
    .header { background: #36433d; color: #ffffff; padding: 28px 32px; }
    .header h1 { margin: 0; font-size: 24px; line-height: 1.25; }
    .content { padding: 32px; font-size: 15px; line-height: 1.7; }
    .reason { padding: 14px 16px; border-left: 3px solid #8d9a94; background: #f7f9f8; }
    .footer { padding: 18px 32px; background: #f8faf9; color: #7b8882; font-size: 12px; }
  </style>
</head>
<body>
  <div class="container">
    <div class="header"><h1>申请审核结果</h1></div>
    <div class="content">
      <p>{{recipient_name}}，您好：</p>
      <p>您的工作台访问申请暂未通过审核。</p>
      <div class="reason">{{rejection_reason}}</div>
      <p>如需继续申请，请联系系统管理员。</p>
    </div>
    <div class="footer">此邮件由 {{site_name}} 自动发送，请勿直接回复。</div>
  </div>
</body>
</html>"#;

const ACCOUNT_INVITATION_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <style>
    body { margin: 0; padding: 24px; background: #f3f5f4; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: #1b2521; }
    .container { max-width: 640px; margin: 0 auto; background: #ffffff; border: 1px solid #dfe6e2; border-radius: 8px; overflow: hidden; }
    .header { background: #176b4d; color: #ffffff; padding: 28px 32px; }
    .header h1 { margin: 0; font-size: 24px; line-height: 1.25; }
    .content { padding: 32px; font-size: 15px; line-height: 1.7; }
    .button { display: inline-block; margin: 12px 0; padding: 11px 18px; border-radius: 6px; background: #176b4d; color: #ffffff !important; text-decoration: none; font-weight: 600; }
    .footer { padding: 18px 32px; background: #f8faf9; color: #7b8882; font-size: 12px; }
  </style>
</head>
<body>
  <div class="container">
    <div class="header"><h1>工作台账号邀请</h1></div>
    <div class="content">
      <p>{{recipient_name}}，您好：</p>
      <p>{{inviter_name}} 已邀请您加入 {{site_name}}。请在 <strong>{{expires_in_hours}}</strong> 小时内设置初始密码。</p>
      <p><a class="button" href="{{activation_url}}">接受邀请并激活</a></p>
      <p>若按钮无法打开，请复制以下地址到浏览器：<br>{{activation_url}}</p>
    </div>
    <div class="footer">此邮件由 {{site_name}} 自动发送，请勿直接回复。</div>
  </div>
</body>
</html>"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemplateKey {
    RegistrationVerification,
    RegistrationApproved,
    RegistrationRejected,
    AccountInvitation,
}

impl TemplateKey {
    pub const ALL: [Self; 4] = [
        Self::RegistrationVerification,
        Self::RegistrationApproved,
        Self::RegistrationRejected,
        Self::AccountInvitation,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::RegistrationVerification => "registration_verification",
            Self::RegistrationApproved => "registration_approved",
            Self::RegistrationRejected => "registration_rejected",
            Self::AccountInvitation => "account_invitation",
        }
    }

    fn parse(value: &str) -> ApiResult<Self> {
        Self::ALL
            .into_iter()
            .find(|key| key.as_str() == value)
            .ok_or_else(|| ApiError::BadRequest("email template key is invalid".to_string()))
    }

    fn label(self) -> &'static str {
        match self {
            Self::RegistrationVerification => "注册验证码",
            Self::RegistrationApproved => "申请通过",
            Self::RegistrationRejected => "申请拒绝",
            Self::AccountInvitation => "账号邀请",
        }
    }

    fn default_subject(self) -> &'static str {
        match self {
            Self::RegistrationVerification => "{{site_name}} 邮箱验证码",
            Self::RegistrationApproved => "你的 {{site_name}} 申请已通过",
            Self::RegistrationRejected => "{{site_name}} 申请审核结果",
            Self::AccountInvitation => "{{inviter_name}} 邀请你加入 {{site_name}}",
        }
    }

    fn default_html(self) -> &'static str {
        match self {
            Self::RegistrationVerification => REGISTRATION_VERIFICATION_HTML,
            Self::RegistrationApproved => REGISTRATION_APPROVED_HTML,
            Self::RegistrationRejected => REGISTRATION_REJECTED_HTML,
            Self::AccountInvitation => ACCOUNT_INVITATION_HTML,
        }
    }

    fn variables(self) -> &'static [&'static str] {
        match self {
            Self::RegistrationVerification => &[
                "recipient_name",
                "verification_code",
                "expires_in_minutes",
                "site_name",
            ],
            Self::RegistrationApproved => &[
                "recipient_name",
                "activation_url",
                "expires_in_hours",
                "site_name",
            ],
            Self::RegistrationRejected => &["recipient_name", "rejection_reason", "site_name"],
            Self::AccountInvitation => &[
                "recipient_name",
                "activation_url",
                "expires_in_hours",
                "inviter_name",
                "site_name",
            ],
        }
    }

    fn critical_variable(self) -> Option<&'static str> {
        match self {
            Self::RegistrationVerification => Some("verification_code"),
            Self::RegistrationApproved | Self::AccountInvitation => Some("activation_url"),
            Self::RegistrationRejected => None,
        }
    }
}

#[derive(Debug, FromRow)]
struct TemplateRow {
    template_key: String,
    subject: String,
    html_body: String,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct EmailTemplateView {
    pub key: String,
    pub label: String,
    pub subject: String,
    pub html_body: String,
    pub default_subject: String,
    pub default_html_body: String,
    pub variables: Vec<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateEmailTemplateRequest {
    pub subject: String,
    pub html_body: String,
}

#[derive(Debug)]
pub struct RenderedEmail {
    pub subject: String,
    pub html_body: String,
}

pub async fn list_email_templates(
    State(state): State<AppState>,
    current: CurrentUser,
) -> ApiResult<Json<ApiEnvelope<Vec<EmailTemplateView>>>> {
    ensure_system_admin(&current)?;
    let rows = sqlx::query_as::<_, TemplateRow>(
        r#"
        SELECT template_key, subject, html_body, updated_at
        FROM organization_email_templates
        WHERE organization_id = $1
        "#,
    )
    .bind(current.organization_id)
    .fetch_all(&state.db)
    .await?;
    let rows = rows
        .into_iter()
        .map(|row| (row.template_key.clone(), row))
        .collect::<BTreeMap<_, _>>();
    let views = TemplateKey::ALL
        .into_iter()
        .map(|key| view_for(key, rows.get(key.as_str())))
        .collect();
    Ok(ok(views))
}

pub async fn update_email_template(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(key): Path<String>,
    Json(payload): Json<UpdateEmailTemplateRequest>,
) -> ApiResult<Json<ApiEnvelope<EmailTemplateView>>> {
    let key = TemplateKey::parse(&key)?;
    validate_template(key, &payload.subject, &payload.html_body)?;
    let subject = payload.subject.trim().to_string();
    let mut transaction = state.db.begin().await?;
    let actor = auth::lock_current_user_in_transaction(&mut transaction, &current).await?;
    ensure_system_admin(&actor)?;
    let row = sqlx::query_as::<_, TemplateRow>(
        r#"
        INSERT INTO organization_email_templates (
          organization_id, template_key, subject, html_body, updated_by, updated_at
        )
        VALUES ($1, $2, $3, $4, $5, now())
        ON CONFLICT (organization_id, template_key) DO UPDATE
        SET subject = EXCLUDED.subject,
            html_body = EXCLUDED.html_body,
            updated_by = EXCLUDED.updated_by,
            updated_at = now()
        RETURNING template_key, subject, html_body, updated_at
        "#,
    )
    .bind(actor.organization_id)
    .bind(key.as_str())
    .bind(subject)
    .bind(payload.html_body)
    .bind(actor.id)
    .fetch_one(&mut *transaction)
    .await?;
    log_event_in_transaction(
        &mut transaction,
        actor.organization_id,
        Some(actor.id),
        "settings.email_template.update",
        "organization_email_template",
        None,
        json!({ "template_key": key.as_str() }),
    )
    .await?;
    transaction.commit().await?;
    Ok(ok(view_for(key, Some(&row))))
}

pub async fn render_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    key: TemplateKey,
    values: &BTreeMap<&str, String>,
) -> ApiResult<RenderedEmail> {
    let row: Option<(String, String)> = sqlx::query_as(
        r#"
        SELECT subject, html_body
        FROM organization_email_templates
        WHERE organization_id = $1 AND template_key = $2
        "#,
    )
    .bind(organization_id)
    .bind(key.as_str())
    .fetch_optional(&mut **transaction)
    .await?;
    let (subject, html_body) = row.unwrap_or_else(|| {
        (
            key.default_subject().to_string(),
            key.default_html().to_string(),
        )
    });
    validate_template(key, &subject, &html_body)?;
    Ok(RenderedEmail {
        subject: render_variables(&subject, values, false)?,
        html_body: render_variables(&html_body, values, true)?,
    })
}

fn view_for(key: TemplateKey, row: Option<&TemplateRow>) -> EmailTemplateView {
    EmailTemplateView {
        key: key.as_str().to_string(),
        label: key.label().to_string(),
        subject: row
            .map(|row| row.subject.clone())
            .unwrap_or_else(|| key.default_subject().to_string()),
        html_body: row
            .map(|row| row.html_body.clone())
            .unwrap_or_else(|| key.default_html().to_string()),
        default_subject: key.default_subject().to_string(),
        default_html_body: key.default_html().to_string(),
        variables: key
            .variables()
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        updated_at: row.map(|row| row.updated_at),
    }
}

fn validate_template(key: TemplateKey, subject: &str, html_body: &str) -> ApiResult<()> {
    let subject = subject.trim();
    if subject.is_empty() || subject.chars().count() > 300 {
        return Err(ApiError::BadRequest(
            "email template subject must contain 1 to 300 characters".to_string(),
        ));
    }
    if html_body.is_empty() || html_body.len() > 200_000 {
        return Err(ApiError::BadRequest(
            "email template HTML must contain 1 to 200000 bytes".to_string(),
        ));
    }
    let lowercase = html_body.to_ascii_lowercase();
    if !lowercase.contains("<html") || !lowercase.contains("<body") {
        return Err(ApiError::BadRequest(
            "email template must contain html and body elements".to_string(),
        ));
    }
    let allowed = key.variables().iter().copied().collect::<BTreeSet<_>>();
    let mut used = template_variables(subject)?;
    used.extend(template_variables(html_body)?);
    if let Some(variable) = used
        .iter()
        .find(|variable| !allowed.contains(variable.as_str()))
    {
        return Err(ApiError::BadRequest(format!(
            "email template variable is not allowed: {variable}"
        )));
    }
    if let Some(required) = key.critical_variable()
        && !used.contains(required)
    {
        return Err(ApiError::BadRequest(format!(
            "email template must contain {{{{{required}}}}}"
        )));
    }

    let document = Html::parse_document(html_body);
    let unsafe_selector = Selector::parse("script, iframe, object, embed, form, base").unwrap();
    if document.select(&unsafe_selector).next().is_some() {
        return Err(ApiError::BadRequest(
            "email template contains an unsafe element".to_string(),
        ));
    }
    let all_selector = Selector::parse("*").unwrap();
    for element in document.select(&all_selector) {
        for (name, value) in element.value().attrs() {
            let name = name.to_ascii_lowercase();
            let value = value.trim().to_ascii_lowercase();
            if name.starts_with("on") || value.starts_with("javascript:") {
                return Err(ApiError::BadRequest(
                    "email template contains an unsafe attribute".to_string(),
                ));
            }
            if element.value().name() == "meta" && name == "http-equiv" && !value.is_empty() {
                return Err(ApiError::BadRequest(
                    "email template contains an unsafe meta directive".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn template_variables(input: &str) -> ApiResult<BTreeSet<String>> {
    let mut variables = BTreeSet::new();
    let mut cursor = 0;
    while let Some(relative_start) = input[cursor..].find("{{") {
        let start = cursor + relative_start;
        if input[cursor..start].contains("}}") {
            return Err(ApiError::BadRequest(
                "email template contains malformed variables".to_string(),
            ));
        }
        let value_start = start + 2;
        let relative_end = input[value_start..].find("}}").ok_or_else(|| {
            ApiError::BadRequest("email template contains an unclosed variable".to_string())
        })?;
        let end = value_start + relative_end;
        let value = input[value_start..end].trim();
        if value.is_empty()
            || value.contains("{{")
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(ApiError::BadRequest(
                "email template variable syntax is invalid".to_string(),
            ));
        }
        variables.insert(value.to_string());
        cursor = end + 2;
    }
    if input[cursor..].contains("}}") {
        return Err(ApiError::BadRequest(
            "email template contains malformed variables".to_string(),
        ));
    }
    Ok(variables)
}

fn render_variables(input: &str, values: &BTreeMap<&str, String>, html: bool) -> ApiResult<String> {
    template_variables(input)?;
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(relative_start) = input[cursor..].find("{{") {
        let start = cursor + relative_start;
        let value_start = start + 2;
        let end = value_start
            + input[value_start..]
                .find("}}")
                .expect("validated template variable");
        let variable = input[value_start..end].trim();
        let value = values.get(variable).ok_or_else(|| {
            ApiError::Internal(anyhow::anyhow!(
                "email template variable is missing: {variable}"
            ))
        })?;
        output.push_str(&input[cursor..start]);
        if html {
            output.push_str(&html_escape(value));
        } else {
            output.push_str(&value.replace(['\r', '\n'], " "));
        }
        cursor = end + 2;
    }
    output.push_str(&input[cursor..]);
    Ok(output)
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn ensure_system_admin(current: &CurrentUser) -> ApiResult<()> {
    if current.system_role != SystemRole::SystemAdmin {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        REGISTRATION_VERIFICATION_HTML, TemplateKey, html_escape, template_variables,
        validate_template,
    };

    #[test]
    fn default_verification_template_is_valid() {
        validate_template(
            TemplateKey::RegistrationVerification,
            "{{site_name}} 邮箱验证码",
            REGISTRATION_VERIFICATION_HTML,
        )
        .unwrap();
    }

    #[test]
    fn template_rejects_unknown_variables_and_scripts() {
        assert!(
            validate_template(
                TemplateKey::RegistrationVerification,
                "验证码",
                "<html><body>{{verification_code}}{{unknown}}</body></html>",
            )
            .is_err()
        );
        assert!(
            validate_template(
                TemplateKey::RegistrationVerification,
                "验证码",
                "<html><body><script>alert(1)</script>{{verification_code}}</body></html>",
            )
            .is_err()
        );
    }

    #[test]
    fn variables_are_strict_and_values_are_escaped() {
        let variables = template_variables("{{site_name}} {{ verification_code }}").unwrap();
        assert!(variables.contains("site_name"));
        assert!(variables.contains("verification_code"));
        assert!(template_variables("{{bad-name}}").is_err());
        assert_eq!(html_escape("<A&B>"), "&lt;A&amp;B&gt;");
    }
}
