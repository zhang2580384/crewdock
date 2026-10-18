<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref } from 'vue';
import { onBeforeRouteLeave } from 'vue-router';
import {
  Archive,
  ArchiveRestore,
  Briefcase,
  ChevronDown,
  ChevronRight,
  Menu,
  Plus,
  Save,
  Search,
  Trash2,
  UserPlus,
  X,
} from '@lucide/vue';
import WorkbenchShell from '../components/WorkbenchShell.vue';
import { api } from '../api';
import { useSessionStore } from '../stores/session';
import type {
  Department,
  ProjectDepartmentGrant,
  ProjectGrantCandidate,
  ProjectListItem,
  ProjectRole,
  ProjectUserGrant,
} from '../types';

interface ProjectBaseline {
  id: string;
  name: string;
  slug: string;
  departmentId: string;
  description: string;
}

interface DepartmentGrantDraft {
  role: ProjectRole | '';
  includeDescendants: boolean;
}

const session = useSessionStore();
const projects = ref<ProjectListItem[]>([]);
const departments = ref<Department[]>([]);
const manageableDepartmentIds = ref<string[]>([]);
const selectedId = ref('');
const searchQuery = ref('');
const drawerOpen = ref(false);
const loading = ref(true);
const detailLoading = ref(false);
const grantsReady = ref(false);
const saving = ref(false);
const actionBusy = ref(false);
const saveStage = ref('');
const error = ref('');
const success = ref('');
const baseline = ref<ProjectBaseline | null>(null);
const candidates = ref<ProjectGrantCandidate[]>([]);
const userGrantDetails = ref(new Map<string, ProjectUserGrant>());
const initialUserRoles = ref(new Map<string, ProjectRole>());
const initialDepartmentGrants = ref(new Map<string, DepartmentGrantDraft>());
const advancedOpen = ref(false);
const addUserId = ref('');
const addUserRole = ref<ProjectRole>('viewer');
const addDepartmentId = ref('');
const addDepartmentRole = ref<ProjectRole>('viewer');
const addDepartmentDescendants = ref(true);
const createOpen = ref(false);
const createSaving = ref(false);
const createName = ref('');
const deleteOpen = ref(false);
const deleteSaving = ref(false);
const deleteConfirmation = ref('');
const createDialogRef = ref<HTMLElement | null>(null);
const createNameRef = ref<HTMLInputElement | null>(null);
const deleteDialogRef = ref<HTMLElement | null>(null);
const deleteNameRef = ref<HTMLInputElement | null>(null);
const dialogReturnFocus = ref<HTMLElement | null>(null);
let detailGeneration = 0;

const form = reactive({ name: '', slug: '', departmentId: '', description: '' });
const directRoles = reactive<Record<string, ProjectRole | ''>>({});
const departmentDrafts = reactive<Record<string, DepartmentGrantDraft>>({});

const roleOptions: Array<{ value: ProjectRole; label: string }> = [
  { value: 'administrator', label: '管理员' },
  { value: 'editor', label: '编辑者' },
  { value: 'viewer', label: '查看者' },
];

const selectedProject = computed(() => projects.value.find((project) => project.id === selectedId.value) ?? null);
const isSystemAdmin = computed(() => session.user?.system_role === 'system_admin');
const canCreateProject = computed(() => Boolean(
  isSystemAdmin.value || (
    session.user?.can_manage_departments &&
    session.user.department_id &&
    manageableDepartmentSet.value.has(session.user.department_id)
  )
));
const canManageProject = computed(() => selectedProject.value?.access_role === 'administrator');
const canEditProject = computed(() => canManageProject.value && !selectedProject.value?.archived_at);
const canChangeDepartment = computed(() => canEditProject.value && (
  isSystemAdmin.value || session.user?.can_manage_departments
));
const manageableDepartmentSet = computed(() => new Set(manageableDepartmentIds.value));
const departmentOptions = computed(() => isSystemAdmin.value
  ? departments.value
  : departments.value.filter((department) => manageableDepartmentSet.value.has(department.id)));
const filteredProjects = computed(() => {
  const query = searchQuery.value.trim().toLocaleLowerCase();
  if (!query) return projects.value;
  return projects.value.filter((project) => `${project.name} ${project.slug}`.toLocaleLowerCase().includes(query));
});

