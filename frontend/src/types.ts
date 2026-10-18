export type UserRole =
  | 'system_admin'
  | 'department_admin'
  | 'project_admin'
  | 'project_member'
  | 'readonly_member';

export type SystemRole = 'system_admin' | 'member';
export type ProjectRole = 'administrator' | 'editor' | 'viewer';
export type RegistrationMode = 'closed' | 'application';

export type TicketStatus = 'pending' | 'in_progress' | 'waiting_confirm' | 'done' | 'closed';

export interface Company {
  id: string;
  name: string;
  slug: string;
}

export interface User {
  id: string;
  organization_id: string;
  department_id: string | null;
  email: string;
  display_name: string;
  role: UserRole;
  system_role: SystemRole;
  is_active: boolean;
  totp_enabled: boolean;
  created_at: string;
}

export interface AccountCapabilities {
  can_edit_profile: boolean;
  can_change_system_role: boolean;
  can_change_department: boolean;
  can_change_active: boolean;
  can_reset_password: boolean;
}

export interface Account extends User {
  department_ids: string[];
  capabilities: AccountCapabilities;
}

export interface AccountManagementCapabilities {
  can_create_users: boolean;
  can_assign_system_role: boolean;
  can_create_without_department: boolean;
  manageable_department_ids: string[];
}

export interface AccountManagement {
  users: Account[];
  capabilities: AccountManagementCapabilities;
}

export interface AccountProjectGrant {
  project_id: string;
  project_name: string;
  project_role: ProjectRole;
}

export interface Project {
  id: string;
  organization_id: string;
  department_id: string | null;
  name: string;
  slug: string;
  description: string | null;
  metadata: Record<string, unknown>;
  created_at: string;
  archived_at: string | null;
  can_write?: boolean;
}

export interface SessionUser {
  id: string;
  organization_id: string;
  department_id: string | null;
  department_ids: string[];
  email: string;
  display_name: string;
  role: UserRole;
  system_role: SystemRole;
  can_manage_accounts: boolean;
  can_manage_departments: boolean;
  can_view_audit: boolean;
}

export interface RegistrationPolicy {
  registration_mode: RegistrationMode;
  allow_all_email_domains: boolean;
  allowed_email_domains: string[];
  email_ready: boolean;
  verification_expires_in_minutes: number;
  resend_cooldown_seconds: number;
}

export interface VerificationAccepted {
  accepted: boolean;
  verification_expires_in_minutes: number;
  resend_cooldown_seconds: number;
}

export interface RegistrationRequest {
  id: string;
  email: string;
  display_name: string;
  department_interest?: string | null;
  reason?: string | null;
  status: string;
  email_verified_at?: string | null;
  created_at: string;
  reviewed_at?: string | null;
  rejection_reason?: string | null;
  approved_department_ids: string[];
  primary_department_id?: string | null;
  activated_user_id?: string | null;
}

export interface AuthSettings {
  registration_mode: RegistrationMode;
  allow_all_email_domains: boolean;
  allowed_email_domains: string[];
  updated_at?: string;
}

export type SmtpTlsMode = 'none' | 'starttls' | 'tls';

export interface EmailSettings {
  enabled: boolean;
  smtp_host: string;
  smtp_port: number;
  tls_mode: SmtpTlsMode;
  smtp_username?: string | null;
  from_email: string;
  from_name: string;
  reply_to?: string | null;
  has_password: boolean;
  email_ready: boolean;
  last_test_at?: string | null;
  last_test_succeeded?: boolean | null;
  last_test_error?: string | null;
  updated_at: string;
}

export interface UpdateEmailSettings {
  enabled: boolean;
  smtp_host: string;
  smtp_port: number;
  tls_mode: SmtpTlsMode;
  smtp_username?: string;
  smtp_password?: string;
  clear_smtp_password: boolean;
  from_email: string;
  from_name: string;
  reply_to?: string;
}

