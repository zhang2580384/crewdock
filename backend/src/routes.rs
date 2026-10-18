use crate::{
    accounts, app::AppState, auth, departments, email_templates, file_storage, files,
    models::AuditEvent, onboarding, projects, response::ok, settings, share, signing, sites,
    tickets, updates, vault,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderValue, Method},
    routing::{get, patch, post, put},
};
use serde::Serialize;
use tower_http::cors::CorsLayer;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
    public_base_url: String,
    session_secret_configured: bool,
    master_key_configured: bool,
    version: &'static str,
    git_sha: Option<&'static str>,
    build_time: Option<&'static str>,
}

#[derive(Serialize, sqlx::FromRow)]
struct CurrentCompanyResponse {
    id: uuid::Uuid,
    name: String,
    slug: String,
}

pub fn router(state: AppState) -> Router {
    let public_origin = public_origin(&state.config.public_base_url)
        .expect("CREWDOCK_PUBLIC_BASE_URL must contain a valid absolute URL");
    let cors = CorsLayer::new()
        .allow_origin(public_origin)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
        ]);

    Router::new()
        .route("/health", get(health))
        .route("/api/company", get(current_company))
        .route("/api/auth/login", post(auth::login))
        .route(
            "/api/auth/registration-policy",
            get(onboarding::registration_policy),
        )
        .route("/api/auth/register", post(onboarding::register))
        .route("/api/auth/verify-email", post(onboarding::verify_email))
        .route(
            "/api/auth/resend-verification",
            post(onboarding::resend_verification),
        )
        .route("/api/auth/activate", post(onboarding::activate))
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/change-password", post(auth::change_password))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/sessions", get(auth::list_sessions))
        .route(
            "/api/auth/sessions/{id}",
            axum::routing::delete(auth::revoke_session),
        )
        .route(
            "/api/admin/registration-requests",
            get(onboarding::list_pending_requests),
        )
        .route(
            "/api/admin/registration-requests/{id}/approve",
            post(onboarding::approve_request),
        )
        .route(
            "/api/admin/registration-requests/{id}/reject",
            post(onboarding::reject_request),
        )
        .route(
            "/api/admin/invitations",
            post(onboarding::create_invitation),
        )
        .route(
            "/api/admin/auth-settings",
            get(settings::get_auth_settings).put(settings::update_auth_settings),
        )
        .route(
            "/api/admin/email-settings",
            get(settings::get_email_settings).put(settings::update_email_settings),
        )
        .route(
            "/api/admin/email-settings/test",
            post(settings::test_email_settings),
        )
        .route(
            "/api/admin/email-templates",
            get(email_templates::list_email_templates),
        )
        .route(
            "/api/admin/email-templates/{key}",
            put(email_templates::update_email_template),
        )
        .route("/api/admin/updates", get(updates::get_status))
        .route("/api/admin/updates/check", post(updates::check_for_updates))
        .route("/api/admin/updates/apply", post(updates::apply_update))
        .route(
            "/api/seal-generator-template",
            get(signing::get_seal_generator_template).put(signing::update_seal_generator_template),
        )
        .route(
            "/api/seals",
            get(signing::list_seals)
                .post(signing::upload_seal)
                .layer(DefaultBodyLimit::max(6 * 1024 * 1024)),
        )
        .route(
            "/api/seals/{id}",
            patch(signing::update_seal)
                .put(signing::replace_seal)
                .delete(signing::delete_seal)
                .layer(DefaultBodyLimit::max(6 * 1024 * 1024)),
        )
        .route("/api/seals/{id}/image", get(signing::seal_image))
        .route(
            "/api/seals/{id}/authorizations",
            get(signing::get_seal_authorizations).put(signing::replace_seal_authorizations),
        )
        .route(
            "/api/signing/documents",
            get(signing::list_documents)
                .post(signing::upload_document)
                .layer(DefaultBodyLimit::max(32 * 1024 * 1024)),
        )
        .route(
            "/api/signing/documents/{id}",
            axum::routing::delete(signing::delete_document),
        )
        .route(
            "/api/signing/documents/{id}/source",
            get(signing::document_source),
        )
        .route(
            "/api/signing/documents/{id}/finalize",
            post(signing::finalize_document),
        )
        .route(
            "/api/signing/documents/{id}/download",
            get(signing::download_signed_document),
        )
        .route(
            "/api/signing/documents/{id}/images",
            get(signing::export_signed_images),
        )
        .route(
            "/api/users",
            get(accounts::list_users).post(accounts::create_user),
        )
        .route("/api/users/{id}", patch(accounts::update_user))
        .route(
            "/api/users/{id}/managed-departments",
            get(accounts::get_managed_departments).put(accounts::update_managed_departments),
        )
        .route(
            "/api/users/{id}/reset-password",
            post(accounts::reset_password),
        )
        .route(
            "/api/users/{id}/project-grants",
            get(accounts::list_user_project_grants),
        )
        .route(
            "/api/users/{id}/project-grants/{project_id}",
            put(accounts::put_user_project_grant).delete(accounts::delete_user_project_grant),
        )
        .route(
            "/api/admin/file-storage-jobs/{job_id}/retry",
            post(file_storage::retry_job),
        )
        .route(
            "/api/departments",
            get(departments::list_departments).post(departments::create_department),
        )
        .route(
            "/api/departments/{id}",
            patch(departments::update_department).delete(departments::delete_department),
        )
        .route(
            "/api/departments/{id}/move",
            post(departments::move_department),
        )
        .route(
            "/api/sites",
            get(sites::list_sites).post(sites::create_site),
        )
        .route(
            "/api/sites/{id}",
            patch(sites::update_site).delete(sites::delete_site),
        )
        .route("/api/sites/{id}/open", post(sites::record_site_open))
        .route(
            "/api/sites/{id}/icon/refresh",
            post(sites::refresh_site_icon),
        )
        .route(
            "/api/sites/import-bookmarks",
            post(sites::import_bookmarks).layer(DefaultBodyLimit::max(5 * 1024 * 1024)),
        )
        .route(
            "/api/projects",
            get(projects::list_projects).post(projects::create_project),
        )
        .route(
            "/api/projects/{project_id}",
            patch(projects::update_project).delete(projects::delete_project),
        )
        .route(
            "/api/projects/{project_id}/members",
            get(projects::list_project_assignable_users),
        )
        .route(
            "/api/projects/{project_id}/grant-candidates",
            get(projects::list_project_grant_candidates),
        )
        .route(
            "/api/projects/{project_id}/archive",
            post(projects::archive_project),
        )
        .route(
            "/api/projects/{project_id}/restore",
            post(projects::restore_project),
        )
        .route(
            "/api/projects/{project_id}/grants/users",
            get(projects::list_project_user_grants),
        )
        .route(
            "/api/projects/{project_id}/grants/users/{user_id}",
            put(projects::put_project_user_grant).delete(projects::delete_project_user_grant),
        )
        .route(
            "/api/projects/{project_id}/grants/departments",
            get(projects::list_project_department_grants),
        )
        .route(
            "/api/projects/{project_id}/grants/departments/{department_id}",
            put(projects::put_project_department_grant)
                .delete(projects::delete_project_department_grant),
        )
        .route(
            "/api/projects/{project_id}/links",
            get(projects::list_links).post(projects::create_link),
        )
        .route(
            "/api/projects/{project_id}/links/{id}",
            patch(projects::update_link).delete(projects::delete_link),
        )
        .route(
            "/api/projects/{project_id}/docs",
            get(projects::list_docs).post(projects::create_doc),
        )
        .route(
            "/api/projects/{project_id}/docs/{id}",
            patch(projects::update_doc).delete(projects::delete_doc),
        )
        .route(
            "/api/projects/{project_id}/docs/{id}/move",
            post(projects::move_doc),
        )
        .route(
            "/api/projects/{project_id}/docs/{id}/permissions",
            get(projects::list_doc_department_permissions)
                .put(projects::replace_doc_department_permissions),
        )
        .route(
            "/api/projects/{project_id}/folders",
            get(projects::list_folders).post(projects::create_folder),
        )
        .route(
            "/api/projects/{project_id}/folders/{id}",
            patch(projects::update_folder).delete(projects::delete_folder),
        )
        .route(
            "/api/projects/{project_id}/folders/{id}/move",
            post(projects::move_folder),
        )
        .route(
            "/api/projects/{project_id}/folders/{id}/permissions",
            get(projects::list_folder_department_permissions)
                .put(projects::replace_folder_department_permissions),
        )
        .route(
            "/api/vault",
            get(vault::list_vault).post(vault::create_vault_item),
        )
        .route(
            "/api/vault/{id}",
            patch(vault::update_vault_item).delete(vault::delete_vault_item),
        )
        .route("/api/vault/{id}/reveal", post(vault::reveal_secret))
        .route(
            "/api/projects/{project_id}/files",
            get(files::list_files)
                .post(files::upload_file)
                .layer(DefaultBodyLimit::max(200 * 1024 * 1024)),
        )
        .route(
            "/api/projects/{project_id}/files/{id}",
            patch(files::update_file).delete(files::delete_file),
        )
        .route(
            "/api/projects/{project_id}/files/{id}/download",
            get(files::download_file),
        )
        .route(
            "/api/tickets",
            get(tickets::list_tickets).post(tickets::create_ticket),
        )
        .route(
            "/api/tickets/{id}/status",
            patch(tickets::update_ticket_status),
        )
        .route(
            "/api/tickets/{id}",
            patch(tickets::update_ticket).delete(tickets::delete_ticket),
        )
        .route(
            "/api/share-links",
            get(share::list_share_links).post(share::create_share_link),
        )
        .route(
            "/api/share-links/{id}/resolve",
            get(share::resolve_share_link),
        )
        .route(
            "/api/share-links/{id}",
            axum::routing::delete(share::delete_share_link),
        )
        .route("/api/audit-events", get(list_audit_events))
        .layer(cors)
        .with_state(state)
}