const candidateMap = computed(() => new Map(candidates.value.map((candidate) => [candidate.id, candidate])));
const stagedUserIds = computed(() => Object.keys(directRoles).filter((id) => directRoles[id]));
const availableCandidates = computed(() => candidates.value.filter((candidate) => !directRoles[candidate.id]));
const stagedDepartmentIds = computed(() => Object.keys(departmentDrafts).filter((id) => departmentDrafts[id]?.role));
const availableDepartments = computed(() => departments.value.filter((department) => !departmentDrafts[department.id]?.role));
const hasAdvancedGrantData = computed(() => stagedUserIds.value.length > 0 || stagedDepartmentIds.value.length > 0);
const advancedVisible = computed(() => candidates.value.length > 1 || advancedOpen.value);

const settingsDirty = computed(() => {
  const previous = baseline.value;
  if (!previous || previous.id !== selectedId.value || !canEditProject.value) return false;
  return form.name !== previous.name || form.slug !== previous.slug ||
    form.departmentId !== previous.departmentId || form.description !== previous.description;
});

const grantsDirty = computed(() => {
  if (!grantsReady.value || !canEditProject.value) return false;
  const userIds = new Set([...Object.keys(directRoles), ...initialUserRoles.value.keys()]);
  for (const id of userIds) {
    if ((directRoles[id] ?? '') !== (initialUserRoles.value.get(id) ?? '')) return true;
  }
  const departmentIds = new Set([...Object.keys(departmentDrafts), ...initialDepartmentGrants.value.keys()]);
  for (const id of departmentIds) {
    const current = departmentDrafts[id] ?? { role: '', includeDescendants: false };
    const previous = initialDepartmentGrants.value.get(id) ?? { role: '', includeDescendants: false };
    if (current.role !== previous.role || (current.role && current.includeDescendants !== previous.includeDescendants)) return true;
  }
  return false;
});

const isDirty = computed(() => settingsDirty.value || grantsDirty.value);

function setMessage(nextError = '', nextSuccess = '') {
  error.value = nextError;
  success.value = nextSuccess;
}

function roleLabel(role: ProjectRole) {
  return roleOptions.find((option) => option.value === role)?.label ?? role;
}

function departmentLabel(id: string | null) {
  if (!id) return '未关联部门';
  return departments.value.find((department) => department.id === id)?.name ?? '未知部门';
}

function userLabel(id: string) {
  const candidate = candidateMap.value.get(id);
  const grant = userGrantDetails.value.get(id);
  return {
    name: candidate?.display_name ?? grant?.display_name ?? '未知账号',
    email: candidate?.email ?? grant?.email ?? id,
  };
}

function clearDrafts() {
  Object.keys(directRoles).forEach((id) => delete directRoles[id]);
  Object.keys(departmentDrafts).forEach((id) => delete departmentDrafts[id]);
  candidates.value = [];
  userGrantDetails.value = new Map();
  initialUserRoles.value = new Map();
  initialDepartmentGrants.value = new Map();
  grantsReady.value = false;
  addUserId.value = '';
  addDepartmentId.value = '';
}

function hydrateProject(project: ProjectListItem) {
  form.name = project.name;
  form.slug = project.slug;
  form.departmentId = project.department_id ?? '';
  form.description = project.description ?? '';
  baseline.value = {
    id: project.id,
    name: project.name,
    slug: project.slug,
    departmentId: project.department_id ?? '',
    description: project.description ?? '',
  };
}

function applyGrantDrafts(
  candidateRows: ProjectGrantCandidate[],
  userGrants: ProjectUserGrant[],
  departmentGrants: ProjectDepartmentGrant[],
) {
  clearDrafts();
  candidates.value = candidateRows;
  const nextUserDetails = new Map<string, ProjectUserGrant>();
  const nextUserRoles = new Map<string, ProjectRole>();
  userGrants.forEach((grant) => {
    directRoles[grant.user_id] = grant.project_role;
    nextUserDetails.set(grant.user_id, grant);
    nextUserRoles.set(grant.user_id, grant.project_role);
  });
  userGrantDetails.value = nextUserDetails;
  initialUserRoles.value = nextUserRoles;
  const nextDepartmentGrants = new Map<string, DepartmentGrantDraft>();
  departmentGrants.forEach((grant) => {
    const draft = { role: grant.project_role, includeDescendants: grant.include_descendants };
    departmentDrafts[grant.department_id] = { ...draft };
    nextDepartmentGrants.set(grant.department_id, { ...draft });
  });
  initialDepartmentGrants.value = nextDepartmentGrants;
  grantsReady.value = true;
  advancedOpen.value = candidateRows.length > 1;
}

