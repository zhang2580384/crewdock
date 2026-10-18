import type {
  Account,
  AccountManagement,
  AccountProjectGrant,
  AuthSettings,
  AuditEvent,
  Company,
  Department,
  EmailTemplate,
  EmailSettings,
  FinalizeSigningRequest,
  FileAsset,
  HealthInfo,
  MarkdownDoc,
  MarkdownDepartmentGrant,
  MarkdownFolder,
  Project,
  ProjectAssignableUser,
  ProjectDepartmentGrant,
  ProjectGrantCandidate,
  ProjectLink,
  ProjectListItem,
  ProjectRole,
  ProjectUserGrant,
  PutProjectDepartmentGrantRequest,
  PutProjectUserGrantRequest,
  RegistrationPolicy,
  RegistrationRequest,
  SealGeneratorDefinition,
  SealGeneratorSettings,
  SealGeneratorTemplate,
  SealImageOptions,
  Seal,
  SealAuthorizations,
  SigningDocument,
  SystemUpdateStatus,
  Ticket,
  TicketStatus,
  User,
  VaultItem,
  ShareLink,
  Site,
  SiteVisibility,
  SessionUser,
  UserSession,
  UpdateEmailSettings,
  VerificationAccepted,
} from './types';

const API_BASE = import.meta.env.VITE_API_BASE ?? '';

interface Envelope<T> {
  data: T;
}

export function getToken() {
  return localStorage.getItem('crewdock-token');
}

export function setToken(token: string) {
  localStorage.setItem('crewdock-token', token);
}

export function clearToken() {
  localStorage.removeItem('crewdock-token');
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers);
  if (!(init.body instanceof FormData)) {
    headers.set('Content-Type', 'application/json');
  }
  const token = getToken();
  if (token) {
    headers.set('Authorization', `Bearer ${token}`);
  }
  const res = await fetch(`${API_BASE}${path}`, { ...init, headers, cache: 'no-store' });
  if (!res.ok) {
    throw await responseError(res, path);
  }
  const json = (await res.json()) as Envelope<T>;
  return json.data;
}

