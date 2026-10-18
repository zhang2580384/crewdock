<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref } from 'vue';
import { onBeforeRouteLeave } from 'vue-router';
import {
  ChevronDown,
  ChevronRight,
  Folder,
  FolderOpen,
  FolderPlus,
  Menu,
  Save,
  Trash2,
  X,
} from '@lucide/vue';
import WorkbenchShell from '../components/WorkbenchShell.vue';
import { api } from '../api';
import { useSessionStore } from '../stores/session';
import type { Department } from '../types';

interface DepartmentRow extends Department {
  depth: number;
  hasChildren: boolean;
}

interface DepartmentBaseline {
  id: string;
  name: string;
  parentId: string;
  position: number;
}

const session = useSessionStore();
const departments = ref<Department[]>([]);
const manageableIds = ref<string[]>([]);
const selectedId = ref('');
const collapsedIds = ref(new Set<string>());
const drawerOpen = ref(false);
const loading = ref(true);
const saving = ref(false);
const deleting = ref(false);
const error = ref('');
const success = ref('');
const baseline = ref<DepartmentBaseline | null>(null);
const createOpen = ref(false);
const createSaving = ref(false);
const createParentId = ref<string | null>(null);
const createName = ref('');
const createDialogRef = ref<HTMLElement | null>(null);
const createNameRef = ref<HTMLInputElement | null>(null);
const dialogReturnFocus = ref<HTMLElement | null>(null);

const form = reactive({ name: '', parentId: '', position: 0 });

const isSystemAdmin = computed(() => session.user?.system_role === 'system_admin');
const manageableSet = computed(() => new Set(manageableIds.value));
const selectedDepartment = computed(
  () => departments.value.find((department) => department.id === selectedId.value) ?? null,
);
const canManageSelected = computed(
  () => Boolean(selectedDepartment.value) && (isSystemAdmin.value || manageableSet.value.has(selectedId.value)),
);
const isDirty = computed(() => {
  if (!baseline.value || baseline.value.id !== selectedId.value) return false;
  return (
    form.name !== baseline.value.name ||
    form.parentId !== baseline.value.parentId ||
    form.position !== baseline.value.position
  );
});

const rows = computed<DepartmentRow[]>(() => {
  const children = new Map<string, Department[]>();
  for (const department of departments.value) {
    const key = department.parent_id ?? '';
    const group = children.get(key) ?? [];
    group.push(department);
    children.set(key, group);
  }
  children.forEach((group) => group.sort((a, b) => a.sort_order - b.sort_order || a.name.localeCompare(b.name)));
  const result: DepartmentRow[] = [];
  const visit = (parentId: string, depth: number) => {
    for (const department of children.get(parentId) ?? []) {
      const hasChildren = (children.get(department.id)?.length ?? 0) > 0;
      result.push({ ...department, depth, hasChildren });
      if (hasChildren && !collapsedIds.value.has(department.id)) visit(department.id, depth + 1);
    }
  };
  visit('', 0);
  return result;
});

const descendantIds = computed(() => {
  const descendants = new Set<string>();
  const visit = (parentId: string) => {
    departments.value.filter((item) => item.parent_id === parentId).forEach((item) => {
      descendants.add(item.id);
      visit(item.id);
    });
  };
  if (selectedId.value) visit(selectedId.value);
  return descendants;
});

const parentOptions = computed(() => departments.value.filter((department) => {
  if (department.id === selectedId.value || descendantIds.value.has(department.id)) return false;
  return isSystemAdmin.value || manageableSet.value.has(department.id);
}));

const siblingLimit = computed(() => departments.value.filter(
  (department) => (department.parent_id ?? '') === form.parentId && department.id !== selectedId.value,
).length);

function setMessage(nextError = '', nextSuccess = '') {
  error.value = nextError;
  success.value = nextSuccess;
}

function hydrate(department: Department) {
  form.name = department.name;
  form.parentId = department.parent_id ?? '';
  form.position = department.sort_order;
  baseline.value = {
    id: department.id,
    name: department.name,
    parentId: department.parent_id ?? '',
    position: department.sort_order,
  };
}

async function loadDepartments() {
  departments.value = await api.departments();
}

async function load() {
  loading.value = true;
  setMessage();
  try {
    await session.loadMe();
    if (!session.user?.can_manage_departments) throw new Error('当前账号没有组织管理权限');
    const departmentPromise = api.departments();
    const managementPromise = session.user.can_manage_accounts ? api.users() : Promise.resolve(null);
    const [departmentRows, management] = await Promise.all([departmentPromise, managementPromise]);
    departments.value = departmentRows;
    manageableIds.value = management?.capabilities.manageable_department_ids ?? [];
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '加载组织结构失败');
  } finally {
    loading.value = false;
  }
}

function confirmDiscard() {
  return !isDirty.value || window.confirm('当前部门有未保存修改，确认放弃这些修改？');
}