async function loadProjects() {
  projects.value = await api.projects(true);
}

async function load() {
  loading.value = true;
  setMessage();
  try {
    await session.loadMe();
    const projectPromise = api.projects(true);
    const departmentPromise = api.departments();
    const managementPromise = session.user?.can_manage_accounts ? api.users().catch(() => null) : Promise.resolve(null);
    const [projectRows, departmentRows, management] = await Promise.all([
      projectPromise,
      departmentPromise,
      managementPromise,
    ]);
    projects.value = projectRows;
    departments.value = departmentRows;
    manageableDepartmentIds.value = management?.capabilities.manageable_department_ids ?? [];
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '加载项目失败');
  } finally {
    loading.value = false;
  }
}

async function loadGrants(projectId: string, generation: number) {
  detailLoading.value = true;
  try {
    const [candidateRows, userGrants, departmentGrants] = await Promise.all([
      api.projectGrantCandidates(projectId),
      api.projectUserGrants(projectId),
      api.projectDepartmentGrants(projectId),
    ]);
    if (generation !== detailGeneration || selectedId.value !== projectId) return;
    applyGrantDrafts(candidateRows, userGrants, departmentGrants);
  } catch (err) {
    if (generation === detailGeneration && selectedId.value === projectId) {
      setMessage(err instanceof Error ? err.message : '加载项目权限失败');
    }
  } finally {
    if (generation === detailGeneration) detailLoading.value = false;
  }
}

async function forceSelect(projectId: string) {
  const project = projects.value.find((item) => item.id === projectId);
  if (!project) {
    selectedId.value = '';
    baseline.value = null;
    clearDrafts();
    return;
  }
  detailGeneration += 1;
  const generation = detailGeneration;
  selectedId.value = project.id;
  drawerOpen.value = false;
  clearDrafts();
  hydrateProject(project);
  if (project.access_role === 'administrator' && !project.archived_at) {
    await loadGrants(project.id, generation);
  }
}

function confirmDiscard() {
  return !isDirty.value || window.confirm('当前项目有未保存修改，确认放弃这些修改？');
}

async function selectProject(project: ProjectListItem) {
  if (saving.value || actionBusy.value || detailLoading.value || project.id === selectedId.value) return;
  if (!confirmDiscard()) return;
  setMessage();
  await forceSelect(project.id);
}

function stageUserGrant() {
  if (!addUserId.value) return;
  directRoles[addUserId.value] = addUserRole.value;
  addUserId.value = '';
}

function stageDepartmentGrant() {
  if (!addDepartmentId.value) return;
  departmentDrafts[addDepartmentId.value] = {
    role: addDepartmentRole.value,
    includeDescendants: addDepartmentDescendants.value,
  };
  addDepartmentId.value = '';
}

async function persistGrantChanges(projectId: string) {
  const userIds = new Set([...Object.keys(directRoles), ...initialUserRoles.value.keys()]);
  for (const userId of userIds) {
    const previous = initialUserRoles.value.get(userId) ?? '';
    const next = directRoles[userId] ?? '';
    if (previous === next) continue;
    if (next) await api.putProjectUserGrant(projectId, userId, { project_role: next });
    else await api.deleteProjectUserGrant(projectId, userId);
  }
  const departmentIds = new Set([...Object.keys(departmentDrafts), ...initialDepartmentGrants.value.keys()]);
  for (const departmentId of departmentIds) {
    const previous = initialDepartmentGrants.value.get(departmentId) ?? { role: '', includeDescendants: false };
    const next = departmentDrafts[departmentId] ?? { role: '', includeDescendants: false };
    if (previous.role === next.role && (!next.role || previous.includeDescendants === next.includeDescendants)) continue;
    if (next.role) {
      await api.putProjectDepartmentGrant(projectId, departmentId, {
        project_role: next.role,
        include_descendants: next.includeDescendants,
      });
    } else {
      await api.deleteProjectDepartmentGrant(projectId, departmentId);
    }
  }
}