async function responseError(res: Response, path: string) {
  const body = await res.text();
  let message = body;
  let errorCode = '';
  try {
    const parsed = JSON.parse(body) as { error?: string; code?: string; retryable?: boolean };
    message = parsed.error || body;
    errorCode = parsed.code || '';
  } catch {
    // Keep plain-text API errors unchanged.
  }

  if (res.status === 401 && path !== '/api/auth/login') {
    clearToken();
    if (window.location.pathname !== '/login') window.location.assign('/login');
  }

  const friendlyMessages: Record<string, string> = {
    unauthorized: '账号、密码或登录状态无效',
    forbidden: '当前账号没有执行此操作的权限',
    'not found': '请求的内容不存在',
    'bad request: current password is incorrect': '当前密码不正确',
    'bad request: password must be between 12 and 128 characters': '密码长度需要在 12 到 128 位之间',
    'bad request: application registration requires at least one allowed email domain':
      '开放员工申请时至少需要配置一个企业邮箱域名',
    'bad request: email is invalid': '邮箱地址格式不正确',
    'bad request: email domain is not allowed': '该邮箱域名不在允许申请的企业域名范围内',
    'bad request: verification code is invalid or expired': '邮箱验证码错误或已失效，请重新获取',
    'bad request: verification code must contain six digits': '请输入六位数字邮箱验证码',
    'bad request: activation token is invalid or expired': '账号激活链接无效或已过期，请联系管理员重新邀请',
    'bad request: token is invalid': '链接中的验证信息无效',
    'conflict: account already exists': '该邮箱已经激活为工作台账号，请直接登录',
    'conflict: email already exists': '该邮箱已被工作台账号使用',
    'conflict: email already has an active request': '该邮箱已有待处理申请或待激活邀请',
    'conflict: email already has an active registration or invitation':
      '该邮箱已有待处理申请或待激活邀请',
    'conflict: approved registration has no primary department':
      '审批信息缺少主要部门，请联系管理员重新处理',
    'conflict: approved departments are no longer available; contact an administrator':
      '审批时分配的部门已发生变化，请联系管理员重新处理',
    'bad request: at least one department and a matching primary department are required':
      '至少选择一个所属部门，并指定其中一个为主要部门',
    'bad request: one or more departments are invalid': '一个或多个部门已不存在或不属于当前组织',
    'bad request: primary department is required': '请选择主要部门',
    'bad request: primary department must be included in departments': '主要部门必须包含在所属部门中',
    'bad request: managed departments can only be assigned to active member accounts':
      '只有启用中的普通成员可以配置管辖部门',
    'bad request: cannot remove or deactivate the last system administrator':
      '必须保留至少一名启用中的系统管理员',
    'bad request: cannot remove your own system administrator role': '不能移除自己的系统管理员身份',
    'bad request: cannot deactivate your own account': '不能停用自己的账号',
    'conflict: assign another department administrator before removing the last one':
      '请先为该部门配置另一名管理员，再移除当前管理员',
    'conflict: assign another department administrator before deactivating or promoting this account':
      '请先为相关部门配置另一名管理员，再停用或调整此账号身份',
    'email delivery is unavailable': '邮件服务尚未就绪，请先检查系统邮件设置',
    'registration rate limiter is unavailable': '申请服务暂时不可用，请稍后重试',
    'login rate limiter is unavailable': '登录保护服务暂时不可用，请稍后重试',
    'login verifier is unavailable': '登录服务繁忙，请稍后重试',
    'bad request: CREWDOCK_MASTER_KEY must be configured before enabling email delivery or storing an SMTP password':
      '部署环境未配置 CREWDOCK_MASTER_KEY，无法保存 SMTP 密码或启用邮件',
    'bad request: smtp_password and clear_smtp_password cannot be used together':
      '不能同时填写 SMTP 密码并选择清除已保存密码',
    'bad request: smtp_port must be between 1 and 65535': 'SMTP 端口必须在 1 到 65535 之间',
    'bad request: unencrypted SMTP is limited to loopback relays':
      '不加密 SMTP 仅允许连接本机回环地址',
    'bad request: document content does not match its file extension': '文件内容与扩展名不一致，请选择原始文件',
    'conflict: signing document has already been finalized': '该文档已经完成签章，签章记录不能再次覆盖',
    'bad request: one or more seals are unavailable': '所选印章已被删除或授权已撤销，请重新选择',
    'bad request: one or more seal authorization users are unavailable': '部分员工已停用或不属于当前企业，请刷新后重试',
    'bad request: at most 100 seals are allowed': '企业印章最多可保存 100 枚，请先删除不再使用的印章',
    'bad request: only PDF, DOC, DOCX, PNG, JPEG, and WebP files are supported':
      '仅支持 PDF、Word、PNG、JPG 和 WebP 文件',
    'bad request: image must be PNG, JPEG, or WebP': '图片无法解析，请使用有效的 PNG、JPG 或 WebP 文件',
    'bad request: image dimensions must not exceed 8192 pixels or 40 megapixels':
      '图片边长不能超过 8192 像素，且总像素不能超过 4000 万',
    'bad request: smtp_password must be between 1 and 1024 bytes': 'SMTP 密码长度不正确',
    'bad request: enabled email delivery requires smtp_host, from_email, and smtp_password':
      '启用邮件前需要填写 SMTP 主机、发件邮箱和 SMTP 密码',
    'bad request: email delivery must be fully configured and enabled before testing':
      '请先完整配置并启用邮件发送，再发送测试邮件',
    'bad request: smtp_host is invalid': 'SMTP 主机格式不正确',
    'bad request: folder is not empty': '文件夹中还有子文件夹或文档，请先移动或删除其中内容',
    'bad request: folder destination is invalid': '目标文件夹无效或不属于当前项目',
    'bad request: folder cannot be moved into itself or a descendant': '文件夹不能移动到自身或自己的子文件夹中',
    'bad request: folder name is required': '文件夹名称不能为空',
    'bad request: department name is required': '部门名称不能为空',
    'bad request: department destination is invalid': '目标部门无效或不属于当前组织',
    'bad request: department cannot be moved into itself or a descendant': '部门不能移动到自身或其下级部门中',
    'bad request: department is not empty': '部门仍被子部门、用户、项目或权限配置引用',
    'bad request: project must be archived before deletion': '请先归档项目，再执行永久删除',
    'bad request: project must retain at least one direct administrator':
      '项目必须保留至少一名直接管理员',
    'bad request: cannot remove the last project administrator':
      '不能移除或降级最后一名项目管理员',
    'bad request: team sites require at least one department': '团队站点至少需要选择一个部门',
    'bad request: site department scope is invalid': '站点的部门范围无效，请重新选择',
    'bad request: site resource type is invalid': '入口类型无效，请重新选择',
    'bad request: ssh connection config is invalid': 'SSH 连接配置无效，请检查后重试',
    'bad request: rdp connection config is invalid': 'RDP 连接配置无效，请检查后重试',
    'bad request: ssh authentication method is invalid': 'SSH 认证方式无效',
    'bad request: ssh identity file is required': '使用密钥认证时需要填写本机密钥路径',
    'bad request: connection host is invalid': '服务器地址格式无效',
    'bad request: connection username is invalid': '登录用户名格式无效',
    'bad request: connection port is invalid': '服务器端口必须在 1 到 65535 之间',
    'bad request: markdown permissions only support viewer or editor': '文档权限只能设置为只读或可编辑',
    'bad request: markdown permission department scope is invalid': '文档的部门权限范围无效，请重新选择',
    'bad request: two-factor authentication is not available; contact an administrator':
      '此账号的双重验证状态异常，请联系管理员处理',
    'update service is unavailable': '主机更新服务尚未安装或未正确挂载',
    'bad request: update version must use vX.Y.Z': '更新版本号格式无效',
    'bad request: target update version must be newer than the running version': '目标版本必须高于当前运行版本',
    'conflict: target update is no longer the latest checked release': '可用版本已经变化，请重新检查更新',
  };
  if (errorCode === 'busy') return new Error('资源正在被其他操作占用，请稍后重试');
  if (errorCode === 'rate_limited') {
    const seconds = Number(res.headers.get('Retry-After'));
    if (path === '/api/auth/login' && seconds > 0) {
      return new Error('登录尝试过于频繁，请约 ' + Math.ceil(seconds / 60) + ' 分钟后重试');
    }
    return new Error('操作过于频繁，请稍后重试');
  }
  if (errorCode === 'conflict') {
    return new Error(friendlyMessages[message] || '数据状态已发生变化，请刷新后重试');
  }
  if (errorCode === 'unavailable') {
    return new Error(friendlyMessages[message] || '相关服务暂时不可用，请稍后重试');
  }
  return new Error(friendlyMessages[message] || message || `请求失败（${res.status}）`);
}

