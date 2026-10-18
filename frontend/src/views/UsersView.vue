<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref } from 'vue';
import { onBeforeRouteLeave } from 'vue-router';
import {
  Check,
  Clock3,
  KeyRound,
  MailPlus,
  Menu,
  Save,
  Search,
  Settings2,
  UserCheck,
  UserPlus,
  UserRound,
  XCircle,
  X,
} from '@lucide/vue';
import WorkbenchShell from '../components/WorkbenchShell.vue';
import { api } from '../api';
import { useSessionStore } from '../stores/session';
import type {
  Account,
  AccountManagement,
  AccountProjectGrant,
  Department,
  ProjectListItem,
  ProjectRole,
  RegistrationRequest,
  SystemRole,
} from '../types';

interface ProfileBaseline {
  userId: string;
  display_name: string;
  system_role: SystemRole;
  department_id: string;
  department_ids: string[];
}

interface ResetTarget {
  id: string;
  email: string;
  label: string;
}

const session = useSessionStore();
const management = ref<AccountManagement | null>(null);
const departments = ref<Department[]>([]);
const projects = ref<ProjectListItem[]>([]);
const selectedId = ref('');
const searchQuery = ref('');
const error = ref('');
const success = ref('');
const loading = ref(true);
const saving = ref(false);
const saveStage = ref('');
const grantsLoading = ref(false);
const grantsReady = ref(false);
const hydrating = ref(false);
const createOpen = ref(false);
const createSaving = ref(false);
const resetOpen = ref(false);
const resetSaving = ref(false);
const resetTarget = ref<ResetTarget | null>(null);
const accountDrawerOpen = ref(false);
const activeTab = ref<'members' | 'requests'>('members');
const registrationRequests = ref<RegistrationRequest[]>([]);
const selectedRequestId = ref('');
const requestDrawerOpen = ref(false);
const reviewSaving = ref(false);
const inviteOpen = ref(false);
const inviteSaving = ref(false);
const managedLoading = ref(false);
const managedSaving = ref(false);
const managedReady = ref(false);
const emailReady = ref(false);
const managedDepartmentIds = ref<string[]>([]);
const managedDepartmentBaseline = ref<string[]>([]);
const advancedOpen = ref(false);
const profileBaseline = ref<ProfileBaseline | null>(null);
const baselineVersion = ref(0);
const createDialogRef = ref<HTMLElement | null>(null);
const createEmailRef = ref<HTMLInputElement | null>(null);
const resetDialogRef = ref<HTMLElement | null>(null);
const inviteDialogRef = ref<HTMLElement | null>(null);
const resetPasswordRef = ref<HTMLInputElement | null>(null);
const inviteEmailRef = ref<HTMLInputElement | null>(null);
const dialogReturnFocus = ref<HTMLElement | null>(null);

let grantsRequestGeneration = 0;
let managedRequestGeneration = 0;
let initialProjectRoles = new Map<string, ProjectRole>();

const editForm = reactive({
  display_name: '',
  system_role: 'member' as SystemRole,
  department_id: '',
  department_ids: [] as string[],
});

const createForm = reactive({
  email: '',
  display_name: '',
  password: '',
  system_role: 'member' as SystemRole,
  department_id: '',
  department_ids: [] as string[],
});

const resetForm = reactive({ password: '' });
const reviewForm = reactive({ department_ids: [] as string[], primary_department_id: '', rejection_reason: '' });
const inviteForm = reactive({ email: '', display_name: '', department_ids: [] as string[], primary_department_id: '' });
const projectRoles = reactive<Record<string, ProjectRole | ''>>({});

const systemRoleOptions: Array<{ value: SystemRole; label: string }> = [
  { value: 'system_admin', label: '系统管理员' },
  { value: 'member', label: '成员' },
];

const projectRoleOptions: Array<{ value: ProjectRole; label: string }> = [
  { value: 'administrator', label: '管理员' },
  { value: 'editor', label: '编辑者' },
  { value: 'viewer', label: '查看者' },
];

const users = computed(() => management.value?.users ?? []);
const selectedUser = computed(() => users.value.find((user) => user.id === selectedId.value) ?? null);
const activeAccountCount = computed(() => users.value.filter((user) => user.is_active).length);
const isSystemAdmin = computed(() => session.user?.system_role === 'system_admin');
const selectedRequest = computed(() => registrationRequests.value.find((item) => item.id === selectedRequestId.value) ?? null);
const pendingRequestCount = computed(() => registrationRequests.value.length);
const managedEligible = computed(() => selectedUser.value?.system_role === 'member' && selectedUser.value.is_active);
const managedDirty = computed(() => managedReady.value && !sameDepartmentIds(managedDepartmentIds.value, managedDepartmentBaseline.value));
const advancedVisible = computed(() => activeAccountCount.value > 1 || advancedOpen.value);
const manageableDepartmentIds = computed(
  () => new Set(management.value?.capabilities.manageable_department_ids ?? []),
);
const assignableDepartments = computed(() =>
  departments.value.filter((department) => manageableDepartmentIds.value.has(department.id)),
);
const editableDepartments = computed(() =>
  departments.value.filter(
    (department) =>
      manageableDepartmentIds.value.has(department.id) || editForm.department_ids.includes(department.id),
  ),
);
const manageableProjects = computed(() =>
  projects.value.filter((project) => project.access_role === 'administrator'),
);
const filteredUsers = computed(() => {
  const query = searchQuery.value.trim().toLocaleLowerCase();
  if (!query) return users.value;
  return users.value.filter((user) =>
    `${user.display_name} ${user.email}`.toLocaleLowerCase().includes(query),
  );
});
const detailDirty = computed(() => {
  baselineVersion.value;
  if (hydrating.value || !selectedUser.value || !profileBaseline.value) return false;
  const user = selectedUser.value;
  const baseline = profileBaseline.value;
  if (baseline.userId !== user.id) return false;
  if (user.capabilities.can_edit_profile && editForm.display_name !== baseline.display_name) {
    return true;
  }
  if (
    user.capabilities.can_change_system_role &&
    editForm.system_role !== baseline.system_role
  ) {
    return true;
  }
  if (
    user.capabilities.can_change_department &&
    (editForm.department_id !== baseline.department_id ||
      !sameDepartmentIds(editForm.department_ids, baseline.department_ids))
  ) {
    return true;
  }
  if (!grantsReady.value) return false;
  return manageableProjects.value.some(
    (project) =>
      (projectRoles[project.id] ?? '') !== (initialProjectRoles.get(project.id) ?? ''),
  );
});
const isDirty = computed(() => detailDirty.value || managedDirty.value);

