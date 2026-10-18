<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue';
import { Check, Copy, Menu, Plus, Search, Share2, Trash2, X } from '@lucide/vue';
import WorkbenchShell from '../components/WorkbenchShell.vue';
import { api } from '../api';
import { useSessionStore } from '../stores/session';
import { useWorkspaceStore } from '../stores/workspace';
import type { ShareLink } from '../types';

const session = useSessionStore();
const workspace = useWorkspaceStore();

interface TargetOption {
  key: string;
  type: string;
  id: string;
  label: string;
}

const options = ref<TargetOption[]>([]);
const shares = ref<ShareLink[]>([]);
const selectedId = ref('');
const creatingMode = ref(false);
const drawerOpen = ref(false);
const query = ref('');
const loading = ref(false);
const creating = ref(false);
const deletingId = ref('');
const copied = ref(false);
const generatedUrl = ref('');
const generatedShareId = ref('');
const error = ref('');
const hasManageableProjects = ref(false);
const form = reactive({ targetKey: '', title: '', expiresAt: '' });

const selectedTarget = computed(() => options.value.find((option) => option.key === form.targetKey));
const selectedShare = computed(() => shares.value.find((share) => share.id === selectedId.value) || null);
const filteredShares = computed(() => {
  const needle = query.value.trim().toLocaleLowerCase();
  if (!needle) return shares.value;
  return shares.value.filter((share) => [share.title, typeLabel(share.target_type), expiryLabel(share)].some((value) => value.toLocaleLowerCase().includes(needle)));
});

function defaultExpiry() {
  const date = new Date(Date.now() + 7 * 24 * 60 * 60 * 1000);
  const local = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
  return local.toISOString().slice(0, 16);
}

function syncTitle() {
  if (selectedTarget.value) form.title = selectedTarget.value.label;
}

function typeLabel(type: string) {
  return ({ project: '项目', project_link: '链接', markdown_doc: '文档', file_asset: '文件', ticket: '工单' } as Record<string, string>)[type] || type;
}

function expiryLabel(share: ShareLink) {
  if (!share.expires_at) return '长期有效';
  if (new Date(share.expires_at).getTime() <= Date.now()) return '已过期';
  return `${new Date(share.expires_at).toLocaleString()} 到期`;
}

function isExpired(share: ShareLink) {
  return Boolean(share.expires_at && new Date(share.expires_at).getTime() <= Date.now());
}

function resetCreateForm() {
  form.targetKey = options.value[0]?.key || '';
  form.title = '';
  form.expiresAt = defaultExpiry();
  syncTitle();
}

function startCreate() {
  if (!hasManageableProjects.value) return;
  selectedId.value = '';
  creatingMode.value = true;
  copied.value = false;
  resetCreateForm();
  drawerOpen.value = false;
  error.value = '';
}

function selectShare(share: ShareLink) {
  selectedId.value = share.id;
  creatingMode.value = false;
  copied.value = false;
  drawerOpen.value = false;
  error.value = '';
}