async function requestBlob(path: string): Promise<Blob> {
  const headers = new Headers();
  const token = getToken();
  if (token) headers.set('Authorization', `Bearer ${token}`);
  const res = await fetch(`${API_BASE}${path}`, { headers, cache: 'no-store' });
  if (!res.ok) throw await responseError(res, path);
  return res.blob();
}

export const api = {
  async health() {
    return request<HealthInfo>('/health');
  },
  async registrationPolicy() {
    return request<RegistrationPolicy>('/api/auth/registration-policy');
  },
  async register(payload: {
    email: string;
    display_name: string;
    department_interest?: string;
    reason?: string;
  }) {
    return request<VerificationAccepted>('/api/auth/register', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async verifyEmail(email: string, code: string) {
    return request<{ verified: boolean }>('/api/auth/verify-email', {
      method: 'POST',
      body: JSON.stringify({ email, code }),
    });
  },
  async resendVerification(email: string) {
    return request<VerificationAccepted>('/api/auth/resend-verification', {
      method: 'POST',
      body: JSON.stringify({ email }),
    });
  },
  async activate(token: string, password: string) {
    return request<{ activated: boolean; email: string }>('/api/auth/activate', {
      method: 'POST',
      body: JSON.stringify({ token, password }),
    });
  },
  async login(email: string, password: string) {
    return request<{ token: string; user: User; expires_at: string }>('/api/auth/login', {
      method: 'POST',
      body: JSON.stringify({ email, password }),
    });
  },
  async me() {
    return request<{ user: SessionUser }>('/api/auth/me');
  },
  async changePassword(current_password: string, new_password: string) {
    return request<{ changed: boolean }>('/api/auth/change-password', {
      method: 'POST',
      body: JSON.stringify({ current_password, new_password }),
    });
  },
  async logout() {
    return request<{ logged_out: boolean }>('/api/auth/logout', { method: 'POST' });
  },
  async sessions() {
    return request<UserSession[]>('/api/auth/sessions');
  },
  async revokeSession(id: string) {
    return request<{ revoked: boolean }>(`/api/auth/sessions/${id}`, { method: 'DELETE' });
  },
  async registrationRequests() {
    return request<RegistrationRequest[]>('/api/admin/registration-requests');
  },
  async approveRegistrationRequest(
    id: string,
    payload: { department_ids: string[]; primary_department_id: string },
  ) {
    return request<RegistrationRequest>(`/api/admin/registration-requests/${id}/approve`, {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async rejectRegistrationRequest(id: string, reason?: string) {
    return request<RegistrationRequest>(`/api/admin/registration-requests/${id}/reject`, {
      method: 'POST',
      body: JSON.stringify({ reason }),
    });
  },
  async inviteUser(payload: {
    email: string;
    display_name: string;
    department_ids: string[];
    primary_department_id: string;
  }) {
    return request<RegistrationRequest>('/api/admin/invitations', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async authSettings() {
    return request<AuthSettings>('/api/admin/auth-settings');
  },
  async updateAuthSettings(payload: AuthSettings) {
    return request<AuthSettings>('/api/admin/auth-settings', {
      method: 'PUT',
      body: JSON.stringify(payload),
    });
  },
  async emailSettings() {
    return request<EmailSettings>('/api/admin/email-settings');
  },
  async updateEmailSettings(payload: UpdateEmailSettings) {
    return request<EmailSettings>('/api/admin/email-settings', {
      method: 'PUT',
      body: JSON.stringify(payload),
    });
  },
  async testEmailSettings() {
    return request<{ queued: boolean; recipient: string }>('/api/admin/email-settings/test', {
      method: 'POST',
    });
  },
  async emailTemplates() {
    return request<EmailTemplate[]>('/api/admin/email-templates');
  },
  async updateEmailTemplate(key: EmailTemplate['key'], payload: { subject: string; html_body: string }) {
    return request<EmailTemplate>(`/api/admin/email-templates/${key}`, {
      method: 'PUT',
      body: JSON.stringify(payload),
    });
  },
  async updateStatus() {
    return request<SystemUpdateStatus>('/api/admin/updates');
  },
  async checkForUpdates() {
    return request<SystemUpdateStatus>('/api/admin/updates/check', { method: 'POST' });
  },
  async applyUpdate(targetVersion: string) {
    return request<SystemUpdateStatus>('/api/admin/updates/apply', {
      method: 'POST',
      body: JSON.stringify({ target_version: targetVersion }),
    });
  },
  async seals() {
    return request<Seal[]>('/api/seals');
  },
  async sealGeneratorTemplate() {
    return request<SealGeneratorTemplate>('/api/seal-generator-template');
  },
  async updateSealGeneratorTemplate(settings: SealGeneratorSettings) {
    return request<SealGeneratorTemplate>('/api/seal-generator-template', {
      method: 'PUT',
      body: JSON.stringify(settings),
    });
  },
  async createSeal(
    name: string,
    file: Blob,
    options: SealImageOptions,
    generatorDefinition?: SealGeneratorDefinition,
  ) {
    const body = new FormData();
    body.append('name', name);
    body.append('file', file, file instanceof File ? file.name : 'generated-seal.png');
    body.append('remove_background', String(options.remove_background));
    body.append('red_ink_only', String(options.red_ink_only));
    body.append('ink_texture', String(options.ink_texture));
    if (generatorDefinition) body.append('generator_definition', JSON.stringify(generatorDefinition));
    return request<Seal>('/api/seals', { method: 'POST', body });
  },
  async updateSeal(id: string, name: string) {
    return request<Seal>(`/api/seals/${id}`, {
      method: 'PATCH',
      body: JSON.stringify({ name }),
    });
  },
  async replaceSeal(
    id: string,
    name: string,
    file: Blob,
    options: SealImageOptions,
    generatorDefinition?: SealGeneratorDefinition,
  ) {
    const body = new FormData();
    body.append('name', name);
    body.append('file', file, file instanceof File ? file.name : 'generated-seal.png');
    body.append('remove_background', String(options.remove_background));
    body.append('red_ink_only', String(options.red_ink_only));
    body.append('ink_texture', String(options.ink_texture));
    if (generatorDefinition) body.append('generator_definition', JSON.stringify(generatorDefinition));
    return request<Seal>(`/api/seals/${id}`, { method: 'PUT', body });
  },
  async deleteSeal(id: string) {
    return request<{ deleted: boolean }>(`/api/seals/${id}`, { method: 'DELETE' });
  },
  async sealImage(id: string) {
    return requestBlob(`/api/seals/${id}/image`);
  },
  async sealAuthorizations(id: string) {
    return request<SealAuthorizations>(`/api/seals/${id}/authorizations`);
  },
  async updateSealAuthorizations(id: string, userIds: string[]) {
    return request<SealAuthorizations>(`/api/seals/${id}/authorizations`, {
      method: 'PUT',
      body: JSON.stringify({ user_ids: userIds }),
    });
  },
  async signingDocuments() {
    return request<SigningDocument[]>('/api/signing/documents');
  },
  async uploadSigningDocument(file: File) {
    const body = new FormData();
    body.append('file', file);
    return request<SigningDocument>('/api/signing/documents', { method: 'POST', body });
  },
  async deleteSigningDocument(id: string) {
    return request<{ deleted: boolean }>(`/api/signing/documents/${id}`, { method: 'DELETE' });
  },
  async signingDocumentSource(id: string) {
    return requestBlob(`/api/signing/documents/${id}/source`);
  },
  async finalizeSigningDocument(id: string, payload: FinalizeSigningRequest) {
    return request<SigningDocument>(`/api/signing/documents/${id}/finalize`, {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async signedDocument(id: string) {
    return requestBlob(`/api/signing/documents/${id}/download`);
  },
  async signedDocumentImages(id: string) {
    return requestBlob(`/api/signing/documents/${id}/images`);
  },
  async users() {
    return request<AccountManagement>('/api/users');
  },
  async company() {
    return request<Company>('/api/company');
  },
  async createUser(payload: {
    email: string;
    display_name: string;
    password: string;
    system_role?: User['system_role'];
    department_id?: string;
    department_ids?: string[];
    primary_department_id?: string;
  }) {
    return request<Account>('/api/users', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async updateUser(
    id: string,
    payload: Partial<{
      display_name: string;
      system_role: User['system_role'];
      department_id: string | null;
      department_ids: string[];
      primary_department_id: string | null;
      is_active: boolean;
    }>,
  ) {
    return request<Account>(`/api/users/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(payload),
    });
  },
  async resetUserPassword(id: string, password: string) {
    return request<Account>(`/api/users/${id}/reset-password`, {
      method: 'POST',
      body: JSON.stringify({ password }),
    });
  },
  async userManagedDepartments(id: string) {
    return request<{ department_ids: string[] }>(`/api/users/${id}/managed-departments`);
  },
  async updateUserManagedDepartments(id: string, department_ids: string[]) {
    return request<{ department_ids: string[] }>(`/api/users/${id}/managed-departments`, {
      method: 'PUT',
      body: JSON.stringify({ department_ids }),
    });
  },
  async userProjectGrants(id: string) {
    return request<AccountProjectGrant[]>(`/api/users/${id}/project-grants`);
  },
  async putUserProjectGrant(id: string, projectId: string, project_role: ProjectRole) {
    return request<ProjectUserGrant>(`/api/users/${id}/project-grants/${projectId}`, {
      method: 'PUT',
      body: JSON.stringify({ project_role }),
    });
  },
  async deleteUserProjectGrant(id: string, projectId: string) {
    return request<{ deleted: boolean }>(`/api/users/${id}/project-grants/${projectId}`, {
      method: 'DELETE',
    });
  },
  async departments() {
    return request<Department[]>('/api/departments');
  },
  async createDepartment(payload: { parent_id?: string | null; name: string }) {
    return request<Department>('/api/departments', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async updateDepartment(id: string, payload: { name: string }) {
    return request<Department>(`/api/departments/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(payload),
    });
  },
  async moveDepartment(id: string, payload: { parent_id?: string | null; position: number }) {
    return request<Department>(`/api/departments/${id}/move`, {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async deleteDepartment(id: string) {
    return request<{ deleted: boolean }>(`/api/departments/${id}`, { method: 'DELETE' });
  },
  async sites() {
    const sites = await request<Site[]>('/api/sites');
    return sites.map((site) => ({
      ...site,
      resource_type: site.resource_type || 'website',
      connection_config: site.connection_config || {},
      open_count: site.open_count || 0,
      last_opened_at: site.last_opened_at || null,
    }));
  },
  async recordSiteOpen(id: string) {
    return request<{ open_count: number; last_opened_at: string }>(`/api/sites/${id}/open`, {
      method: 'POST',
      keepalive: true,
    });
  },
  async createSite(payload: {
    visibility?: SiteVisibility;
    category?: string;
    title: string;
    url: string;
    resource_type?: import('./types').SiteResourceType;
    connection_config?: import('./types').SiteConnectionConfig;
    internal_url?: string;
    description?: string;
    icon_url?: string;
    icon_text?: string;
    tags?: string[];
    is_pinned?: boolean;
    department_ids?: string[];
  }) {
    return request<Site>('/api/sites', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async updateSite(
    id: string,
    payload: Partial<{
      visibility: SiteVisibility;
      category: string;
      title: string;
      url: string;
      resource_type: import('./types').SiteResourceType;
      connection_config: import('./types').SiteConnectionConfig;
      internal_url: string;
      description: string;
      icon_url: string;
      icon_text: string;
      tags: string[];
      is_pinned: boolean;
      department_ids: string[];
    }>,
  ) {
    return request<Site>(`/api/sites/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(payload),
    });
  },
  async deleteSite(id: string) {
    return request<{ deleted: boolean }>(`/api/sites/${id}`, { method: 'DELETE' });
  },
  async importBookmarks(payload: { html: string; visibility?: SiteVisibility; category?: string; department_ids?: string[] }) {
    return request<{ imported: number; skipped: number; sites: Site[] }>('/api/sites/import-bookmarks', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async projects(includeArchived = false) {
    return request<ProjectListItem[]>(`/api/projects${includeArchived ? '?include_archived=true' : ''}`);
  },
  async refreshSiteIcon(id: string) {
    return request<Site>(`/api/sites/${id}/icon/refresh`, { method: 'POST' });
  },
  async createProject(payload: {
    name: string;
    slug?: string;
    description?: string;
    department_id?: string;
  }) {
    return request<Project>('/api/projects', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async updateProject(
    id: string,
    payload: {
      name: string;
      slug?: string;
      description?: string;
      department_id?: string | null;
    },
  ) {
    return request<Project>(`/api/projects/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(payload),
    });
  },
  async archiveProject(id: string) {
    return request<Project>(`/api/projects/${id}/archive`, { method: 'POST' });
  },
  async restoreProject(id: string) {
    return request<Project>(`/api/projects/${id}/restore`, { method: 'POST' });
  },
  async deleteProject(id: string) {
    return request<{ deleted: boolean; files_cleanup_pending: boolean }>(`/api/projects/${id}`, {
      method: 'DELETE',
    });
  },
  async projectMembers(projectId: string) {
    return request<ProjectAssignableUser[]>(`/api/projects/${projectId}/members`);
  },
  async projectGrantCandidates(projectId: string) {
    return request<ProjectGrantCandidate[]>(`/api/projects/${projectId}/grant-candidates`);
  },
  async projectUserGrants(projectId: string) {
    return request<ProjectUserGrant[]>(`/api/projects/${projectId}/grants/users`);
  },
  async putProjectUserGrant(
    projectId: string,
    userId: string,
    payload: PutProjectUserGrantRequest,
  ) {
    return request<ProjectUserGrant>(`/api/projects/${projectId}/grants/users/${userId}`, {
      method: 'PUT',
      body: JSON.stringify(payload),
    });
  },
  async deleteProjectUserGrant(projectId: string, userId: string) {
    return request<{ deleted: boolean }>(`/api/projects/${projectId}/grants/users/${userId}`, {
      method: 'DELETE',
    });
  },
  async projectDepartmentGrants(projectId: string) {
    return request<ProjectDepartmentGrant[]>(`/api/projects/${projectId}/grants/departments`);
  },
  async putProjectDepartmentGrant(
    projectId: string,
    departmentId: string,
    payload: PutProjectDepartmentGrantRequest,
  ) {
    return request<ProjectDepartmentGrant>(
      `/api/projects/${projectId}/grants/departments/${departmentId}`,
      { method: 'PUT', body: JSON.stringify(payload) },
    );
  },
  async deleteProjectDepartmentGrant(projectId: string, departmentId: string) {
    return request<{ deleted: boolean }>(
      `/api/projects/${projectId}/grants/departments/${departmentId}`,
      { method: 'DELETE' },
    );
  },
  async links(projectId: string) {
    return request<ProjectLink[]>(`/api/projects/${projectId}/links`);
  },
  async createLink(projectId: string, payload: Omit<ProjectLink, 'id' | 'project_id' | 'created_at'>) {
    return request<ProjectLink>(`/api/projects/${projectId}/links`, {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async updateLink(
    projectId: string,
    id: string,
    payload: { title: string; url: string; description?: string; tags: string[] },
  ) {
    return request<ProjectLink>(`/api/projects/${projectId}/links/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(payload),
    });
  },
  async deleteLink(projectId: string, id: string) {
    return request<{ deleted: boolean }>(`/api/projects/${projectId}/links/${id}`, { method: 'DELETE' });
  },
  async docs(projectId: string) {
    return request<MarkdownDoc[]>(`/api/projects/${projectId}/docs`);
  },
  async createDoc(
    projectId: string,
    payload: { folder_id?: string; title: string; body_markdown: string; tags: string[] },
  ) {
    return request<MarkdownDoc>(`/api/projects/${projectId}/docs`, {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async updateDoc(
    projectId: string,
    id: string,
    payload: { title: string; body_markdown: string; tags: string[] },
  ) {
    return request<MarkdownDoc>(`/api/projects/${projectId}/docs/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(payload),
    });
  },
  async deleteDoc(projectId: string, id: string) {
    return request<{ deleted: boolean }>(`/api/projects/${projectId}/docs/${id}`, { method: 'DELETE' });
  },
  async moveDoc(projectId: string, id: string, payload: { folder_id?: string; position: number }) {
    return request<MarkdownDoc>(`/api/projects/${projectId}/docs/${id}/move`, {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async docPermissions(projectId: string, id: string) {
    return request<MarkdownDepartmentGrant[]>(`/api/projects/${projectId}/docs/${id}/permissions`);
  },
  async replaceDocPermissions(projectId: string, id: string, department_grants: Array<{ department_id: string; access_role: 'viewer' | 'editor' }>) {
    return request<MarkdownDepartmentGrant[]>(`/api/projects/${projectId}/docs/${id}/permissions`, {
      method: 'PUT',
      body: JSON.stringify({ department_grants }),
    });
  },
  async folders(projectId: string) {
    return request<MarkdownFolder[]>(`/api/projects/${projectId}/folders`);
  },
  async createFolder(projectId: string, payload: { parent_id?: string; name: string }) {
    return request<MarkdownFolder>(`/api/projects/${projectId}/folders`, {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async updateFolder(projectId: string, id: string, name: string) {
    return request<MarkdownFolder>(`/api/projects/${projectId}/folders/${id}`, {
      method: 'PATCH',
      body: JSON.stringify({ name }),
    });
  },
  async deleteFolder(projectId: string, id: string) {
    return request<{ deleted: boolean }>(`/api/projects/${projectId}/folders/${id}`, { method: 'DELETE' });
  },
  async moveFolder(projectId: string, id: string, payload: { parent_id?: string; position: number }) {
    return request<MarkdownFolder>(`/api/projects/${projectId}/folders/${id}/move`, {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async folderPermissions(projectId: string, id: string) {
    return request<MarkdownDepartmentGrant[]>(`/api/projects/${projectId}/folders/${id}/permissions`);
  },
  async replaceFolderPermissions(projectId: string, id: string, department_grants: Array<{ department_id: string; access_role: 'viewer' | 'editor' }>) {
    return request<MarkdownDepartmentGrant[]>(`/api/projects/${projectId}/folders/${id}/permissions`, {
      method: 'PUT',
      body: JSON.stringify({ department_grants }),
    });
  },
  async vault() {
    return request<VaultItem[]>('/api/vault');
  },
  async createVault(payload: {
    project_id: string;
    title: string;
    kind: string;
    username?: string;
    url?: string;
    secret: string;
    notes?: string;
    tags: string[];
  }) {
    return request<VaultItem>('/api/vault', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async updateVault(
    id: string,
    payload: {
      title: string;
      kind: string;
      username?: string;
      url?: string;
      secret?: string;
      notes?: string;
      tags: string[];
    },
  ) {
    return request<VaultItem>(`/api/vault/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(payload),
    });
  },
  async deleteVault(id: string) {
    return request<{ deleted: boolean }>(`/api/vault/${id}`, { method: 'DELETE' });
  },
  async revealVault(id: string) {
    return request<{ id: string; secret: string }>(`/api/vault/${id}/reveal`, { method: 'POST' });
  },
  async files(projectId: string) {
    return request<FileAsset[]>(`/api/projects/${projectId}/files`);
  },
  async uploadFile(projectId: string, file: File) {
    const body = new FormData();
    body.set('file', file);
    return request<FileAsset>(`/api/projects/${projectId}/files`, { method: 'POST', body });
  },
  async downloadFile(projectId: string, id: string) {
    return requestBlob(`/api/projects/${projectId}/files/${id}/download`);
  },
  async updateFile(projectId: string, id: string, filename: string) {
    return request<FileAsset>(`/api/projects/${projectId}/files/${id}`, {
      method: 'PATCH',
      body: JSON.stringify({ filename }),
    });
  },
  async deleteFile(projectId: string, id: string) {
    return request<{ deleted: boolean }>(`/api/projects/${projectId}/files/${id}`, { method: 'DELETE' });
  },
  async tickets() {
    return request<Ticket[]>('/api/tickets');
  },
  async createTicket(payload: {
    project_id: string;
    title: string;
    description_markdown: string;
    assignee_id?: string | null;
  }) {
    return request<Ticket>('/api/tickets', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async updateTicketStatus(id: string, status: TicketStatus) {
    return request<Ticket>(`/api/tickets/${id}/status`, {
      method: 'PATCH',
      body: JSON.stringify({ status }),
    });
  },
  async updateTicket(
    id: string,
    payload: {
      title: string;
      description_markdown: string;
      status: TicketStatus;
      assignee_id: string | null;
    },
  ) {
    return request<Ticket>(`/api/tickets/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(payload),
    });
  },
  async deleteTicket(id: string) {
    return request<{ deleted: boolean }>(`/api/tickets/${id}`, { method: 'DELETE' });
  },
  async auditEvents() {
    return request<AuditEvent[]>('/api/audit-events');
  },
  async shareLinks() {
    return request<ShareLink[]>('/api/share-links');
  },
  async createShareLink(payload: {
    target_type: string;
    target_id: string;
    title: string;
    expires_at?: string | null;
  }) {
    return request<{ share: ShareLink; url: string }>('/api/share-links', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  },
  async deleteShareLink(id: string) {
    return request<{ deleted: boolean }>(`/api/share-links/${id}`, { method: 'DELETE' });
  },
  async resolveShareLink(id: string, token: string) {
    return request<{ share: ShareLink; target: Record<string, unknown> }>(
      `/api/share-links/${id}/resolve?token=${encodeURIComponent(token)}`,
    );
  },
};
