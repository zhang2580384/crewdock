<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import { onBeforeRouteLeave } from 'vue-router';
import { Copy, Eye, EyeOff, KeyRound, Menu, Pencil, Plus, Search, Trash2, X } from '@lucide/vue';
import WorkbenchShell from '../components/WorkbenchShell.vue';
import { api } from '../api';
import { useSessionStore } from '../stores/session';
import { useWorkspaceStore } from '../stores/workspace';
import type { ProjectListItem, VaultItem } from '../types';

const session = useSessionStore();
const workspace = useWorkspaceStore();

const projects = ref<ProjectListItem[]>([]);
const items = ref<VaultItem[]>([]);
const revealed = ref<Record<string, string>>({});
const selectedId = ref('');
const editing = ref(false);
const creating = ref(false);
const drawerOpen = ref(false);
const query = ref('');
const loading = ref(false);
const saving = ref(false);
const revealingId = ref('');
const deletingId = ref('');
const copiedId = ref('');
const error = ref('');
const form = reactive({ project_id: '', title: '', kind: 'server', username: '', url: '', secret: '', notes: '', tags: '' });

const selectedItem = computed(() => items.value.find((item) => item.id === selectedId.value) || null);
const projectNames = computed(() => Object.fromEntries(projects.value.map((project) => [project.id, project.name])));
const writableProjects = computed(() => projects.value.filter((project) => project.can_write));
const canWriteSelected = computed(() => Boolean(selectedItem.value && projects.value.some((project) => project.id === selectedItem.value?.project_id && project.can_write)));
const filteredItems = computed(() => {
  const needle = query.value.trim().toLocaleLowerCase();
  const scoped = workspace.projectId
    ? items.value.filter((item) => item.project_id === workspace.projectId)
    : items.value;
  if (!needle) return scoped;
  return scoped.filter((item) => [item.title, item.kind, item.username || '', item.url || '', projectNames.value[item.project_id] || '', ...item.tags]
    .some((value) => value.toLocaleLowerCase().includes(needle)));
});
const dirty = computed(() => {
  if (creating.value) return Boolean(form.title.trim() || form.username.trim() || form.url.trim() || form.secret || form.notes.trim() || form.tags.trim());
  const item = selectedItem.value;
  if (!editing.value || !item) return false;
  return form.title !== item.title ||
    form.kind !== item.kind ||
    form.username !== (item.username || '') ||
    form.url !== (item.url || '') ||
    Boolean(form.secret) ||
    form.notes !== (item.notes || '') ||
    form.tags !== item.tags.join(' ');
});

function message(err: unknown, fallback: string) {
  return err instanceof Error ? err.message : fallback;
}

function splitTags(value: string) {
  return value.split(/[,，\s]+/).map((tag) => tag.trim()).filter(Boolean);
}

function fillForm(item: VaultItem) {
  form.project_id = item.project_id;
  form.title = item.title;
  form.kind = item.kind;
  form.username = item.username || '';
  form.url = item.url || '';
  form.secret = '';
  form.notes = item.notes || '';
  form.tags = item.tags.join(' ');
}

function resetForm() {
  form.project_id = writableProjects.value.find((project) => project.id === workspace.projectId)?.id
    || writableProjects.value[0]?.id
    || '';
  form.title = '';
  form.kind = 'server';
  form.username = '';
  form.url = '';
  form.secret = '';
  form.notes = '';
  form.tags = '';
}

function confirmDiscard() {
  return !dirty.value || window.confirm('当前凭据有未保存修改，确认放弃这些修改？');
}

function selectItem(item: VaultItem) {
  if (saving.value || item.id === selectedId.value) return;
  if (!confirmDiscard()) return;
  selectedId.value = item.id;
  editing.value = false;
  creating.value = false;
  fillForm(item);
  drawerOpen.value = false;
  error.value = '';
}

function startCreate() {
  if (saving.value || !writableProjects.value.length || !confirmDiscard()) return;
  selectedId.value = '';
  editing.value = false;
  creating.value = true;
  resetForm();
  drawerOpen.value = false;
  error.value = '';
}

function startEdit() {
  if (!selectedItem.value || !canWriteSelected.value) return;
  fillForm(selectedItem.value);
  editing.value = true;
  creating.value = false;
}

function cancelEdit() {
  if (!confirmDiscard()) return;
  if (creating.value) {
    creating.value = false;
    resetForm();
  } else if (selectedItem.value) {
    editing.value = false;
    fillForm(selectedItem.value);
  }
}