function setMessage(nextError = '', nextSuccess = '') {
  error.value = nextError;
  success.value = nextSuccess;
}

function clearProjectRoles() {
  Object.keys(projectRoles).forEach((projectId) => delete projectRoles[projectId]);
  initialProjectRoles = new Map();
  baselineVersion.value += 1;
}

function setProfileBaseline(user: Account) {
  hydrating.value = true;
  editForm.display_name = user.display_name;
  editForm.system_role = user.system_role;
  editForm.department_id = user.department_id ?? '';
  editForm.department_ids = [...user.department_ids];
  profileBaseline.value = {
    userId: user.id,
    display_name: user.display_name,
    system_role: user.system_role,
    department_id: user.department_id ?? '',
    department_ids: [...user.department_ids],
  };
  baselineVersion.value += 1;
  hydrating.value = false;
}

function invalidateEditor() {
  grantsRequestGeneration += 1;
  managedRequestGeneration += 1;
  hydrating.value = true;
  selectedId.value = '';
  profileBaseline.value = null;
  grantsReady.value = false;
  grantsLoading.value = false;
  managedLoading.value = false;
  managedReady.value = false;
  clearProjectRoles();
  hydrating.value = false;
}

async function loadAccounts() {
  management.value = await api.users();
}

async function load() {
  loading.value = true;
  setMessage();
  try {
    await session.loadMe();
    const [accountData, departmentRows, projectRows, requestRows, registrationPolicy] = await Promise.all([
      api.users(),
      api.departments(),
      api.projects(),
      isSystemAdmin.value ? api.registrationRequests() : Promise.resolve([]),
      isSystemAdmin.value ? api.registrationPolicy() : Promise.resolve(null),
    ]);
    management.value = accountData;
    departments.value = departmentRows;
    projects.value = projectRows;
    registrationRequests.value = requestRows;
    emailReady.value = registrationPolicy?.email_ready ?? false;
    if (requestRows[0]) selectRequest(requestRows[0]);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '加载账号失败');
  } finally {
    loading.value = false;
  }
}

async function loadManagedDepartments(userId: string) {
  if (!isSystemAdmin.value) {
    managedReady.value = true;
    return true;
  }
  const generation = ++managedRequestGeneration;
  managedLoading.value = true;
  managedReady.value = false;
  try {
    const result = await api.userManagedDepartments(userId);
    if (generation !== managedRequestGeneration || selectedId.value !== userId) return false;
    managedDepartmentIds.value = [...result.department_ids];
    managedDepartmentBaseline.value = [...result.department_ids];
    managedReady.value = true;
    return true;
  } catch (err) {
    if (generation === managedRequestGeneration && selectedId.value === userId) {
      setMessage(err instanceof Error ? err.message : '加载管辖部门失败');
      managedReady.value = false;
    }
    return false;
  } finally {
    if (generation === managedRequestGeneration && selectedId.value === userId) {
      managedLoading.value = false;
    }
  }
}

async function loadRegistrationRequests() {
  if (!isSystemAdmin.value) return;
  registrationRequests.value = await api.registrationRequests();
  if (!registrationRequests.value.some((item) => item.id === selectedRequestId.value)) {
    selectedRequestId.value = registrationRequests.value[0]?.id || '';
  }
  const selected = registrationRequests.value.find((item) => item.id === selectedRequestId.value);
  if (selected) selectRequest(selected);
}

function applyProjectGrantBaseline(grants: AccountProjectGrant[]) {
  hydrating.value = true;
  clearProjectRoles();
  manageableProjects.value.forEach((project) => {
    projectRoles[project.id] = '';
  });
  grants.forEach((grant) => {
    if (grant.project_id in projectRoles) {
      projectRoles[grant.project_id] = grant.project_role;
      initialProjectRoles.set(grant.project_id, grant.project_role);
    }
  });
  grantsReady.value = true;
  baselineVersion.value += 1;
  hydrating.value = false;
}

async function loadProjectGrants(userId: string) {
  const generation = ++grantsRequestGeneration;
  grantsLoading.value = true;
  grantsReady.value = false;
  try {
    const grants = await api.userProjectGrants(userId);
    if (generation !== grantsRequestGeneration || selectedId.value !== userId) return false;
    applyProjectGrantBaseline(grants);
    return true;
  } catch (err) {
    if (generation === grantsRequestGeneration && selectedId.value === userId) {
      setMessage(err instanceof Error ? err.message : '加载项目授权失败');
    }
    return false;
  } finally {
    if (generation === grantsRequestGeneration && selectedId.value === userId) {
      grantsLoading.value = false;
    }
  }
}