async function saveProject() {
  const project = selectedProject.value;
  if (!project || !canEditProject.value || saving.value || detailLoading.value || !grantsReady.value) return;
  const projectId = project.id;
  saving.value = true;
  setMessage();
  try {
    if (settingsDirty.value) {
      saveStage.value = '项目设置';
      await api.updateProject(projectId, {
        name: form.name.trim(),
        ...(form.slug.trim() ? { slug: form.slug.trim() } : {}),
        description: form.description.trim(),
        ...(canChangeDepartment.value ? { department_id: form.departmentId || null } : {}),
      });
    }
    saveStage.value = '访问权限';
    await persistGrantChanges(projectId);
    saveStage.value = '刷新';
    await loadProjects();
    await forceSelect(projectId);
    setMessage('', `已保存“${projects.value.find((item) => item.id === projectId)?.name ?? project.name}”`);
  } catch (err) {
    const stage = saveStage.value;
    try {
      await loadProjects();
      await forceSelect(projectId);
    } catch {
      selectedId.value = '';
      baseline.value = null;
      clearDrafts();
    }
    setMessage(`保存失败（${stage}）：${err instanceof Error ? err.message : '未知错误'}。已刷新服务器状态`);
  } finally {
    saveStage.value = '';
    saving.value = false;
  }
}

async function openCreate() {
  dialogReturnFocus.value = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  createName.value = '';
  createOpen.value = true;
  setMessage();
  await nextTick();
  createNameRef.value?.focus();
}

async function closeCreate(force = false) {
  if (createSaving.value && !force) return;
  if (!force && createName.value.trim() && !window.confirm('项目名称尚未提交，确认关闭？')) return;
  createOpen.value = false;
  createName.value = '';
  await nextTick();
  dialogReturnFocus.value?.focus();
  dialogReturnFocus.value = null;
}

async function createProject() {
  if (!createName.value.trim() || createSaving.value) return;
  createSaving.value = true;
  setMessage();
  try {
    const created = await api.createProject({
      name: createName.value.trim(),
      ...(isSystemAdmin.value ? {} : { department_id: session.user?.department_id ?? undefined }),
    });
    await closeCreate(true);
    await loadProjects();
    await forceSelect(created.id);
    setMessage('', `已创建“${created.name}”`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '创建项目失败');
  } finally {
    createSaving.value = false;
  }
}

async function archiveProject() {
  const project = selectedProject.value;
  if (!project || !canManageProject.value || actionBusy.value) return;
  if (!confirmDiscard()) return;
  if (!window.confirm(`确认归档项目“${project.name}”？归档后项目内容将停止编辑。`)) return;
  actionBusy.value = true;
  setMessage();
  try {
    await api.archiveProject(project.id);
    await loadProjects();
    await forceSelect(project.id);
    setMessage('', `已归档“${project.name}”`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '归档项目失败');
  } finally {
    actionBusy.value = false;
  }
}

async function restoreProject() {
  const project = selectedProject.value;
  if (!project || !canManageProject.value || actionBusy.value) return;
  actionBusy.value = true;
  setMessage();
  try {
    await api.restoreProject(project.id);
    await loadProjects();
    await forceSelect(project.id);
    setMessage('', `已恢复“${project.name}”`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '恢复项目失败');
  } finally {
    actionBusy.value = false;
  }
}

async function openDelete() {
  dialogReturnFocus.value = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  deleteConfirmation.value = '';
  deleteOpen.value = true;
  await nextTick();
  deleteNameRef.value?.focus();
}

async function closeDelete(force = false) {
  if (deleteSaving.value && !force) return;
  if (!force && deleteConfirmation.value && !window.confirm('删除确认尚未提交，确认关闭？')) return;
  deleteOpen.value = false;
  deleteConfirmation.value = '';
  await nextTick();
  dialogReturnFocus.value?.focus();
  dialogReturnFocus.value = null;
}

async function permanentlyDeleteProject() {
  const project = selectedProject.value;
  if (!project || deleteConfirmation.value !== project.name || deleteSaving.value) return;
  deleteSaving.value = true;
  setMessage();
  try {
    await api.deleteProject(project.id);
    await closeDelete(true);
    selectedId.value = '';
    baseline.value = null;
    clearDrafts();
    await loadProjects();
    setMessage('', `已永久删除“${project.name}”，文件清理任务已进入队列`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '永久删除项目失败');
  } finally {
    deleteSaving.value = false;
  }
}