async function load() {
  loading.value = true;
  error.value = '';
  try {
    if (!session.user) await session.loadMe();
    const [nextProjects, nextItems] = await Promise.all([Promise.resolve(workspace.projects), api.vault()]);
    projects.value = nextProjects;
    items.value = nextItems;
    const refreshed = nextItems.find((item) => item.id === selectedId.value);
    if (refreshed) fillForm(refreshed);
    else if (selectedId.value) selectedId.value = '';
    if (!form.project_id) form.project_id = nextProjects.find((project) => project.can_write)?.id || '';
  } catch (err) {
    error.value = message(err, '加载保险库失败');
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
    if (selectedItem.value && projectId && selectedItem.value.project_id !== projectId) {
      selectedId.value = '';
      editing.value = false;
    }
    if (creating.value) resetForm();
  },
);

async function submit() {
  if (!form.project_id || !form.title.trim() || !form.kind.trim()) return;
  saving.value = true;
  error.value = '';
  try {
    const common = {
      title: form.title.trim(),
      kind: form.kind.trim(),
      username: form.username.trim(),
      url: form.url.trim(),
      notes: form.notes.trim(),
      tags: splitTags(form.tags),
    };
    const saved = creating.value
      ? await api.createVault({ project_id: form.project_id, ...common, secret: form.secret })
      : await api.updateVault(selectedId.value, { ...common, secret: form.secret || undefined });
    selectedId.value = saved.id;
    editing.value = false;
    creating.value = false;
    await load();
  } catch (err) {
    error.value = message(err, '保存凭据失败');
  } finally {
    saving.value = false;
  }
}

async function reveal(item: VaultItem) {
  if (revealed.value[item.id]) {
    delete revealed.value[item.id];
    return;
  }
  revealingId.value = item.id;
  error.value = '';
  try {
    const data = await api.revealVault(item.id);
    revealed.value[item.id] = data.secret;
  } catch (err) {
    error.value = message(err, '读取敏感字段失败');
  } finally {
    revealingId.value = '';
  }
}

async function copySecret(item: VaultItem) {
  revealingId.value = item.id;
  error.value = '';
  try {
    if (!revealed.value[item.id]) {
      const data = await api.revealVault(item.id);
      revealed.value[item.id] = data.secret;
    }
    await navigator.clipboard.writeText(revealed.value[item.id]);
    copiedId.value = item.id;
    window.setTimeout(() => { if (copiedId.value === item.id) copiedId.value = ''; }, 1600);
  } catch (err) {
    error.value = message(err, '复制敏感字段失败');
  } finally {
    revealingId.value = '';
  }
}