async function forceSelectUser(userId: string) {
  const user = users.value.find((candidate) => candidate.id === userId);
  if (!user) {
    invalidateEditor();
    return false;
  }
  selectedId.value = user.id;
  accountDrawerOpen.value = false;
  advancedOpen.value = activeAccountCount.value > 1;
  setProfileBaseline(user);
  clearProjectRoles();
  managedRequestGeneration += 1;
  managedDepartmentIds.value = [];
  managedDepartmentBaseline.value = [];
  managedReady.value = false;
  const [grantsLoaded] = await Promise.all([
    loadProjectGrants(user.id),
    loadManagedDepartments(user.id),
  ]);
  return grantsLoaded;
}

async function refreshSelectedFromServer(userId: string) {
  grantsRequestGeneration += 1;
  grantsReady.value = false;
  await loadAccounts();
  return forceSelectUser(userId);
}

function confirmDiscardChanges() {
  return !isDirty.value || window.confirm('当前账号有未保存修改，确认放弃这些修改？');
}

async function selectUser(user: Account) {
  if (saving.value || grantsLoading.value || user.id === selectedId.value) return;
  if (!confirmDiscardChanges()) return;
  setMessage();
  await forceSelectUser(user.id);
}

function openAccountDrawer() {
  if (saving.value || grantsLoading.value) return;
  accountDrawerOpen.value = true;
}

function closeAccountDrawer() {
  if (saving.value || grantsLoading.value) return;
  accountDrawerOpen.value = false;
}

async function openCreate() {
  dialogReturnFocus.value = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  createForm.email = '';
  createForm.display_name = '';
  createForm.password = '';
  createForm.system_role = 'member';
  createForm.department_id = '';
  createForm.department_ids = [];
  createOpen.value = true;
  setMessage();
  await nextTick();
  createEmailRef.value?.focus();
}

async function closeCreate() {
  if (createSaving.value) return;
  createOpen.value = false;
  await nextTick();
  dialogReturnFocus.value?.focus();
  dialogReturnFocus.value = null;
}

async function createUser() {
  if (createForm.password.length < 12 || createForm.password.length > 128) {
    setMessage('初始密码长度需要在 12 到 128 位之间');
    return;
  }
  createSaving.value = true;
  setMessage();
  try {
    const user = await api.createUser({
      email: createForm.email,
      display_name: createForm.display_name,
      password: createForm.password,
      system_role: management.value?.capabilities.can_assign_system_role
        ? createForm.system_role
        : 'member',
      department_id: createForm.department_id || undefined,
      department_ids: [...createForm.department_ids],
      primary_department_id: createForm.department_id || undefined,
    });
    createOpen.value = false;
    await loadAccounts();
    await forceSelectUser(user.id);
    setMessage('', `已创建账号 ${user.email}`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '创建账号失败');
  } finally {
    createSaving.value = false;
  }
}

async function saveProjectChanges(
  userId: string,
  isActive: boolean,
  nextRoles: Map<string, ProjectRole | ''>,
  previousRoles: Map<string, ProjectRole>,
) {
  for (const project of manageableProjects.value) {
    const previous = previousRoles.get(project.id) ?? '';
    const next = nextRoles.get(project.id) ?? '';
    if (previous === next) continue;
    if (next) {
      if (!isActive) throw new Error('停用账号不能新增或修改项目授权');
      await api.putUserProjectGrant(userId, project.id, next);
    } else {
      await api.deleteUserProjectGrant(userId, project.id);
    }
  }
}

async function recoverAfterSaveFailure(userId: string) {
  try {
    const refreshed = await refreshSelectedFromServer(userId);
    if (!refreshed) invalidateEditor();
    return refreshed;
  } catch {
    invalidateEditor();
    return false;
  }
}

async function saveDetail() {
  const user = selectedUser.value;
  if (!user || saving.value || grantsLoading.value || !grantsReady.value) return;
  const userId = user.id;
  const userEmail = user.email;
  const userIsActive = user.is_active;
  const capabilities = { ...user.capabilities };
  const profilePayload = {
    display_name: editForm.display_name,
    system_role: editForm.system_role,
    department_id: editForm.department_id,
    department_ids: [...editForm.department_ids],
  };
  const nextRoles = new Map<string, ProjectRole | ''>(
    manageableProjects.value.map((project) => [project.id, projectRoles[project.id] ?? '']),
  );
  const previousRoles = new Map(initialProjectRoles);

  saving.value = true;
  saveStage.value = '账号资料';
  setMessage();
  try {
    if (capabilities.can_edit_profile) {
      await api.updateUser(userId, {
        display_name: profilePayload.display_name,
        ...(capabilities.can_change_system_role
          ? { system_role: profilePayload.system_role }
          : {}),
        ...(capabilities.can_change_department
          ? {
              department_id: profilePayload.department_id || null,
              primary_department_id: profilePayload.department_id || null,
              department_ids: profilePayload.department_ids,
            }
          : {}),
      });
    }
    saveStage.value = '项目授权';
    await saveProjectChanges(userId, userIsActive, nextRoles, previousRoles);
    saveStage.value = '刷新';
    if (!(await refreshSelectedFromServer(userId))) {
      throw new Error('刷新项目授权失败');
    }
    setMessage('', `已保存 ${userEmail}`);
  } catch (err) {
    const failedStage = saveStage.value;
    const detail = err instanceof Error ? err.message : '保存账号失败';
    const refreshed = await recoverAfterSaveFailure(userId);
    setMessage(
      refreshed
        ? `保存失败（${failedStage}）：${detail}。部分操作可能已保存，已刷新`
        : `保存失败（${failedStage}）：${detail}。自动刷新失败，编辑状态已清除`,
    );
  } finally {
    saveStage.value = '';
    saving.value = false;
  }
}

