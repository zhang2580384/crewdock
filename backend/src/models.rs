use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type, PartialEq, Eq)]
#[sqlx(type_name = "user_role", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    SystemAdmin,
    DepartmentAdmin,
    ProjectAdmin,
    ProjectMember,
    ReadonlyMember,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type, PartialEq, Eq)]
#[sqlx(type_name = "system_role", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum SystemRole {
    SystemAdmin,
    Member,
}

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq, Eq, PartialOrd, Ord,
)]
#[sqlx(type_name = "project_role", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ProjectRole {
    Viewer,
    Editor,
    Administrator,
}

impl ProjectRole {
    pub fn can_write(self) -> bool {
        self >= Self::Editor
    }

    pub fn can_manage(self) -> bool {
        self == Self::Administrator
    }
}

impl UserRole {
    pub fn can_manage(&self) -> bool {
        matches!(
            self,
            UserRole::SystemAdmin | UserRole::DepartmentAdmin | UserRole::ProjectAdmin
        )
    }

    pub fn can_write(&self) -> bool {
        !matches!(self, UserRole::ReadonlyMember)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type, PartialEq, Eq)]
#[sqlx(type_name = "ticket_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum TicketStatus {
    Pending,
    InProgress,
    WaitingConfirm,
    Done,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type, PartialEq, Eq)]
#[sqlx(type_name = "site_visibility", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum SiteVisibility {
    Public,
    Personal,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub department_id: Option<Uuid>,
    pub email: String,
    pub display_name: String,
    pub role: UserRole,
    pub system_role: SystemRole,
    pub is_active: bool,
    pub totp_enabled: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Project {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub department_id: Option<Uuid>,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ProjectListItem {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub department_id: Option<Uuid>,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
    pub access_role: ProjectRole,
    pub can_write: bool,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Department {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub sort_order: i32,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ProjectUserGrant {
    pub user_id: Uuid,
    pub display_name: String,
    pub email: String,
    pub project_role: ProjectRole,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ProjectDepartmentGrant {
    pub department_id: Uuid,
    pub department_name: String,
    pub project_role: ProjectRole,
    pub include_descendants: bool,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ProjectLink {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub url: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Site {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub owner_id: Option<Uuid>,
    pub visibility: SiteVisibility,
    pub category: String,
    pub title: String,
    pub resource_type: String,
    pub connection_config: Value,
    pub url: String,
    pub internal_url: Option<String>,
    pub description: Option<String>,
    pub icon_url: Option<String>,
    pub icon_text: Option<String>,
    pub auto_icon_url: Option<String>,
    pub icon_fetched_at: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
    pub is_pinned: bool,
    pub sort_order: i32,
    pub open_count: i64,
    pub last_opened_at: Option<DateTime<Utc>>,
    pub department_ids: Vec<Uuid>,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MarkdownDoc {
    pub id: Uuid,
    pub project_id: Uuid,
    pub folder_id: Option<Uuid>,
    pub title: String,
    pub body_markdown: String,
    pub tags: Vec<String>,
    pub sort_order: i32,
    pub created_by: Uuid,
    pub updated_at: DateTime<Utc>,
    #[sqlx(default)]
    pub can_write: bool,
    #[sqlx(default)]
    pub has_custom_access: bool,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MarkdownFolder {
    pub id: Uuid,
    pub project_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub sort_order: i32,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[sqlx(default)]
    pub can_write: bool,
    #[sqlx(default)]
    pub has_custom_access: bool,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct VaultItemListRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub kind: String,
    pub username: Option<String>,
    pub url: Option<String>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct FileAsset {
    pub id: Uuid,
    pub project_id: Uuid,
    pub filename: String,
    pub stored_path: String,
    pub content_type: Option<String>,
    pub size_bytes: i64,
    pub uploaded_by: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Ticket {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub description_markdown: String,
    pub status: TicketStatus,
    pub assignee_id: Option<Uuid>,
    pub created_by: Uuid,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AuditEvent {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub actor_id: Option<Uuid>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<Uuid>,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}