async fn health(
    State(state): State<AppState>,
) -> Json<crate::response::ApiEnvelope<HealthResponse>> {
    ok(HealthResponse {
        status: "ok",
        service: "crewdock-api",
        public_base_url: state.config.public_base_url,
        session_secret_configured: session_secret_configured(&state.config.session_secret),
        master_key_configured: state.config.master_key != [0u8; 32],
        version: env!("CARGO_PKG_VERSION"),
        git_sha: option_env!("CREWDOCK_GIT_SHA"),
        build_time: option_env!("CREWDOCK_BUILD_TIME"),
    })
}

fn public_origin(value: &str) -> Result<HeaderValue, String> {
    reqwest::Url::parse(value)
        .map_err(|error| error.to_string())?
        .origin()
        .ascii_serialization()
        .parse::<HeaderValue>()
        .map_err(|error| error.to_string())
}

fn session_secret_configured(value: &str) -> bool {
    value.len() >= 32 && !value.starts_with("change-me")
}

async fn current_company(
    State(state): State<AppState>,
    current: auth::CurrentUser,
) -> crate::error::ApiResult<Json<crate::response::ApiEnvelope<CurrentCompanyResponse>>> {
    let company = sqlx::query_as::<_, CurrentCompanyResponse>(
        "SELECT id, name, slug FROM organizations WHERE id = $1",
    )
    .bind(current.organization_id)
    .fetch_one(&state.db)
    .await?;
    Ok(ok(company))
}