async function openReset() {
  const user = selectedUser.value;
  if (!user) return;
  dialogReturnFocus.value = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  resetTarget.value = { id: user.id, email: user.email, label: user.display_name };
  resetForm.password = '';
  resetOpen.value = true;
  setMessage();
  await nextTick();
  resetPasswordRef.value?.focus();
}

async function closeReset() {
  if (!resetSaving.value) {
    resetOpen.value = false;
    resetTarget.value = null;
    resetForm.password = '';
    await nextTick();
    dialogReturnFocus.value?.focus();
    dialogReturnFocus.value = null;
  }
}

async function resetPassword() {
  const target = resetTarget.value;
  if (!target) return;
  if (resetForm.password.length < 12 || resetForm.password.length > 128) {
    setMessage('新密码长度需要在 12 到 128 位之间');
    return;
  }
  resetSaving.value = true;
  setMessage();
  try {
    await api.resetUserPassword(target.id, resetForm.password);
    resetOpen.value = false;
    resetTarget.value = null;
    resetForm.password = '';
    setMessage('', `已重置 ${target.email} 的密码并撤销现有会话`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '重置密码失败');
  } finally {
    resetSaving.value = false;
  }
}

function selectRequest(request: RegistrationRequest) {
  selectedRequestId.value = request.id;
  reviewForm.department_ids = [...request.approved_department_ids];
  reviewForm.primary_department_id = request.primary_department_id || reviewForm.department_ids[0] || '';
  reviewForm.rejection_reason = '';
  requestDrawerOpen.value = false;
}

function ensureReviewPrimaryDepartment() {
  if (!reviewForm.department_ids.includes(reviewForm.primary_department_id)) {
    reviewForm.primary_department_id = reviewForm.department_ids[0] || '';
  }
}

async function approveRequest() {
  const request = selectedRequest.value;
  if (!request || !reviewForm.department_ids.length || !reviewForm.primary_department_id) {
    setMessage('通过申请前需要分配所属部门和主要部门');
    return;
  }
  reviewSaving.value = true;
  setMessage();
  try {
    await api.approveRegistrationRequest(request.id, {
      department_ids: [...reviewForm.department_ids],
      primary_department_id: reviewForm.primary_department_id,
    });
    await loadRegistrationRequests();
    setMessage('', `已通过 ${request.email} 的申请，激活邮件已加入发送队列`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '审批申请失败');
  } finally {
    reviewSaving.value = false;
  }
}

async function rejectRequest() {
  const request = selectedRequest.value;
  if (!request || !window.confirm(`拒绝 ${request.email} 的加入申请？`)) return;
  reviewSaving.value = true;
  setMessage();
  try {
    await api.rejectRegistrationRequest(request.id, reviewForm.rejection_reason.trim() || undefined);
    await loadRegistrationRequests();
    setMessage('', `已拒绝 ${request.email} 的申请`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '拒绝申请失败');
  } finally {
    reviewSaving.value = false;
  }
}

async function openInvite() {
  dialogReturnFocus.value = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  inviteForm.email = '';
  inviteForm.display_name = '';
  inviteForm.department_ids = [];
  inviteForm.primary_department_id = '';
  inviteOpen.value = true;
  setMessage();
  await nextTick();
  inviteEmailRef.value?.focus();
}

async function closeInvite() {
  if (inviteSaving.value) return;
  inviteOpen.value = false;
  await nextTick();
  dialogReturnFocus.value?.focus();
  dialogReturnFocus.value = null;
}

function ensureInvitePrimaryDepartment() {
  if (!inviteForm.department_ids.includes(inviteForm.primary_department_id)) {
    inviteForm.primary_department_id = inviteForm.department_ids[0] || '';
  }
}

async function inviteUser() {
  if (!inviteForm.department_ids.length || !inviteForm.primary_department_id) {
    setMessage('邀请成员前需要分配所属部门和主要部门');
    return;
  }
  inviteSaving.value = true;
  setMessage();
  try {
    const invitation = await api.inviteUser({
      email: inviteForm.email.trim(),
      display_name: inviteForm.display_name.trim(),
      department_ids: [...inviteForm.department_ids],
      primary_department_id: inviteForm.primary_department_id,
    });
    inviteOpen.value = false;
    await nextTick();
    dialogReturnFocus.value?.focus();
    dialogReturnFocus.value = null;
    setMessage('', `${invitation.email} 的激活邮件已加入发送队列`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '邀请成员失败');
  } finally {
    inviteSaving.value = false;
  }
}

async function saveManagedDepartments() {
  const user = selectedUser.value;
  if (!user || !managedEligible.value || !managedReady.value) return;
  managedSaving.value = true;
  setMessage();
  try {
    const result = await api.updateUserManagedDepartments(user.id, [...managedDepartmentIds.value]);
    managedDepartmentIds.value = [...result.department_ids];
    managedDepartmentBaseline.value = [...result.department_ids];
    managedReady.value = true;
    setMessage('', `已保存 ${user.display_name} 的管辖部门`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '保存管辖部门失败');
  } finally {
    managedSaving.value = false;
  }
}

async function toggleActive() {
  const user = selectedUser.value;
  if (!user?.capabilities.can_change_active || saving.value || grantsLoading.value) return;
  if (isDirty.value) {
    setMessage('请先保存或放弃当前修改，再更改启用状态');
    return;
  }
  const userId = user.id;
  const nextActive = !user.is_active;
  if (!nextActive && !window.confirm(`停用 ${user.email} 并撤销其全部会话？`)) return;
  setMessage();
  try {
    await api.updateUser(userId, { is_active: nextActive });
    await refreshSelectedFromServer(userId);
    setMessage('', nextActive ? `已启用 ${user.email}` : `已停用 ${user.email}`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '更新账号状态失败');
  }
}