export type EmailTemplateKey =
  | 'registration_verification'
  | 'registration_approved'
  | 'registration_rejected'
  | 'account_invitation';

export interface EmailTemplate {
  key: EmailTemplateKey;
  label: string;
  subject: string;
  html_body: string;
  default_subject: string;
  default_html_body: string;
  variables: string[];
  updated_at?: string | null;
}

export interface Seal {
  id: string;
  name: string;
  width_px: number;
  height_px: number;
  size_bytes: number;
  source_kind: 'upload' | 'generated' | 'legacy_generated';
  generator_definition?: SealGeneratorDefinition | null;
  authorized_user_count: number;
  created_at: string;
  updated_at: string;
}

export interface SealAuthorizationUser {
  user_id: string;
  display_name: string;
  email: string;
  department_name?: string | null;
  authorized: boolean;
}

export interface SealAuthorizations {
  seal_id: string;
  users: SealAuthorizationUser[];
}

export interface SealImageOptions {
  remove_background: boolean;
  red_ink_only: boolean;
  ink_texture: boolean;
}

export interface SealGeneratorSettings {
  schema_version: 2;
  template: 'round' | 'oval' | 'square';
  color: '#d6212e' | '#ef2a2d' | '#b91f2b';
  wear: number;
  outer_border_width: number;
  organization_font_size: number;
  organization_spacing: number;
  organization_inset: number;
  organization_height: number;
  organization_width: number;
  text_stroke_width: number;
  star_size: number;
  purpose_offset_y: number;
  purpose_font_size: number;
  purpose_spacing: number;
  serial_font_size: number;
  serial_spacing: number;
  serial_inset: number;
  serial_width: number;
}

export interface SealGeneratorDefinition {
  schema_version: 1;
  organization: string;
  purpose: string;
  serial: string;
  settings: SealGeneratorSettings;
}

export interface SealGeneratorTemplate {
  settings: SealGeneratorSettings;
  saved: boolean;
  updated_at?: string | null;
}

export interface SigningDocument {
  id: string;
  original_filename: string;
  source_format: 'pdf' | 'doc' | 'docx' | 'png' | 'jpg' | 'webp';
  source_size_bytes: number;
  page_count: number;
  has_signed_result: boolean;
  signed_at?: string | null;
  created_at: string;
  updated_at: string;
}