async fn list_audit_events(
    State(state): State<AppState>,
    current: auth::CurrentUser,
) -> crate::error::ApiResult<Json<crate::response::ApiEnvelope<Vec<AuditEvent>>>> {
    if current.system_role != crate::models::SystemRole::SystemAdmin {
        return Err(crate::error::ApiError::Forbidden);
    }
    let rows = sqlx::query_as::<_, AuditEvent>(
        r#"
        SELECT id, organization_id, actor_id, action, target_type, target_id, metadata, created_at
        FROM audit_events
        WHERE organization_id = $1
        ORDER BY created_at DESC
        LIMIT 200
        "#,
    )
    .bind(current.organization_id)
    .fetch_all(&state.db)
    .await?;
    Ok(ok(rows))
}

#[cfg(test)]
mod tests {
    use super::{public_origin, session_secret_configured};

    #[test]
    fn cors_origin_is_restricted_to_the_configured_site_origin() {
        assert_eq!(
            public_origin("https://work.example.com/app")
                .unwrap()
                .to_str()
                .unwrap(),
            "https://work.example.com"
        );
        assert!(public_origin("not a url").is_err());
    }

    #[test]
    fn health_rejects_placeholder_session_secrets() {
        assert!(!session_secret_configured("change-me"));
        assert!(!session_secret_configured(
            "change-me-to-a-long-random-session-secret"
        ));
        assert!(session_secret_configured(&"s".repeat(32)));
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }
}