function trapDialogFocus(event: KeyboardEvent, dialog: HTMLElement | null, close: () => void) {
  if (event.key === 'Escape') {
    event.preventDefault();
    close();
    return;
  }
  if (event.key !== 'Tab' || !dialog) return;
  const focusable = Array.from(dialog.querySelectorAll<HTMLElement>(
    'button:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
  ));
  if (!focusable.length) return;
  const first = focusable[0];
  const last = focusable[focusable.length - 1];
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault(); last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault(); first.focus();
  }
}

function handleBeforeUnload(event: BeforeUnloadEvent) {
  if (!isDirty.value) return;
  event.preventDefault();
  event.returnValue = '';
}

onBeforeRouteLeave(() => !saving.value && !actionBusy.value && confirmDiscard());
onMounted(() => {
  window.addEventListener('beforeunload', handleBeforeUnload);
  load();
});
onBeforeUnmount(() => {
  detailGeneration += 1;
  window.removeEventListener('beforeunload', handleBeforeUnload);
});
</script>

<template>
  <WorkbenchShell layout="workspace">
    <div class="page-head access-page-head">
      <div><h1>项目管理</h1></div>
      <button v-if="canCreateProject" class="primary-button" type="button" @click="openCreate"><Plus :size="17" /><span>创建项目</span></button>
    </div>

    <p v-if="error" class="error access-message">{{ error }}</p>
    <p v-if="success" class="success access-message">{{ success }}</p>

    <section v-if="!loading" class="access-workspace projects-workspace" :class="{ 'rail-open': drawerOpen }">
      <button class="access-drawer-backdrop" type="button" aria-label="关闭项目列表" @click="drawerOpen = false" />
      <aside id="project-list-panel" class="access-rail project-list-rail">
        <div class="access-rail-head project-list-head">
          <label class="access-search"><Search :size="16" /><input v-model="searchQuery" type="search" placeholder="搜索项目" /></label>
          <button class="icon-button access-rail-close" type="button" title="关闭项目列表" @click="drawerOpen = false"><X :size="17" /></button>
        </div>
        <div v-if="filteredProjects.length" class="project-list-scroll">
          <button v-for="project in filteredProjects" :key="project.id" class="project-list-item" :class="{ active: project.id === selectedId }" type="button" :disabled="saving || actionBusy || detailLoading" @click="selectProject(project)">
            <span class="project-list-icon"><Briefcase :size="17" /></span>
            <span class="project-list-copy"><strong>{{ project.name }}</strong><small>{{ project.slug }}</small></span>
            <span class="project-role-badge" :class="{ archived: project.archived_at }">{{ project.archived_at ? '已归档' : roleLabel(project.access_role) }}</span>
          </button>
        </div>
        <div v-else class="access-rail-empty"><Briefcase :size="21" /><span>{{ projects.length ? '没有匹配项目' : '暂无项目' }}</span><button v-if="!projects.length && canCreateProject" class="secondary-button" type="button" @click="openCreate">创建项目</button></div>
        <div class="access-rail-count">{{ filteredProjects.length }} / {{ projects.length }}</div>
      </aside>

      <main v-if="selectedProject" class="access-detail-surface">
        <header class="access-detail-head">
          <button class="icon-button access-rail-toggle" type="button" title="打开项目列表" aria-controls="project-list-panel" :aria-expanded="drawerOpen" @click="drawerOpen = true"><Menu :size="18" /></button>
          <div class="access-detail-title"><strong>{{ selectedProject.name }}</strong><span>{{ roleLabel(selectedProject.access_role) }} · {{ selectedProject.archived_at ? '已归档' : departmentLabel(selectedProject.department_id) }}</span></div>
          <span v-if="selectedProject.archived_at" class="project-archived-label">已归档</span>
        </header>

        <div class="access-detail-scroll">
          <section class="access-detail-section">
            <div class="access-section-head"><h2>项目信息</h2><span>{{ canEditProject ? '可编辑' : '只读' }}</span></div>
            <div class="access-fields-grid">
              <label class="field"><span>名称</span><input v-model="form.name" :readonly="!canEditProject" required /></label>
              <label class="field"><span>Slug <small>可选</small></span><input v-model="form.slug" :readonly="!canEditProject" pattern="[a-z0-9-]+" /></label>
              <label class="field">
                <span>所属部门 <small>可选</small></span>
                <select v-if="canChangeDepartment" v-model="form.departmentId"><option value="">未关联部门</option><option v-for="department in departmentOptions" :key="department.id" :value="department.id">{{ department.name }}</option></select>
                <input v-else :value="departmentLabel(selectedProject.department_id)" readonly />
              </label>
              <div class="access-readonly-field"><span>访问角色</span><strong>{{ roleLabel(selectedProject.access_role) }}</strong></div>
              <label class="field access-description-field"><span>描述 <small>可选</small></span><textarea v-model="form.description" :readonly="!canEditProject" /></label>
            </div>
          </section>

          <section v-if="canManageProject && !selectedProject.archived_at" class="access-detail-section project-grants-section">
            <div class="access-section-head"><h2>访问权限</h2><span>直接账号与部门授权</span></div>
            <div v-if="detailLoading" class="access-inline-state">正在加载权限...</div>
            <template v-else-if="grantsReady">
              <button v-if="!advancedVisible" class="advanced-disclosure" type="button" :aria-expanded="advancedOpen" @click="advancedOpen = true"><ChevronRight :size="16" /><span>展开高级授权</span><small v-if="hasAdvancedGrantData">已配置</small></button>
              <div v-else class="grant-groups">
                <div class="grant-group">
                  <div class="grant-group-head"><strong>直接账号授权</strong><button v-if="candidates.length <= 1" class="icon-button" type="button" title="折叠高级授权" @click="advancedOpen = false"><ChevronDown :size="16" /></button></div>
                  <div class="grant-add-row">
                    <select v-model="addUserId" aria-label="选择账号"><option value="">选择账号</option><option v-for="candidate in availableCandidates" :key="candidate.id" :value="candidate.id">{{ candidate.display_name }} · {{ candidate.email }}</option></select>
                    <select v-model="addUserRole" aria-label="选择项目角色"><option v-for="option in roleOptions" :key="option.value" :value="option.value">{{ option.label }}</option></select>
                    <button class="icon-button" type="button" title="添加账号授权" :disabled="!addUserId" @click="stageUserGrant"><UserPlus :size="16" /></button>
                  </div>
                  <div v-if="stagedUserIds.length" class="grant-list">
                    <div v-for="userId in stagedUserIds" :key="userId" class="grant-row">
                      <span class="grant-identity"><strong>{{ userLabel(userId).name }}</strong><small>{{ userLabel(userId).email }}</small></span>
                      <select v-model="directRoles[userId]" aria-label="账号项目角色"><option v-for="option in roleOptions" :key="option.value" :value="option.value">{{ option.label }}</option></select>
                      <button class="icon-button" type="button" title="移除账号授权" @click="directRoles[userId] = ''"><X :size="16" /></button>
                    </div>
                  </div>
                  <div v-else class="grant-empty">暂无直接账号授权</div>
                </div>

                <div class="grant-group">
                  <div class="grant-group-head"><strong>部门授权</strong></div>
                  <div class="grant-add-row department-grant-add">
                    <select v-model="addDepartmentId" aria-label="选择部门"><option value="">选择部门</option><option v-for="department in availableDepartments" :key="department.id" :value="department.id">{{ department.name }}</option></select>
                    <select v-model="addDepartmentRole" aria-label="选择项目角色"><option v-for="option in roleOptions" :key="option.value" :value="option.value">{{ option.label }}</option></select>
                    <label class="compact-check"><input v-model="addDepartmentDescendants" type="checkbox" /><span>含下级</span></label>
                    <button class="icon-button" type="button" title="添加部门授权" :disabled="!addDepartmentId" @click="stageDepartmentGrant"><Plus :size="16" /></button>
                  </div>
                  <div v-if="stagedDepartmentIds.length" class="grant-list">
                    <div v-for="departmentId in stagedDepartmentIds" :key="departmentId" class="grant-row department-grant-row">
                      <span class="grant-identity"><strong>{{ departmentLabel(departmentId) }}</strong><small>部门范围</small></span>
                      <select v-model="departmentDrafts[departmentId].role" aria-label="部门项目角色"><option v-for="option in roleOptions" :key="option.value" :value="option.value">{{ option.label }}</option></select>
                      <label class="compact-check"><input v-model="departmentDrafts[departmentId].includeDescendants" type="checkbox" /><span>含下级</span></label>
                      <button class="icon-button" type="button" title="移除部门授权" @click="departmentDrafts[departmentId].role = ''"><X :size="16" /></button>
                    </div>
                  </div>
                  <div v-else class="grant-empty">暂无部门授权</div>
                </div>
              </div>
            </template>
          </section>

          <section v-else-if="!canManageProject" class="access-detail-section project-readonly-note"><strong>权限设置只对项目管理员开放</strong><span>当前角色可以查看项目资料，但不能修改项目设置或授权。</span></section>

          <section v-if="canManageProject" class="access-detail-section access-danger-section project-state-section">
            <div><strong>{{ selectedProject.archived_at ? '恢复项目' : '归档项目' }}</strong><span>{{ selectedProject.archived_at ? '恢复后项目内容可继续使用。' : '归档会停止项目设置和内容编辑。' }}</span></div>
            <button class="secondary-button" type="button" :disabled="actionBusy" @click="selectedProject.archived_at ? restoreProject() : archiveProject()"><component :is="selectedProject.archived_at ? ArchiveRestore : Archive" :size="16" /><span>{{ actionBusy ? '处理中' : selectedProject.archived_at ? '恢复' : '归档' }}</span></button>
          </section>

          <section v-if="isSystemAdmin && selectedProject.archived_at" class="access-detail-section access-danger-section permanent-delete-section">
            <div><strong>永久删除</strong><span>删除项目数据并进入异步文件清理，无法恢复。</span></div>
            <button class="secondary-button danger-button" type="button" @click="openDelete"><Trash2 :size="16" /><span>永久删除</span></button>
          </section>
        </div>

        <footer v-if="canEditProject" class="access-detail-actions">
          <span v-if="isDirty" class="access-dirty-state">有未保存修改</span>
          <button class="primary-button" type="button" :disabled="saving || detailLoading || !grantsReady || !isDirty || !form.name.trim()" @click="saveProject"><Save :size="17" /><span>{{ saving ? `保存中 · ${saveStage}` : '保存' }}</span></button>
        </footer>
      </main>

      <div v-else class="access-detail-empty">
        <button class="icon-button access-rail-toggle" type="button" title="打开项目列表" aria-controls="project-list-panel" :aria-expanded="drawerOpen" @click="drawerOpen = true"><Menu :size="18" /></button>
        <div class="workspace-selection-empty">
          <Briefcase :size="24" />
          <strong>选择一个项目</strong>
          <span>查看项目设置、成员和部门授权</span>
        </div>
      </div>
    </section>
    <div v-else class="access-loading">正在加载项目...</div>

    <div v-if="createOpen" class="modal-backdrop" @click.self="closeCreate()">
      <form ref="createDialogRef" class="dialog-panel access-dialog" role="dialog" aria-modal="true" aria-labelledby="create-project-title" @keydown="trapDialogFocus($event, createDialogRef, closeCreate)" @submit.prevent="createProject">
        <div class="panel-head"><h2 id="create-project-title">创建项目</h2><button class="icon-button" type="button" title="关闭" @click="closeCreate()"><X :size="17" /></button></div>
        <label class="field"><span>项目名称</span><input ref="createNameRef" v-model="createName" maxlength="160" required /></label>
        <div class="form-actions"><button class="primary-button" type="submit" :disabled="createSaving || !createName.trim()"><Plus :size="17" /><span>{{ createSaving ? '创建中' : '创建' }}</span></button></div>
      </form>
    </div>

    <div v-if="deleteOpen && selectedProject" class="modal-backdrop" @click.self="closeDelete()">
      <form ref="deleteDialogRef" class="dialog-panel access-dialog" role="dialog" aria-modal="true" aria-labelledby="delete-project-title" @keydown="trapDialogFocus($event, deleteDialogRef, closeDelete)" @submit.prevent="permanentlyDeleteProject">
        <div class="panel-head"><div><h2 id="delete-project-title">永久删除项目</h2><p class="access-dialog-target">输入“{{ selectedProject.name }}”确认</p></div><button class="icon-button" type="button" title="关闭" @click="closeDelete()"><X :size="17" /></button></div>
        <label class="field"><span>项目名称</span><input ref="deleteNameRef" v-model="deleteConfirmation" autocomplete="off" /></label>
        <div class="form-actions"><button class="primary-button destructive-primary" type="submit" :disabled="deleteSaving || deleteConfirmation !== selectedProject.name"><Trash2 :size="17" /><span>{{ deleteSaving ? '删除中' : '永久删除' }}</span></button></div>
      </form>
    </div>
  </WorkbenchShell>
</template>