async function load() {
  loading.value = true;
  error.value = '';
  try {
    if (!session.user) await session.loadMe();
    const [allProjects, tickets, nextShares] = await Promise.all([Promise.resolve(workspace.projects), api.tickets(), api.shareLinks()]);
    const projects = allProjects.filter((project) =>
      project.access_role === 'administrator' && (!workspace.projectId || project.id === workspace.projectId));
    const manageableProjectIds = new Set(projects.map((project) => project.id));
    hasManageableProjects.value = projects.length > 0;
    const resources = await Promise.all(
      projects.map(async (project) => {
        const [links, docs, files] = await Promise.all([api.links(project.id), api.docs(project.id), api.files(project.id)]);
        return { project, links, docs, files };
      }),
    );
    const nextOptions: TargetOption[] = [];
    for (const resource of resources) {
      const prefix = resource.project.name;
      nextOptions.push({ key: `project:${resource.project.id}`, type: 'project', id: resource.project.id, label: `${prefix} / 项目概览` });
      for (const link of resource.links) nextOptions.push({ key: `project_link:${link.id}`, type: 'project_link', id: link.id, label: `${prefix} / 链接 / ${link.title}` });
      for (const doc of resource.docs) nextOptions.push({ key: `markdown_doc:${doc.id}`, type: 'markdown_doc', id: doc.id, label: `${prefix} / 文档 / ${doc.title}` });
      for (const file of resource.files) nextOptions.push({ key: `file_asset:${file.id}`, type: 'file_asset', id: file.id, label: `${prefix} / 文件 / ${file.filename}` });
    }
    for (const ticket of tickets.filter((ticket) => manageableProjectIds.has(ticket.project_id))) {
      const projectName = projects.find((project) => project.id === ticket.project_id)?.name || '项目';
      nextOptions.push({ key: `ticket:${ticket.id}`, type: 'ticket', id: ticket.id, label: `${projectName} / 工单 / ${ticket.title}` });
    }
    options.value = nextOptions;
    shares.value = nextShares;
    if (selectedId.value && !nextShares.some((share) => share.id === selectedId.value)) selectedId.value = '';
    if (!form.targetKey || !nextOptions.some((option) => option.key === form.targetKey)) resetCreateForm();
    if (!nextShares.length && hasManageableProjects.value && !selectedId.value) creatingMode.value = true;
  } catch (err) {
    error.value = err instanceof Error ? err.message : '加载分享数据失败';
  } finally {
    loading.value = false;
  }
}

watch(() => workspace.projectId, () => {
  void load();
});

async function createShare() {
  const target = selectedTarget.value;
  if (!target) return;
  creating.value = true;
  error.value = '';
  copied.value = false;
  try {
    const result = await api.createShareLink({
      target_type: target.type,
      target_id: target.id,
      title: form.title.trim() || target.label,
      expires_at: form.expiresAt ? new Date(form.expiresAt).toISOString() : null,
    });
    generatedUrl.value = result.url;
    generatedShareId.value = result.share.id;
    await navigator.clipboard.writeText(result.url);
    copied.value = true;
    shares.value = await api.shareLinks();
    selectedId.value = result.share.id;
    creatingMode.value = false;
  } catch (err) {
    error.value = err instanceof Error ? err.message : '创建分享链接失败';
  } finally {
    creating.value = false;
  }
}

async function copyGenerated() {
  if (!generatedUrl.value || generatedShareId.value !== selectedId.value) return;
  await navigator.clipboard.writeText(generatedUrl.value);
  copied.value = true;
}

async function removeShare(share: ShareLink) {
  if (!window.confirm(`确认撤销分享“${share.title}”？原链接将立即失效。`)) return;
  deletingId.value = share.id;
  error.value = '';
  try {
    await api.deleteShareLink(share.id);
    shares.value = shares.value.filter((item) => item.id !== share.id);
    if (selectedId.value === share.id) selectedId.value = '';
    if (generatedShareId.value === share.id) {
      generatedShareId.value = '';
      generatedUrl.value = '';
    }
  } catch (err) {
    error.value = err instanceof Error ? err.message : '撤销分享失败';
  } finally {
    deletingId.value = '';
  }
}

onMounted(load);
</script>