function trapDialogFocus(event: KeyboardEvent, dialog: HTMLElement | null) {
  if (event.key !== 'Tab' || !dialog) return;
  const focusable = Array.from(
    dialog.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    ),
  ).filter((element) => !element.hidden);
  if (focusable.length === 0) return;
  const first = focusable[0];
  const last = focusable[focusable.length - 1];
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}

function handleCreateDialogKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault();
    closeCreate();
    return;
  }
  trapDialogFocus(event, createDialogRef.value);
}

function handleResetDialogKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault();
    closeReset();
    return;
  }
  trapDialogFocus(event, resetDialogRef.value);
}

function handleInviteDialogKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault();
    closeInvite();
    return;
  }
  trapDialogFocus(event, inviteDialogRef.value);
}

function handleBeforeUnload(event: BeforeUnloadEvent) {
  if (!isDirty.value) return;
  event.preventDefault();
  event.returnValue = '';
}

function systemRoleLabel(role: SystemRole) {
  return systemRoleOptions.find((option) => option.value === role)?.label ?? role;
}

function sameDepartmentIds(left: string[], right: string[]) {
  if (left.length !== right.length) return false;
  const rightIds = new Set(right);
  return left.every((departmentId) => rightIds.has(departmentId));
}

function ensureEditPrimaryDepartment() {
  if (!editForm.department_ids.includes(editForm.department_id)) {
    editForm.department_id = editForm.department_ids[0] ?? '';
  }
}

function ensureCreatePrimaryDepartment() {
  if (!createForm.department_ids.includes(createForm.department_id)) {
    createForm.department_id = createForm.department_ids[0] ?? '';
  }
}

function departmentSelectionLabel(departmentIds: string[]) {
  if (!departmentIds.length) return '未分配部门';
  return departmentIds.map((departmentId) => departmentLabel(departmentId)).join('、');
}

function departmentLabel(departmentId: string | null) {
  if (!departmentId) return '未分配部门';
  return departments.value.find((department) => department.id === departmentId)?.name ?? '未知部门';
}

onBeforeRouteLeave(() => {
  if (saving.value) return false;
  return confirmDiscardChanges();
});

onMounted(() => {
  window.addEventListener('beforeunload', handleBeforeUnload);
  load();
});

onBeforeUnmount(() => {
  grantsRequestGeneration += 1;
  managedRequestGeneration += 1;
  window.removeEventListener('beforeunload', handleBeforeUnload);
});
</script>

