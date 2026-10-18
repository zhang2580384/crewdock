<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import { onBeforeRouteLeave } from 'vue-router';
import { ClipboardList, Menu, Plus, Search, Trash2, X } from '@lucide/vue';
import { MdEditor, MdPreview } from 'md-editor-v3';
import 'md-editor-v3/lib/style.css';
import WorkbenchShell from '../components/WorkbenchShell.vue';
import { api } from '../api';
import { useSessionStore } from '../stores/session';
import { useWorkspaceStore } from '../stores/workspace';
import type { ProjectAssignableUser, ProjectListItem, Ticket, TicketStatus } from '../types';

const session = useSessionStore();
const workspace = useWorkspaceStore();

const projects = ref<ProjectListItem[]>([]);
const tickets = ref<Ticket[]>([]);
const users = ref<ProjectAssignableUser[]>([]);
const membersByProject = ref<Record<string, ProjectAssignableUser[]>>({});
const selectedId = ref('');
const creating = ref(false);
const drawerOpen = ref(false);
const query = ref('');
const loading = ref(false);
const saving = ref(false);
const deletingId = ref('');
const error = ref('');
const blankDescription = '## 问题描述\n\n';
const form = reactive({ project_id: '', title: '', description_markdown: blankDescription, status: 'pending' as TicketStatus, assignee_id: '' });

const statuses: Array<{ value: TicketStatus; label: string }> = [
  { value: 'pending', label: '待处理' },
  { value: 'in_progress', label: '处理中' },
  { value: 'waiting_confirm', label: '待确认' },
  { value: 'done', label: '已完成' },
  { value: 'closed', label: '已关闭' },
];

const selectedTicket = computed(() => tickets.value.find((ticket) => ticket.id === selectedId.value) || null);
const userNames = computed(() => Object.fromEntries(users.value.map((user) => [user.id, user.display_name || user.email])));
const projectNames = computed(() => Object.fromEntries(projects.value.map((project) => [project.id, project.name])));
const writableProjects = computed(() => projects.value.filter((project) => project.can_write));
const canEditSelected = computed(() => Boolean(selectedTicket.value && canWriteProject(selectedTicket.value.project_id)));
const filteredTickets = computed(() => {
  const needle = query.value.trim().toLocaleLowerCase();
  const scoped = workspace.projectId
    ? tickets.value.filter((ticket) => ticket.project_id === workspace.projectId)
    : tickets.value;
  if (!needle) return scoped;
  return scoped.filter((ticket) => [
    ticket.title,
    projectNames.value[ticket.project_id] || '',
    statusLabel(ticket.status),
    assigneeLabel(ticket),
  ].some((value) => value.toLocaleLowerCase().includes(needle)));
});
const assigneeOptions = computed(() => {
  const options = (membersByProject.value[form.project_id] || []).map((user) => ({
    id: user.id,
    label: user.display_name || user.email,
  }));
  if (form.assignee_id && !options.some((option) => option.id === form.assignee_id)) {
    options.unshift({ id: form.assignee_id, label: '当前负责人' });
  }
  return options;
});
const dirty = computed(() => {
  if (creating.value) {
    return Boolean(form.title.trim() || form.description_markdown !== blankDescription || form.assignee_id);
  }
  const ticket = selectedTicket.value;
  if (!ticket || !canEditSelected.value) return false;
  return form.title !== ticket.title ||
    form.description_markdown !== ticket.description_markdown ||
    form.status !== ticket.status ||
    form.assignee_id !== (ticket.assignee_id || '');
});

function message(err: unknown, fallback: string) {
  return err instanceof Error ? err.message : fallback;
}

function statusLabel(value: TicketStatus) {
  return statuses.find((status) => status.value === value)?.label || value;
}

function assigneeLabel(ticket: Ticket) {
  if (!ticket.assignee_id) return '未分配';
  return userNames.value[ticket.assignee_id] || '已分配';
}

function canWriteProject(projectId: string) {
  return projects.value.some((project) => project.id === projectId && project.can_write);
}

function fillForm(ticket: Ticket) {
  form.project_id = ticket.project_id;
  form.title = ticket.title;
  form.description_markdown = ticket.description_markdown;
  form.status = ticket.status;
  form.assignee_id = ticket.assignee_id || '';
}

function resetForm() {
  form.project_id = writableProjects.value.find((project) => project.id === workspace.projectId)?.id
    || writableProjects.value[0]?.id
    || '';
  form.title = '';
  form.description_markdown = blankDescription;
  form.status = 'pending';
  form.assignee_id = '';
}