async function remove(item: VaultItem) {
  if (!window.confirm(`确认删除凭据“${item.title}”？删除后无法恢复。`)) return;
  deletingId.value = item.id;
  error.value = '';
  try {
    await api.deleteVault(item.id);
    delete revealed.value[item.id];
    if (selectedId.value === item.id) {
      selectedId.value = '';
      editing.value = false;
      creating.value = false;
      resetForm();
    }
    await load();
  } catch (err) {
    error.value = message(err, '删除凭据失败');
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
      <div><h1>保险库</h1><p>加密保存服务器账号、API Key 和密钥。</p></div>
      <button class="primary-button" type="button" :disabled="!writableProjects.length" @click="startCreate"><Plus :size="17" /><span>新增凭据</span></button>
    </div>
    <p v-if="error" class="error access-message">{{ error }}</p>

    <section class="access-workspace resource-workspace" :class="{ 'rail-open': drawerOpen }">
      <button class="access-drawer-backdrop" type="button" aria-label="关闭凭据列表" @click="drawerOpen = false" />
      <aside id="vault-list-panel" class="access-rail resource-rail">
        <div class="resource-rail-tools">
          <label class="access-search"><Search :size="16" /><input v-model="query" type="search" placeholder="搜索凭据" /></label>
          <button class="icon-button access-rail-close" type="button" title="关闭凭据列表" @click="drawerOpen = false"><X :size="17" /></button>
        </div>
        <div class="resource-list-scroll">
          <p v-if="loading" class="resource-list-state">正在加载...</p>
          <p v-else-if="!filteredItems.length" class="resource-list-state">暂无凭据</p>
          <button v-for="item in filteredItems" v-else :key="item.id" class="resource-list-item" :class="{ active: item.id === selectedId }" type="button" @click="selectItem(item)">
            <span class="resource-list-icon"><KeyRound :size="17" /></span>
            <span class="resource-list-copy"><strong>{{ item.title }}</strong><small>{{ projectNames[item.project_id] || '未知项目' }} · {{ item.kind }}</small></span>
            <span v-if="item.tags[0]" class="resource-status-badge neutral">{{ item.tags[0] }}</span>
          </button>
        </div>
        <div class="access-rail-count">{{ filteredItems.length }} / {{ items.length }}</div>
      </aside>

      <main v-if="creating || selectedItem" class="resource-detail-surface">
        <header class="access-detail-head">
          <button class="icon-button access-rail-toggle" type="button" title="打开凭据列表" aria-controls="vault-list-panel" :aria-expanded="drawerOpen" @click="drawerOpen = true"><Menu :size="18" /></button>
          <div class="access-detail-title"><strong>{{ creating ? '新增凭据' : selectedItem?.title }}</strong><span>{{ creating ? '新加密记录' : `${projectNames[selectedItem?.project_id || ''] || '未知项目'} · ${selectedItem?.kind || ''}` }}</span></div>
          <template v-if="selectedItem && !editing">
            <button class="icon-button" type="button" :title="revealed[selectedItem.id] ? '隐藏敏感字段' : '查看敏感字段'" :disabled="revealingId === selectedItem.id" @click="reveal(selectedItem)"><component :is="revealed[selectedItem.id] ? EyeOff : Eye" :size="17" /></button>
            <button class="icon-button" type="button" :title="copiedId === selectedItem.id ? '已复制' : '复制敏感字段'" :disabled="revealingId === selectedItem.id" @click="copySecret(selectedItem)"><Copy :size="17" /></button>
            <button v-if="canWriteSelected" class="icon-button" type="button" title="编辑凭据" @click="startEdit"><Pencil :size="17" /></button>
            <button v-if="canWriteSelected" class="icon-button" type="button" title="删除凭据" :disabled="deletingId === selectedItem.id" @click="remove(selectedItem)"><Trash2 :size="17" /></button>
          </template>
        </header>

        <form v-if="creating || editing" class="resource-detail-form" @submit.prevent="submit">
          <div class="resource-detail-scroll">
            <section class="access-detail-section resource-form-section">
              <div class="access-fields-grid">
                <label class="field"><span>项目</span><select v-model="form.project_id" :disabled="!creating" required><option v-for="project in writableProjects" :key="project.id" :value="project.id">{{ project.name }}</option></select></label>
                <label class="field"><span>标题</span><input v-model="form.title" required /></label>
                <label class="field"><span>类型</span><input v-model="form.kind" required /></label>
                <label class="field"><span>账号</span><input v-model="form.username" /></label>
                <label class="field"><span>URL</span><input v-model="form.url" type="url" /></label>
                <label class="field"><span>敏感字段 <small v-if="editing">留空则保持不变</small></span><input v-model="form.secret" type="password" :required="creating" autocomplete="new-password" /></label>
                <label class="field access-description-field"><span>备注</span><textarea v-model="form.notes" /></label>
                <label class="field access-description-field"><span>标签</span><input v-model="form.tags" placeholder="逗号或空格分隔" /></label>
              </div>
            </section>
          </div>
          <footer class="access-detail-actions"><span v-if="dirty" class="access-dirty-state">有未保存修改</span><button class="secondary-button" type="button" @click="cancelEdit">取消</button><button class="primary-button" :disabled="saving || !form.project_id || !form.title.trim() || !form.kind.trim()">{{ saving ? '保存中...' : creating ? '加密保存' : '保存' }}</button></footer>
        </form>

        <div v-else-if="selectedItem" class="resource-detail-scroll">
          <section class="access-detail-section">
            <div class="access-section-head"><h2>凭据信息</h2></div>
            <div class="access-fields-grid">
              <div class="access-readonly-field"><span>账号</span><strong>{{ selectedItem.username || '未填写' }}</strong></div>
              <div class="access-readonly-field"><span>URL</span><strong>{{ selectedItem.url || '未填写' }}</strong></div>
              <div class="access-readonly-field access-description-field"><span>敏感字段</span><code class="vault-secret-display">{{ revealed[selectedItem.id] || '••••••••••••••••' }}</code></div>
              <div class="access-readonly-field access-description-field"><span>备注</span><strong>{{ selectedItem.notes || '未填写' }}</strong></div>
              <div class="access-readonly-field access-description-field"><span>标签</span><strong>{{ selectedItem.tags.join(' · ') || '无标签' }}</strong></div>
            </div>
          </section>
        </div>
      </main>

      <div v-else class="access-detail-empty">
        <button class="icon-button access-rail-toggle" type="button" title="打开凭据列表" aria-controls="vault-list-panel" :aria-expanded="drawerOpen" @click="drawerOpen = true"><Menu :size="18" /></button>
        <div class="workspace-selection-empty"><KeyRound :size="24" /><strong>选择一个凭据</strong><span>查看账号和加密字段</span></div>
      </div>
    </section>
  </WorkbenchShell>
</template>