<template>
  <WorkbenchShell layout="workspace">
    <div class="page-head access-page-head">
      <div><h1>内部分享</h1><p>为项目资源生成有时效的访问链接，并随时撤销。</p></div>
      <button class="primary-button" type="button" :disabled="!hasManageableProjects" @click="startCreate"><Plus :size="17" /><span>创建分享</span></button>
    </div>
    <p v-if="error" class="error access-message">{{ error }}</p>

    <section class="access-workspace resource-workspace" :class="{ 'rail-open': drawerOpen }">
      <button class="access-drawer-backdrop" type="button" aria-label="关闭分享列表" @click="drawerOpen = false" />
      <aside id="share-list-panel" class="access-rail resource-rail">
        <div class="resource-rail-tools">
          <label class="access-search"><Search :size="16" /><input v-model="query" type="search" placeholder="搜索分享" /></label>
          <button class="icon-button access-rail-close" type="button" title="关闭分享列表" @click="drawerOpen = false"><X :size="17" /></button>
        </div>
        <div class="resource-list-scroll">
          <p v-if="loading" class="resource-list-state">正在加载...</p>
          <p v-else-if="!filteredShares.length" class="resource-list-state">暂无分享</p>
          <button v-for="share in filteredShares" v-else :key="share.id" class="resource-list-item" :class="{ active: share.id === selectedId }" type="button" @click="selectShare(share)">
            <span class="resource-list-icon"><Share2 :size="17" /></span>
            <span class="resource-list-copy"><strong>{{ share.title }}</strong><small>{{ typeLabel(share.target_type) }} · {{ expiryLabel(share) }}</small></span>
            <span class="resource-status-badge" :class="{ expired: isExpired(share) }">{{ isExpired(share) ? '过期' : '有效' }}</span>
          </button>
        </div>
        <div class="access-rail-count">{{ filteredShares.length }} / {{ shares.length }}</div>
      </aside>

      <main v-if="creatingMode || selectedShare" class="resource-detail-surface">
        <header class="access-detail-head">
          <button class="icon-button access-rail-toggle" type="button" title="打开分享列表" aria-controls="share-list-panel" :aria-expanded="drawerOpen" @click="drawerOpen = true"><Menu :size="18" /></button>
          <div class="access-detail-title"><strong>{{ creatingMode ? '创建分享' : selectedShare?.title }}</strong><span>{{ creatingMode ? '新分享链接' : `${typeLabel(selectedShare?.target_type || '')} · ${selectedShare ? expiryLabel(selectedShare) : ''}` }}</span></div>
          <button v-if="selectedShare" class="icon-button" type="button" title="撤销分享" :disabled="deletingId === selectedShare.id" @click="removeShare(selectedShare)"><Trash2 :size="17" /></button>
        </header>

        <form v-if="creatingMode" class="resource-detail-form" @submit.prevent="createShare">
          <div class="resource-detail-scroll">
            <section class="access-detail-section resource-form-section resource-narrow-form">
              <label class="field"><span>分享内容</span><select v-model="form.targetKey" required @change="syncTitle"><option v-for="option in options" :key="option.key" :value="option.key">{{ option.label }}</option></select></label>
              <label class="field"><span>链接标题</span><input v-model="form.title" required /></label>
              <label class="field"><span>过期时间</span><input v-model="form.expiresAt" type="datetime-local" /></label>
            </section>
          </div>
          <footer class="access-detail-actions"><button class="primary-button" :disabled="creating || !selectedTarget"><Share2 :size="17" />{{ creating ? '生成中...' : '生成并复制' }}</button></footer>
        </form>

        <div v-else-if="selectedShare" class="resource-detail-scroll">
          <section class="access-detail-section">
            <div class="access-section-head"><h2>分享信息</h2></div>
            <div class="access-fields-grid">
              <div class="access-readonly-field"><span>标题</span><strong>{{ selectedShare.title }}</strong></div>
              <div class="access-readonly-field"><span>资源类型</span><strong>{{ typeLabel(selectedShare.target_type) }}</strong></div>
              <div class="access-readonly-field"><span>创建时间</span><strong>{{ new Date(selectedShare.created_at).toLocaleString() }}</strong></div>
              <div class="access-readonly-field"><span>有效期</span><strong>{{ expiryLabel(selectedShare) }}</strong></div>
            </div>
          </section>
          <section v-if="generatedShareId === selectedShare.id" class="access-detail-section">
            <div class="access-section-head"><h2>分享链接</h2></div>
            <div class="generated-link"><input :value="generatedUrl" readonly /><button class="icon-button" type="button" :title="copied ? '已复制' : '复制链接'" @click="copyGenerated"><Check v-if="copied" :size="17" /><Copy v-else :size="17" /></button></div>
          </section>
        </div>
      </main>

      <div v-else class="access-detail-empty">
        <button class="icon-button access-rail-toggle" type="button" title="打开分享列表" aria-controls="share-list-panel" :aria-expanded="drawerOpen" @click="drawerOpen = true"><Menu :size="18" /></button>
        <div class="workspace-selection-empty"><Share2 :size="24" /><strong>选择一个分享</strong><span>查看有效期或撤销访问</span></div>
      </div>
    </section>
  </WorkbenchShell>
</template>

<style scoped>
.generated-link { min-width: 0; display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 8px; }
.generated-link input { min-width: 0; min-height: 40px; padding: 8px 10px; border: 1px solid var(--line-strong); border-radius: 6px; }
</style>