function confirmDiscard() {
  return !dirty.value || window.confirm('当前工单有未保存修改，确认放弃这些修改？');
}

function selectTicket(ticket: Ticket) {
  if (saving.value || ticket.id === selectedId.value) return;
  if (!confirmDiscard()) return;
  selectedId.value = ticket.id;
  creating.value = false;
  fillForm(ticket);
  drawerOpen.value = false;
  error.value = '';
}

function startCreate() {
  if (saving.value || !writableProjects.value.length || !confirmDiscard()) return;
  selectedId.value = '';
  creating.value = true;
  resetForm();
  drawerOpen.value = false;
  error.value = '';
}

function cancelEdit() {
  if (!confirmDiscard()) return;
  if (creating.value) {
    creating.value = false;
    resetForm();
  } else if (selectedTicket.value) {
    fillForm(selectedTicket.value);
  }
}

async function load() {
  loading.value = true;
  error.value = '';
  try {
    if (!session.user) await session.loadMe();
    const [nextProjects, nextTickets] = await Promise.all([Promise.resolve(workspace.projects), api.tickets()]);
    const memberEntries = await Promise.all(
      nextProjects.map(async (project) => [project.id, await api.projectMembers(project.id)] as const),
    );
    const nextMembersByProject = Object.fromEntries(memberEntries);
    const uniqueUsers = new Map(memberEntries.flatMap(([, members]) => members).map((user) => [user.id, user]));
    projects.value = nextProjects;
    tickets.value = nextTickets;
    users.value = [...uniqueUsers.values()];
    membersByProject.value = nextMembersByProject;
    const refreshed = nextTickets.find((ticket) => ticket.id === selectedId.value);
    if (refreshed) fillForm(refreshed);
    else if (selectedId.value) selectedId.value = '';
    if (!form.project_id) form.project_id = nextProjects.find((project) => project.can_write)?.id || '';
  } catch (err) {
    error.value = message(err, '加载工单失败');
  } finally {
    loading.value = false;
  }
}

watch(
  () => workspace.projectId,
  (projectId, previousProjectId) => {
    if (dirty.value && !confirmDiscard()) {
      if (session.user) workspace.selectProject(session.user, previousProjectId);
      return;
    }
    if (selectedTicket.value && projectId && selectedTicket.value.project_id !== projectId) {
      selectedId.value = '';
    }
    if (creating.value) resetForm();
  },
);

async function submit() {
  if (!form.project_id || !form.title.trim()) return;
  saving.value = true;
  error.value = '';
  try {
    const assignee_id = form.assignee_id || null;
    const saved = creating.value
      ? await api.createTicket({
          project_id: form.project_id,
          title: form.title.trim(),
          description_markdown: form.description_markdown,
          assignee_id,
        })
      : await api.updateTicket(selectedId.value, {
          title: form.title.trim(),
          description_markdown: form.description_markdown,
          status: form.status,
          assignee_id,
        });
    selectedId.value = saved.id;
    creating.value = false;
    await load();
  } catch (err) {
    error.value = message(err, '保存工单失败');
  } finally {
    saving.value = false;
  }
}

async function remove(ticket: Ticket) {
  if (!window.confirm(`确认删除工单“${ticket.title}”？此操作无法撤销。`)) return;
  deletingId.value = ticket.id;
  error.value = '';
  try {
    await api.deleteTicket(ticket.id);
    if (selectedId.value === ticket.id) {
      selectedId.value = '';
      creating.value = false;
      resetForm();
    }
    await load();
  } catch (err) {
    error.value = message(err, '删除工单失败');
  } finally {
    deletingId.value = '';
  }
}

function beforeUnload(event: BeforeUnloadEvent) {
  if (!dirty.value) return;
  event.preventDefault();
  event.returnValue = '';
}

onBeforeRouteLeave(() => confirmDiscard());
onMounted(() => {
  window.addEventListener('beforeunload', beforeUnload);
  void load();
});
onBeforeUnmount(() => window.removeEventListener('beforeunload', beforeUnload));
</script>