export interface StampPlacement {
  seal_id: string;
  page: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface SeamPlacement {
  seal_id: string;
  start_page: number;
  end_page: number;
  y: number;
  height: number;
}

export type FinalizeSigningRequest =
  | { mode: 'single'; placements: StampPlacement[]; seam?: never }
  | { mode: 'seam'; placements: []; seam: SeamPlacement };

export interface UserSession {
  id: string;
  client_ip?: string | null;
  created_at: string;
  last_seen_at: string;
  expires_at: string;
  current: boolean;
}

export interface HealthInfo {
  status: string;
  service: string;
  public_base_url: string;
  session_secret_configured: boolean;
  master_key_configured: boolean;
  version: string;
  git_sha?: string | null;
  build_time?: string | null;
}

export type SystemUpdateState =
  | 'not_configured'
  | 'idle'
  | 'queued'
  | 'checking'
  | 'available'
  | 'up_to_date'
  | 'backing_up'
  | 'downloading'
  | 'building'
  | 'deploying'
  | 'verifying'
  | 'rolling_back'
  | 'rolled_back'
  | 'completed'
  | 'failed';

export interface SystemUpdateStatus {
  current_version: string;
  latest_version?: string | null;
  target_version?: string | null;
  update_available: boolean;
  updater_configured: boolean;
  auto_check_enabled: boolean;
  can_check: boolean;
  can_apply: boolean;
  state: SystemUpdateState;
  operation?: 'check' | 'apply' | null;
  message: string;
  error?: string | null;
  progress: number;
  checked_at?: string | null;
  started_at?: string | null;
  finished_at?: string | null;
}

export interface ProjectListItem extends Project {
  access_role: ProjectRole;
  can_write: boolean;
}

export interface ProjectAssignableUser {
  id: string;
  email: string;
  display_name: string;
  department_id: string | null;
  access_role: ProjectRole;
}

export interface ProjectGrantCandidate {
  id: string;
  email: string;
  display_name: string;
  department_id: string | null;
}

export interface Department {
  id: string;
  organization_id: string;
  parent_id: string | null;
  name: string;
  sort_order: number;
}

export interface ProjectUserGrant {
  user_id: string;
  display_name: string;
  email: string;
  project_role: ProjectRole;
}

export interface ProjectDepartmentGrant {
  department_id: string;
  department_name: string;
  project_role: ProjectRole;
  include_descendants: boolean;
}

export interface PutProjectUserGrantRequest {
  project_role: ProjectRole;
}

export interface PutProjectDepartmentGrantRequest {
  project_role: ProjectRole;
  include_descendants: boolean;
}

export interface ProjectLink {
  id: string;
  project_id: string;
  title: string;
  url: string;
  description?: string | null;
  tags: string[];
  created_at: string;
}

export type SiteVisibility = 'public' | 'personal';
export type SiteResourceType = 'website' | 'ssh' | 'rdp';
export type SshAuthMethod = 'password' | 'key';

export interface SiteConnectionConfig {
  host?: string;
  port?: number;
  username?: string;
  auth_method?: SshAuthMethod;
  identity_file?: string | null;
  redirect_clipboard?: boolean;
  redirect_drives?: boolean;
}

export interface Site {
  id: string;
  organization_id: string;
  owner_id?: string | null;
  visibility: SiteVisibility;
  category: string;
  title: string;
  resource_type: SiteResourceType;
  connection_config: SiteConnectionConfig;
  url: string;
  internal_url?: string | null;
  description?: string | null;
  icon_url?: string | null;
  icon_text?: string | null;
  auto_icon_url?: string | null;
  icon_fetched_at?: string | null;
  tags: string[];
  is_pinned: boolean;
  sort_order: number;
  open_count: number;
  last_opened_at?: string | null;
  department_ids: string[];
  created_by: string;
  created_at: string;
  updated_at: string;
}

export interface MarkdownDoc {
  id: string;
  project_id: string;
  folder_id?: string | null;
  title: string;
  body_markdown: string;
  tags: string[];
  sort_order: number;
  created_by: string;
  updated_at: string;
  can_write: boolean;
  has_custom_access: boolean;
}

export interface MarkdownFolder {
  id: string;
  project_id: string;
  parent_id?: string | null;
  name: string;
  sort_order: number;
  created_by: string;
  created_at: string;
  updated_at: string;
  can_write: boolean;
  has_custom_access: boolean;
}

export type MarkdownAccessRole = 'viewer' | 'editor';

export interface MarkdownDepartmentGrant {
  department_id: string;
  department_name: string;
  access_role: MarkdownAccessRole;
}

export interface VaultItem {
  id: string;
  project_id: string;
  title: string;
  kind: string;
  username?: string | null;
  url?: string | null;
  notes?: string | null;
  tags: string[];
  created_at: string;
}

export interface FileAsset {
  id: string;
  project_id: string;
  filename: string;
  stored_path: string;
  content_type?: string | null;
  size_bytes: number;
  uploaded_by: string;
  created_at: string;
}

export interface Ticket {
  id: string;
  project_id: string;
  title: string;
  description_markdown: string;
  status: TicketStatus;
  assignee_id?: string | null;
  created_by: string;
  updated_at: string;
}

export interface AuditEvent {
  id: string;
  organization_id: string;
  actor_id?: string | null;
  action: string;
  target_type: string;
  target_id?: string | null;
  metadata: Record<string, unknown>;
  created_at: string;
}

export interface ShareLink {
  id: string;
  organization_id: string;
  target_type: string;
  target_id: string;
  title: string;
  expires_at?: string | null;
  created_at: string;
}