function selectDepartment(department: Department) {
  if (saving.value || deleting.value || department.id === selectedId.value) return;
  if (!confirmDiscard()) return;
  selectedId.value = department.id;
  drawerOpen.value = false;
  hydrate(department);
  setMessage();
}

function toggleCollapsed(id: string) {
  const next = new Set(collapsedIds.value);
  next.has(id) ? next.delete(id) : next.add(id);
  collapsedIds.value = next;
}

async function saveDepartment() {
  const department = selectedDepartment.value;
  const previous = baseline.value;
  if (!department || !previous || !canManageSelected.value || saving.value) return;
  saving.value = true;
  setMessage();
  try {
    if (form.name !== previous.name) {
      await api.updateDepartment(department.id, { name: form.name.trim() });
    }
    if (form.parentId !== previous.parentId || form.position !== previous.position) {
      await api.moveDepartment(department.id, {
        parent_id: form.parentId || null,
        position: Math.max(0, Math.min(form.position, siblingLimit.value)),
      });
    }
    await loadDepartments();
    const refreshed = departments.value.find((item) => item.id === department.id);
    if (refreshed) hydrate(refreshed);
    setMessage('', `已保存“${refreshed?.name ?? department.name}”`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '保存部门失败');
  } finally {
    saving.value = false;
  }
}

async function openCreate(parentId: string | null) {
  dialogReturnFocus.value = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  createParentId.value = parentId;
  createName.value = '';
  createOpen.value = true;
  setMessage();
  await nextTick();
  createNameRef.value?.focus();
}

async function closeCreate(force = false) {
  if (createSaving.value && !force) return;
  if (!force && createName.value.trim() && !window.confirm('部门名称尚未提交，确认关闭？')) return;
  createOpen.value = false;
  createName.value = '';
  await nextTick();
  dialogReturnFocus.value?.focus();
  dialogReturnFocus.value = null;
}

async function createDepartment() {
  if (!createName.value.trim() || createSaving.value) return;
  createSaving.value = true;
  setMessage();
  try {
    const created = await api.createDepartment({ parent_id: createParentId.value, name: createName.value.trim() });
    await closeCreate(true);
    await loadDepartments();
    selectedId.value = created.id;
    const refreshed = departments.value.find((item) => item.id === created.id) ?? created;
    hydrate(refreshed);
    setMessage('', `已创建“${created.name}”`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '创建部门失败');
  } finally {
    createSaving.value = false;
  }
}

async function deleteDepartment() {
  const department = selectedDepartment.value;
  if (!department || !canManageSelected.value || deleting.value) return;
  if (!confirmDiscard()) return;
  if (!window.confirm(`确认删除部门“${department.name}”？被下级部门、账号、项目或授权引用时将拒绝删除。`)) return;
  deleting.value = true;
  setMessage();
  try {
    await api.deleteDepartment(department.id);
    selectedId.value = '';
    baseline.value = null;
    await loadDepartments();
    setMessage('', `已删除“${department.name}”`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '删除部门失败');
  } finally {
    deleting.value = false;
  }
}

function trapDialogFocus(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault();
    closeCreate();
    return;
  }
  if (event.key !== 'Tab' || !createDialogRef.value) return;
  const focusable = Array.from(createDialogRef.value.querySelectorAll<HTMLElement>(
    'button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex="-1"])',
  ));
  if (!focusable.length) return;
  const [first] = focusable;
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

onBeforeRouteLeave(() => !saving.value && confirmDiscard());
onMounted(() => {
  window.addEventListener('beforeunload', handleBeforeUnload);
  load();
});
onBeforeUnmount(() => window.removeEventListener('beforeunload', handleBeforeUnload));
</script>