<template>
  <WorkbenchShell layout="workspace">
    <div class="page-head access-page-head">
      <div><h1>轻量工单</h1><p>集中维护问题说明、处理状态和负责人。</p></div>
      <button class="primary-button" type="button" :disabled="!writableProjects.length" @click="startCreate"><Plus :size="17" /><span>创建工单</span></button>
    </div>
    <p v-if="error" class="error access-message">{{ error }}</p>

    <section class="access-workspace resource-workspace" :class="{ 'rail-open': drawerOpen }">
      <button class="access-drawer-backdrop" type="button" aria-label="关闭工单列表" @click="drawerOpen = false" />
      <aside id="ticket-list-panel" class="access-rail resource-rail">
        <div class="resource-rail-tools">
          <label class="access-search"><Search :size="16" /><input v-model="query" type="search" placeholder="搜索工单" /></label>
          <button class="icon-button access-rail-close" type="button" title="关闭工单列表" @click="drawerOpen = false"><X :size="17" /></button>
        </div>
        <div class="resource-list-scroll">
          <p v-if="loading" class="resource-list-state">正在加载...</p>
          <p v-else-if="!filteredTickets.length" class="resource-list-state">暂无工单</p>
          <button
            v-for="ticket in filteredTickets"
            v-else
            :key="ticket.id"
            class="resource-list-item"
            :class="{ active: ticket.id === selectedId }"
            type="button"
            @click="selectTicket(ticket)"
          >
            <span class="resource-list-icon"><ClipboardList :size="17" /></span>
            <span class="resource-list-copy"><strong>{{ ticket.title }}</strong><small>{{ projectNames[ticket.project_id] || '未知项目' }} · {{ assigneeLabel(ticket) }}</small></span>
            <span class="resource-status-badge" :data-status="ticket.status">{{ statusLabel(ticket.status) }}</span>
          </button>
        </div>
        <div class="access-rail-count">{{ filteredTickets.length }} / {{ tickets.length }}</div>
      </aside>

      <main v-if="creating || selectedTicket" class="resource-detail-surface">
        <header class="access-detail-head">
          <button class="icon-button access-rail-toggle" type="button" title="打开工单列表" aria-controls="ticket-list-panel" :aria-expanded="drawerOpen" @click="drawerOpen = true"><Menu :size="18" /></button>
          <div class="access-detail-title"><strong>{{ creating ? '创建工单' : selectedTicket?.title }}</strong><span>{{ creating ? '新工单' : `${projectNames[selectedTicket?.project_id || ''] || '未知项目'} · ${statusLabel(selectedTicket?.status || 'pending')}` }}</span></div>
          <button v-if="selectedTicket && canEditSelected" class="icon-button" type="button" title="删除工单" :disabled="deletingId === selectedTicket.id" @click="remove(selectedTicket)"><Trash2 :size="17" /></button>
        </header>

        <form v-if="creating || canEditSelected" class="resource-detail-form" @submit.prevent="submit">
          <div class="resource-detail-scroll">
            <section class="access-detail-section resource-form-section">
              <div class="access-fields-grid">
                <label class="field"><span>项目</span><select v-model="form.project_id" :disabled="!creating" required><option v-for="project in writableProjects" :key="project.id" :value="project.id">{{ project.name }}</option></select></label>
                <label class="field"><span>标题</span><input v-model="form.title" required /></label>
                <label v-if="!creating" class="field"><span>状态</span><select v-model="form.status"><option v-for="status in statuses" :key="status.value" :value="status.value">{{ status.label }}</option></select></label>
                <label class="field"><span>负责人</span><select v-model="form.assignee_id"><option value="">未分配</option><option v-for="user in assigneeOptions" :key="user.id" :value="user.id">{{ user.label }}</option></select></label>
              </div>
              <div class="resource-editor-field"><span>问题说明</span><MdEditor v-model="form.description_markdown" language="zh-CN" :height="460" /></div>
            </section>
          </div>
          <footer class="access-detail-actions"><span v-if="dirty" class="access-dirty-state">有未保存修改</span><button v-if="dirty" class="secondary-button" type="button" @click="cancelEdit">取消</button><button class="primary-button" :disabled="saving || !form.project_id || !form.title.trim()">{{ saving ? '保存中...' : creating ? '创建' : '保存' }}</button></footer>
        </form>

        <div v-else class="resource-detail-scroll">
          <section class="access-detail-section resource-preview-section"><MdPreview :model-value="selectedTicket?.description_markdown || ''" /></section>
        </div>
      </main>

      <div v-else class="access-detail-empty">
        <button class="icon-button access-rail-toggle" type="button" title="打开工单列表" aria-controls="ticket-list-panel" :aria-expanded="drawerOpen" @click="drawerOpen = true"><Menu :size="18" /></button>
        <div class="workspace-selection-empty"><ClipboardList :size="24" /><strong>选择一个工单</strong><span>查看问题说明和处理状态</span></div>
      </div>
    </section>
  </WorkbenchShell>
</template>