<template>
  <WorkbenchShell layout="workspace">
    <div class="page-head accounts-page-head">
      <div><h1>账号管理</h1></div>
      <div class="page-actions account-page-actions">
        <button v-if="management?.capabilities.can_create_users" class="secondary-button" type="button" title="直接创建账号" @click="openCreate"><UserPlus :size="17" /><span>直接创建</span></button>
        <button v-if="isSystemAdmin" class="primary-button" type="button" :disabled="!emailReady" :title="emailReady ? '邀请成员' : '请先在系统设置中配置并启用邮件'" @click="openInvite"><MailPlus :size="17" /><span>邀请成员</span></button>
      </div>
    </div>

    <nav v-if="isSystemAdmin" class="accounts-mode-tabs" aria-label="账号管理视图">
      <button type="button" :class="{ active: activeTab === 'members' }" @click="activeTab = 'members'"><UserRound :size="16" />成员</button>
      <button type="button" :class="{ active: activeTab === 'requests' }" @click="activeTab = 'requests'"><Clock3 :size="16" />待审核<span v-if="pendingRequestCount" class="accounts-tab-badge">{{ pendingRequestCount }}</span></button>
    </nav>

    <p v-if="error" class="error accounts-message">{{ error }}</p>
    <p v-if="success" class="success accounts-message">{{ success }}</p>

    <section
      v-if="management && activeTab === 'members'"
      class="accounts-workspace"
      :class="{ 'account-list-open': accountDrawerOpen }"
    >
      <button
        class="account-drawer-backdrop"
        type="button"
        aria-label="关闭账号列表"
        @click="closeAccountDrawer"
      />

      <aside id="account-list-panel" class="account-list-rail">
        <div class="account-list-head">
          <label class="account-search">
            <Search :size="16" />
            <input v-model="searchQuery" type="search" placeholder="搜索账号" />
          </label>
          <button
            class="icon-button account-list-close"
            type="button"
            title="关闭账号列表"
            @click="closeAccountDrawer"
          >
            <X :size="17" />
          </button>
        </div>
        <div class="account-list-scroll">
          <button
            v-for="user in filteredUsers"
            :key="user.id"
            class="account-list-item"
            :class="{ active: user.id === selectedId }"
            type="button"
            :disabled="saving || grantsLoading"
            @click="selectUser(user)"
          >
            <span class="account-avatar">{{ user.display_name.slice(0, 1).toUpperCase() }}</span>
            <span class="account-list-copy">
              <strong>{{ user.display_name }}</strong>
              <small>{{ user.email }}</small>
            </span>
            <span class="status-dot" :class="{ inactive: !user.is_active }" />
          </button>
        </div>
        <div class="account-list-count">{{ filteredUsers.length }} / {{ users.length }}</div>
      </aside>

      <main v-if="selectedUser" class="account-detail-surface">
        <header class="account-detail-head">
          <button
            class="icon-button account-list-toggle"
            type="button"
            title="打开账号列表"
            aria-controls="account-list-panel"
            :aria-expanded="accountDrawerOpen"
            @click="openAccountDrawer"
          >
            <Menu :size="18" />
          </button>
          <div class="account-detail-identity">
            <strong>{{ selectedUser.display_name }}</strong>
            <span>{{ selectedUser.email }}</span>
          </div>
          <span class="account-status" :class="{ inactive: !selectedUser.is_active }">
            {{ selectedUser.is_active ? '已启用' : '已停用' }}
          </span>
          <button
            v-if="activeAccountCount <= 1"
            class="icon-button"
            type="button"
            :title="advancedVisible ? '隐藏部门与项目' : '显示部门与项目'"
            @click="advancedOpen = !advancedOpen"
          >
            <Settings2 :size="17" />
          </button>
        </header>

        <div class="account-detail-scroll">
          <section class="account-detail-section">
            <div class="account-section-head"><h2>账号资料</h2></div>
            <div class="account-fields-grid">
              <label class="field">
                <span>显示名</span>
                <input
                  v-model="editForm.display_name"
                  :disabled="!selectedUser.capabilities.can_edit_profile || saving"
                />
              </label>
              <label class="field">
                <span>邮箱</span>
                <input :value="selectedUser.email" disabled />
              </label>
              <label v-if="selectedUser.capabilities.can_change_system_role" class="field">
                <span>系统角色</span>
                <select v-model="editForm.system_role" :disabled="saving">
                  <option v-for="option in systemRoleOptions" :key="option.value" :value="option.value">
                    {{ option.label }}
                  </option>
                </select>
              </label>
              <div v-else class="account-readonly-field">
                <span>系统角色</span>
                <strong>{{ systemRoleLabel(selectedUser.system_role) }}</strong>
              </div>
              <div class="account-readonly-field">
                <span>启用状态</span>
                <strong>{{ selectedUser.is_active ? '已启用' : '已停用' }}</strong>
              </div>
            </div>
          </section>

          <section v-if="advancedVisible" class="account-detail-section">
            <div class="account-section-head"><h2>所属部门</h2><span>{{ selectedUser.department_ids.length }}</span></div>
            <div v-if="selectedUser.capabilities.can_change_department" class="account-department-editor">
              <div class="field">
                <span>部门成员关系</span>
                <div class="account-department-list">
                  <label v-for="department in editableDepartments" :key="department.id" class="account-department-option">
                    <input
                      v-model="editForm.department_ids"
                      type="checkbox"
                      :value="department.id"
                      :disabled="saving || !manageableDepartmentIds.has(department.id)"
                      @change="ensureEditPrimaryDepartment"
                    />
                    <span>{{ department.name }}</span>
                  </label>
                </div>
              </div>
              <label class="field account-department-field">
                <span>主要部门</span>
                <select
                  v-model="editForm.department_id"
                  :disabled="saving || !editForm.department_ids.length"
                  :required="editForm.department_ids.length > 0"
                >
                  <option v-if="!editForm.department_ids.length" value="">未分配部门</option>
                  <option
                    v-for="department in departments.filter((row) => editForm.department_ids.includes(row.id))"
                    :key="department.id"
                    :value="department.id"
                  >
                    {{ department.name }}
                  </option>
                </select>
              </label>
            </div>
            <div v-else class="account-readonly-field account-department-field">
              <span>所属部门</span>
              <strong>{{ departmentSelectionLabel(selectedUser.department_ids) }}</strong>
            </div>
          </section>

          <section v-if="isSystemAdmin && advancedVisible" class="account-detail-section managed-department-section">
            <div class="account-section-head"><h2>管辖部门</h2><span>{{ managedDepartmentIds.length }}</span></div>
            <div v-if="managedLoading" class="account-inline-state">正在加载</div>
            <template v-else-if="managedEligible && managedReady">
              <div class="account-department-list">
                <label v-for="department in departments" :key="department.id" class="account-department-option">
                  <input v-model="managedDepartmentIds" type="checkbox" :value="department.id" :disabled="managedSaving" />
                  <span>{{ department.name }}</span>
                </label>
              </div>
              <div class="managed-department-actions">
                <span>选中的部门作为管辖范围根节点。</span>
                <button class="secondary-button compact-action" type="button" :disabled="managedSaving || !managedReady || !managedDirty" @click="saveManagedDepartments"><Save :size="16" />{{ managedSaving ? '保存中...' : '保存管辖部门' }}</button>
              </div>
            </template>
            <div v-else-if="managedEligible" class="account-inline-state">管辖部门未就绪，请重新选择账号后再试</div>
            <div v-else class="account-inline-state">仅启用中的普通成员可以被设为部门管理员</div>
          </section>

          <section v-if="advancedVisible" class="account-detail-section account-project-section">
            <div class="account-section-head">
              <h2>项目直接授权</h2>
              <span>{{ manageableProjects.length }}</span>
            </div>
            <div v-if="grantsLoading" class="account-inline-state">正在加载</div>
            <div v-else class="account-project-list">
              <label v-for="project in manageableProjects" :key="project.id" class="account-project-row">
                <span>
                  <strong>{{ project.name }}</strong>
                  <small>{{ project.slug }}</small>
                </span>
                <select v-model="projectRoles[project.id]" :disabled="saving || !grantsReady">
                  <option value="">不直接授权</option>
                  <option
                    v-for="option in projectRoleOptions"
                    :key="option.value"
                    :value="option.value"
                    :disabled="!selectedUser.is_active"
                  >
                    {{ option.label }}
                  </option>
                </select>
              </label>
            </div>
          </section>
        </div>

        <footer class="account-detail-actions">
          <button
            class="primary-button"
            type="button"
            :disabled="saving || grantsLoading || !grantsReady || managedDirty || !detailDirty"
            :title="managedDirty ? '请先保存管辖部门' : '保存账号资料与项目授权'"
            @click="saveDetail"
          >
            <Save :size="17" />
            <span>{{ saving ? `保存中：${saveStage}` : '保存' }}</span>
          </button>
          <button
            v-if="selectedUser.capabilities.can_reset_password"
            class="secondary-button"
            type="button"
            :disabled="saving || grantsLoading"
            @click="openReset"
          >
            <KeyRound :size="17" />
            <span>重置密码</span>
          </button>
          <span v-if="detailDirty" class="account-dirty-state">未保存</span>
          <button
            v-if="selectedUser.capabilities.can_change_active"
            class="secondary-button account-state-button"
            type="button"
            :disabled="saving || grantsLoading"
            @click="toggleActive"
          >
            {{ selectedUser.is_active ? '停用账号' : '启用账号' }}
          </button>
        </footer>
      </main>

      <div v-else class="account-detail-empty">
        <button
          class="icon-button account-list-toggle"
          type="button"
          title="打开账号列表"
          aria-controls="account-list-panel"
          :aria-expanded="accountDrawerOpen"
          @click="openAccountDrawer"
        >
          <Menu :size="18" />
        </button>
        <div class="workspace-selection-empty">
          <UserRound :size="24" />
          <strong>选择一个账号</strong>
          <span>查看账号资料、状态和项目权限</span>
        </div>
      </div>
    </section>

    <section v-else-if="management && activeTab === 'requests'" class="accounts-workspace review-workspace" :class="{ 'account-list-open': requestDrawerOpen }">
      <button class="account-drawer-backdrop" type="button" aria-label="关闭申请列表" @click="requestDrawerOpen = false" />
      <aside id="request-list-panel" class="account-list-rail">
        <div class="account-list-head review-list-head"><strong>待审核申请</strong><button class="icon-button account-list-close" type="button" title="关闭申请列表" @click="requestDrawerOpen = false"><X :size="17" /></button></div>
        <div class="account-list-scroll">
          <button v-for="request in registrationRequests" :key="request.id" class="account-list-item review-list-item" :class="{ active: request.id === selectedRequestId }" type="button" @click="selectRequest(request)">
            <span class="account-avatar"><UserCheck :size="16" /></span>
            <span class="account-list-copy"><strong>{{ request.display_name }}</strong><small>{{ request.email }}</small></span>
            <span class="status-dot" />
          </button>
        </div>
        <div class="account-list-count">{{ pendingRequestCount }} 个待审核</div>
      </aside>

      <main v-if="selectedRequest" class="account-detail-surface">
        <header class="account-detail-head">
          <button class="icon-button account-list-toggle" type="button" title="打开申请列表" aria-controls="request-list-panel" :aria-expanded="requestDrawerOpen" @click="requestDrawerOpen = true"><Menu :size="18" /></button>
          <div class="account-detail-identity"><strong>{{ selectedRequest.display_name }}</strong><span>{{ selectedRequest.email }}</span></div>
          <span class="account-status">待审核</span>
        </header>
        <div class="account-detail-scroll">
          <section class="account-detail-section">
            <div class="account-section-head"><h2>申请信息</h2></div>
            <dl class="review-details">
              <div><dt>邮箱验证</dt><dd>{{ selectedRequest.email_verified_at ? '已验证' : '未验证' }}</dd></div>
              <div><dt>申请时间</dt><dd>{{ new Date(selectedRequest.created_at).toLocaleString('zh-CN', { hour12: false }) }}</dd></div>
              <div><dt>部门意向</dt><dd>{{ selectedRequest.department_interest || '未填写' }}</dd></div>
              <div class="review-reason"><dt>申请理由</dt><dd>{{ selectedRequest.reason || '未填写' }}</dd></div>
            </dl>
          </section>
          <section class="account-detail-section">
            <div class="account-section-head"><h2>审批授权</h2><span>{{ reviewForm.department_ids.length }}</span></div>
            <div class="account-department-editor">
              <div class="field"><span>所属部门</span><div class="account-department-list"><label v-for="department in departments" :key="department.id" class="account-department-option"><input v-model="reviewForm.department_ids" type="checkbox" :value="department.id" @change="ensureReviewPrimaryDepartment" /><span>{{ department.name }}</span></label></div></div>
              <label class="field account-department-field"><span>主要部门</span><select v-model="reviewForm.primary_department_id" :disabled="!reviewForm.department_ids.length" required><option value="">选择主要部门</option><option v-for="department in departments.filter((row) => reviewForm.department_ids.includes(row.id))" :key="department.id" :value="department.id">{{ department.name }}</option></select></label>
              <label class="field account-department-field"><span>拒绝原因 <small>选填</small></span><textarea v-model="reviewForm.rejection_reason" rows="3" maxlength="500" /></label>
            </div>
          </section>
        </div>
        <footer class="account-detail-actions review-actions">
          <button class="primary-button" type="button" :disabled="reviewSaving || !emailReady || !reviewForm.department_ids.length" :title="emailReady ? '通过并发送激活邮件' : '请先配置并启用系统邮件'" @click="approveRequest"><Check :size="17" />{{ reviewSaving ? '处理中...' : '通过并发送激活邮件' }}</button>
          <button class="secondary-button danger-outline" type="button" :disabled="reviewSaving" @click="rejectRequest"><XCircle :size="17" />拒绝申请</button>
        </footer>
      </main>
      <div v-else class="account-detail-empty"><button class="icon-button account-list-toggle" type="button" title="打开申请列表" @click="requestDrawerOpen = true"><Menu :size="18" /></button><div class="workspace-selection-empty"><UserCheck :size="24" /><strong>没有待审核申请</strong><span>新申请会在邮箱验证后显示在这里</span></div></div>
    </section>

    <div v-else-if="loading" class="account-loading"><UserRound :size="20" /></div>

    <div v-if="inviteOpen" class="modal-backdrop" @click.self="closeInvite">
      <form ref="inviteDialogRef" class="dialog-panel account-dialog invite-dialog" role="dialog" aria-modal="true" aria-labelledby="invite-account-title" @keydown="handleInviteDialogKeydown" @submit.prevent="inviteUser">
        <div class="panel-head"><h2 id="invite-account-title">邀请成员</h2><button class="icon-button" type="button" title="关闭" :disabled="inviteSaving" @click="closeInvite"><X :size="17" /></button></div>
        <label class="field"><span>企业邮箱</span><input ref="inviteEmailRef" v-model="inviteForm.email" type="email" autocomplete="off" required /></label>
        <label class="field"><span>显示名</span><input v-model="inviteForm.display_name" required /></label>
        <div class="field"><span>所属部门</span><div class="account-department-list compact"><label v-for="department in departments" :key="department.id" class="account-department-option"><input v-model="inviteForm.department_ids" type="checkbox" :value="department.id" @change="ensureInvitePrimaryDepartment" /><span>{{ department.name }}</span></label></div></div>
        <label class="field"><span>主要部门</span><select v-model="inviteForm.primary_department_id" :disabled="!inviteForm.department_ids.length" required><option value="">选择主要部门</option><option v-for="department in departments.filter((row) => inviteForm.department_ids.includes(row.id))" :key="department.id" :value="department.id">{{ department.name }}</option></select></label>
        <p v-if="error" class="error" role="alert">{{ error }}</p>
        <div class="form-actions"><button class="primary-button" type="submit" :disabled="inviteSaving"><MailPlus :size="17" />{{ inviteSaving ? '正在发送...' : '发送激活邮件' }}</button></div>
      </form>
    </div>

    <div v-if="createOpen" class="modal-backdrop" @click.self="closeCreate">
      <form
        ref="createDialogRef"
        class="dialog-panel account-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="create-account-title"
        @keydown="handleCreateDialogKeydown"
        @submit.prevent="createUser"
      >
        <div class="panel-head">
          <h2 id="create-account-title">创建账号</h2>
          <button class="icon-button" type="button" title="关闭" @click="closeCreate">
            <X :size="17" />
          </button>
        </div>
        <label class="field">
          <span>邮箱账号</span>
          <input ref="createEmailRef" v-model="createForm.email" type="email" autocomplete="off" required />
        </label>
        <label class="field">
          <span>显示名</span>
          <input v-model="createForm.display_name" required />
        </label>
        <label class="field">
          <span>初始密码</span>
          <input v-model="createForm.password" type="password" autocomplete="new-password" minlength="12" maxlength="128" required />
        </label>
        <label v-if="isSystemAdmin && management?.capabilities.can_assign_system_role" class="field">
          <span>系统角色</span>
          <select v-model="createForm.system_role">
            <option v-for="option in systemRoleOptions" :key="option.value" :value="option.value">
              {{ option.label }}
            </option>
          </select>
        </label>
        <div class="field">
          <span>所属部门</span>
          <div class="account-department-list compact">
            <label v-for="department in assignableDepartments" :key="department.id" class="account-department-option">
              <input
                v-model="createForm.department_ids"
                type="checkbox"
                :value="department.id"
                @change="ensureCreatePrimaryDepartment"
              />
              <span>{{ department.name }}</span>
            </label>
          </div>
        </div>
        <label class="field">
          <span>主要部门</span>
          <select
            v-model="createForm.department_id"
            :disabled="!createForm.department_ids.length"
            :required="!management?.capabilities.can_create_without_department || createForm.department_ids.length > 0"
          >
            <option v-if="!createForm.department_ids.length" value="">未分配部门</option>
            <option
              v-for="department in assignableDepartments.filter((row) => createForm.department_ids.includes(row.id))"
              :key="department.id"
              :value="department.id"
            >
              {{ department.name }}
            </option>
          </select>
        </label>
        <p v-if="error" class="error" role="alert">{{ error }}</p>
        <div class="form-actions">
          <button class="primary-button" type="submit" :disabled="createSaving">
            <UserPlus :size="17" />
            <span>{{ createSaving ? '创建中' : '创建' }}</span>
          </button>
        </div>
      </form>
    </div>

    <div v-if="resetOpen && resetTarget" class="modal-backdrop" @click.self="closeReset">
      <form
        ref="resetDialogRef"
        class="dialog-panel account-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="reset-password-title"
        @keydown="handleResetDialogKeydown"
        @submit.prevent="resetPassword"
      >
        <div class="panel-head">
          <div>
            <h2 id="reset-password-title">重置密码</h2>
            <p class="account-dialog-target">{{ resetTarget.label }} · {{ resetTarget.email }}</p>
          </div>
          <button class="icon-button" type="button" title="关闭" @click="closeReset">
            <X :size="17" />
          </button>
        </div>
        <label class="field">
          <span>新密码</span>
          <input
            ref="resetPasswordRef"
            v-model="resetForm.password"
            type="password"
            autocomplete="new-password"
            minlength="12"
            maxlength="128"
            required
          />
        </label>
        <p v-if="error" class="error" role="alert">{{ error }}</p>
        <div class="form-actions">
          <button class="primary-button" type="submit" :disabled="resetSaving">
            <KeyRound :size="17" />
            <span>{{ resetSaving ? '重置中' : '重置并撤销会话' }}</span>
          </button>
        </div>
      </form>
    </div>
  </WorkbenchShell>
</template>