<template>
  <WorkbenchShell layout="workspace">
    <div class="page-head access-page-head">
      <div><h1>组织管理</h1></div>
      <button v-if="isSystemAdmin" class="primary-button" type="button" @click="openCreate(null)">
        <FolderPlus :size="17" /><span>新建根部门</span>
      </button>
    </div>

    <p v-if="error" class="error access-message">{{ error }}</p>
    <p v-if="success" class="success access-message">{{ success }}</p>

    <section v-if="!loading" class="access-workspace organization-workspace" :class="{ 'rail-open': drawerOpen }">
      <button class="access-drawer-backdrop" type="button" aria-label="关闭部门树" @click="drawerOpen = false" />
      <aside id="department-tree-panel" class="access-rail department-rail">
        <div class="access-rail-head">
          <strong>部门结构</strong>
          <button class="icon-button access-rail-close" type="button" title="关闭部门树" @click="drawerOpen = false"><X :size="17" /></button>
        </div>
        <div v-if="rows.length" class="department-tree-scroll" role="tree">
          <div v-for="row in rows" :key="row.id" class="department-tree-line" :style="{ '--tree-depth': row.depth }">
            <button
              class="department-tree-toggle"
              type="button"
              :disabled="!row.hasChildren"
              :title="collapsedIds.has(row.id) ? '展开下级部门' : '收起下级部门'"
              @click="toggleCollapsed(row.id)"
            >
              <component :is="collapsedIds.has(row.id) ? ChevronRight : ChevronDown" v-if="row.hasChildren" :size="14" />
            </button>
            <button
              class="department-tree-item"
              :class="{ active: row.id === selectedId }"
              type="button"
              role="treeitem"
              :aria-selected="row.id === selectedId"
              @click="selectDepartment(row)"
            >
              <component :is="row.hasChildren && !collapsedIds.has(row.id) ? FolderOpen : Folder" :size="17" />
              <span>{{ row.name }}</span>
            </button>
          </div>
        </div>
        <div v-else class="access-rail-empty">
          <Folder :size="21" />
          <span>暂无部门</span>
          <button v-if="isSystemAdmin" class="secondary-button" type="button" @click="openCreate(null)">创建部门</button>
        </div>
        <div class="access-rail-count">{{ departments.length }} 个部门</div>
      </aside>

      <main v-if="selectedDepartment" class="access-detail-surface">
        <header class="access-detail-head">
          <button class="icon-button access-rail-toggle" type="button" title="打开部门树" aria-controls="department-tree-panel" :aria-expanded="drawerOpen" @click="drawerOpen = true"><Menu :size="18" /></button>
          <div class="access-detail-title">
            <strong>{{ selectedDepartment.name }}</strong>
            <span>{{ canManageSelected ? '可管理范围' : '只读范围' }}</span>
          </div>
          <button v-if="canManageSelected" class="secondary-button compact-action" type="button" @click="openCreate(selectedDepartment.id)"><FolderPlus :size="16" /><span>新建下级</span></button>
        </header>
        <div class="access-detail-scroll">
          <section class="access-detail-section">
            <div class="access-section-head"><h2>部门信息</h2><span>层级与同级排序</span></div>
            <div class="access-fields-grid">
              <label class="field"><span>名称</span><input v-model="form.name" :readonly="!canManageSelected" required /></label>
              <label class="field">
                <span>父部门</span>
                <select v-model="form.parentId" :disabled="!canManageSelected">
                  <option v-if="isSystemAdmin" value="">无（根部门）</option>
                  <option v-for="department in parentOptions" :key="department.id" :value="department.id">{{ department.name }}</option>
                </select>
              </label>
              <label class="field access-position-field">
                <span>同级位置</span>
                <input v-model.number="form.position" type="number" min="0" :max="siblingLimit" :readonly="!canManageSelected" />
              </label>
              <div class="access-readonly-field"><span>当前排序</span><strong>第 {{ selectedDepartment.sort_order + 1 }} 位</strong></div>
            </div>
          </section>
          <section v-if="canManageSelected" class="access-detail-section access-danger-section">
            <div><strong>删除部门</strong><span>存在下级部门、账号、项目或授权引用时不会删除。</span></div>
            <button class="secondary-button danger-button" type="button" :disabled="deleting" @click="deleteDepartment"><Trash2 :size="16" /><span>{{ deleting ? '删除中' : '删除' }}</span></button>
          </section>
        </div>
        <footer v-if="canManageSelected" class="access-detail-actions">
          <span v-if="isDirty" class="access-dirty-state">有未保存修改</span>
          <button class="primary-button" type="button" :disabled="saving || !isDirty || !form.name.trim()" @click="saveDepartment"><Save :size="17" /><span>{{ saving ? '保存中' : '保存' }}</span></button>
        </footer>
      </main>
      <div v-else class="access-detail-empty">
        <button class="icon-button access-rail-toggle" type="button" title="打开部门树" aria-controls="department-tree-panel" :aria-expanded="drawerOpen" @click="drawerOpen = true"><Menu :size="18" /></button>
        <div class="workspace-selection-empty">
          <Folder :size="24" />
          <strong>选择一个部门</strong>
          <span>查看层级位置和部门管理选项</span>
        </div>
      </div>
    </section>
    <div v-else class="access-loading">正在加载组织结构...</div>

    <div v-if="createOpen" class="modal-backdrop" @click.self="closeCreate()">
      <form ref="createDialogRef" class="dialog-panel access-dialog" role="dialog" aria-modal="true" aria-labelledby="create-department-title" @keydown="trapDialogFocus" @submit.prevent="createDepartment">
        <div class="panel-head">
          <div><h2 id="create-department-title">{{ createParentId ? '新建下级部门' : '新建根部门' }}</h2><p v-if="createParentId" class="access-dialog-target">父部门：{{ selectedDepartment?.name }}</p></div>
          <button class="icon-button" type="button" title="关闭" @click="closeCreate()"><X :size="17" /></button>
        </div>
        <label class="field"><span>部门名称</span><input ref="createNameRef" v-model="createName" maxlength="120" required /></label>
        <div class="form-actions"><button class="primary-button" type="submit" :disabled="createSaving || !createName.trim()"><FolderPlus :size="17" /><span>{{ createSaving ? '创建中' : '创建' }}</span></button></div>
      </form>
    </div>
  </WorkbenchShell>
</template>
