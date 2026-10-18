<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import { Check, Download, FileArchive, Menu, Pencil, Plus, Search, Trash2, Upload, X } from '@lucide/vue';
import WorkbenchShell from '../components/WorkbenchShell.vue';
import ProjectPicker from '../components/ProjectPicker.vue';
import { api } from '../api';
import { useSessionStore } from '../stores/session';
import { useWorkspaceStore } from '../stores/workspace';
import type { FileAsset, ProjectListItem } from '../types';

const session = useSessionStore();
const workspace = useWorkspaceStore();

const projects = ref<ProjectListItem[]>([]);
const files = ref<FileAsset[]>([]);
const selectedProject = ref('');
const selectedId = ref('');
const selectedFile = ref<File | null>(null);
const fileInput = ref<HTMLInputElement | null>(null);
const query = ref('');
const uploadMode = ref(false);
const drawerOpen = ref(false);
const loading = ref(false);
const uploading = ref(false);
const downloadingId = ref('');
const deletingId = ref('');
const editingId = ref('');
const editingFilename = ref('');
const renaming = ref(false);
const error = ref('');

const selectedItem = computed(() => files.value.find((file) => file.id === selectedId.value) || null);
const selectedProjectItem = computed(() => projects.value.find((project) => project.id === selectedProject.value) || null);
const canWriteProject = computed(() => Boolean(selectedProjectItem.value?.can_write));
const filteredFiles = computed(() => {
  const needle = query.value.trim().toLocaleLowerCase();
  if (!needle) return files.value;
  return files.value.filter((file) => [file.filename, file.content_type || '', formatSize(file.size_bytes)]
    .some((value) => value.toLocaleLowerCase().includes(needle)));
});

function message(err: unknown, fallback: string) {
  return err instanceof Error ? err.message : fallback;
}

function formatSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function fileType(file: FileAsset) {
  const extension = file.filename.split('.').pop();
  return extension && extension !== file.filename ? extension.toUpperCase() : 'FILE';
}

function contentType(file: FileAsset) {
  return file.content_type || '未知类型';
}

async function loadProjects() {
  loading.value = true;
  error.value = '';
  try {
    if (!session.user) await session.loadMe();
    projects.value = workspace.projects;
    selectedProject.value = workspace.projectId || selectedProject.value || projects.value[0]?.id || '';
    if (session.user && selectedProject.value !== workspace.projectId) {
      workspace.selectProject(session.user, selectedProject.value);
    }
    await loadFiles();
  } catch (err) {
    error.value = message(err, '加载文件失败');
  } finally {
    loading.value = false;
  }
}

async function loadFiles() {
  loading.value = true;
  error.value = '';
  cancelRename();
  try {
    files.value = selectedProject.value ? await api.files(selectedProject.value) : [];
    if (selectedId.value && !files.value.some((file) => file.id === selectedId.value)) selectedId.value = '';
  } catch (err) {
    error.value = message(err, '加载文件失败');
  } finally {
    loading.value = false;
  }
}

async function changeProject() {
  if (session.user) workspace.selectProject(session.user, selectedProject.value);
  selectedId.value = '';
  uploadMode.value = false;
  selectedFile.value = null;
  drawerOpen.value = false;
  if (fileInput.value) fileInput.value.value = '';
  await loadFiles();
}

watch(
  () => workspace.projectId,
  (projectId) => {
    if (projectId && projectId !== selectedProject.value && !loading.value) {
      selectedProject.value = projectId;
      void changeProject();
    }
  },
);

function selectItem(file: FileAsset) {
  if (renaming.value) return;
  selectedId.value = file.id;
  uploadMode.value = false;
  cancelRename();
  drawerOpen.value = false;
  error.value = '';
}

function startUpload() {
  if (!selectedProject.value || !canWriteProject.value) return;
  selectedId.value = '';
  uploadMode.value = true;
  selectedFile.value = null;
  cancelRename();
  drawerOpen.value = false;
  error.value = '';
  if (fileInput.value) fileInput.value.value = '';
}

function pickFile(event: Event) {
  selectedFile.value = (event.target as HTMLInputElement).files?.[0] ?? null;
}

async function upload() {
  if (!selectedProject.value || !selectedFile.value) return;
  uploading.value = true;
  error.value = '';
  try {
    const saved = await api.uploadFile(selectedProject.value, selectedFile.value);
    selectedFile.value = null;
    uploadMode.value = false;
    selectedId.value = saved.id;
    if (fileInput.value) fileInput.value.value = '';
    await loadFiles();
  } catch (err) {
    error.value = message(err, '上传文件失败');
  } finally {
    uploading.value = false;
  }
}

async function download(file: FileAsset) {
  downloadingId.value = file.id;
  error.value = '';
  try {
    const blob = await api.downloadFile(file.project_id, file.id);
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = file.filename;
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    URL.revokeObjectURL(url);
  } catch (err) {
    error.value = message(err, '下载文件失败');
  } finally {
    downloadingId.value = '';
  }
}

function startRename(file: FileAsset) {
  editingId.value = file.id;
  editingFilename.value = file.filename;
  error.value = '';
}

function cancelRename() {
  editingId.value = '';
  editingFilename.value = '';
}

async function saveRename(file: FileAsset) {
  const filename = editingFilename.value.trim();
  if (!filename) return;
  renaming.value = true;
  error.value = '';
  try {
    await api.updateFile(file.project_id, file.id, filename);
    cancelRename();
    await loadFiles();
  } catch (err) {
    error.value = message(err, '修改文件名失败');
  } finally {
    renaming.value = false;
  }
}

async function remove(file: FileAsset) {
  if (!window.confirm(`确认删除文件“${file.filename}”？此操作无法撤销。`)) return;
  deletingId.value = file.id;
  error.value = '';
  try {
    await api.deleteFile(file.project_id, file.id);
    selectedId.value = '';
    cancelRename();
    await loadFiles();
  } catch (err) {
    error.value = message(err, '删除文件失败');
  } finally {
    deletingId.value = '';
  }
}

onMounted(loadProjects);
</script>

<template>
  <WorkbenchShell layout="workspace">
    <div class="page-head access-page-head file-page-head">
      <div>
        <h1>文件库</h1>
        <p>按项目集中保存、下载和维护交付文件。</p>
      </div>
      <div class="page-actions file-page-actions">
        <ProjectPicker v-model="selectedProject" :projects="projects" :disabled="loading" @update:model-value="changeProject" />
        <button class="primary-button" type="button" :disabled="!selectedProject || !canWriteProject" @click="startUpload">
          <Plus :size="17" /><span>上传文件</span>
        </button>
      </div>
    </div>

    <p v-if="error" class="error access-message">{{ error }}</p>
    <section class="access-workspace resource-workspace" :class="{ 'rail-open': drawerOpen }">
      <button class="access-drawer-backdrop" type="button" aria-label="关闭文件列表" @click="drawerOpen = false" />
      <aside id="file-list-panel" class="access-rail resource-rail">
        <div class="resource-rail-tools">
          <label class="access-search"><Search :size="16" /><input v-model="query" type="search" placeholder="搜索文件" /></label>
          <button class="icon-button access-rail-close" type="button" title="关闭文件列表" @click="drawerOpen = false"><X :size="17" /></button>
        </div>
        <div class="resource-list-scroll">
          <p v-if="loading" class="resource-list-state">正在加载...</p>
          <p v-else-if="!filteredFiles.length" class="resource-list-state">当前项目暂无文件</p>
          <button v-for="file in filteredFiles" v-else :key="file.id" class="resource-list-item" :class="{ active: file.id === selectedId }" type="button" @click="selectItem(file)">
            <span class="resource-list-icon"><FileArchive :size="17" /></span>
            <span class="resource-list-copy"><strong>{{ file.filename }}</strong><small>{{ formatSize(file.size_bytes) }} · {{ new Date(file.created_at).toLocaleDateString() }}</small></span>
            <span class="resource-status-badge neutral">{{ fileType(file) }}</span>
          </button>
        </div>
        <div class="access-rail-count">{{ filteredFiles.length }} / {{ files.length }}</div>
      </aside>

      <main v-if="uploadMode || selectedItem" class="resource-detail-surface">
        <header class="access-detail-head">
          <button class="icon-button access-rail-toggle" type="button" title="打开文件列表" aria-controls="file-list-panel" :aria-expanded="drawerOpen" @click="drawerOpen = true"><Menu :size="18" /></button>
          <div class="access-detail-title">
            <strong>{{ uploadMode ? '上传文件' : selectedItem?.filename }}</strong>
            <span>{{ uploadMode ? selectedProjectItem?.name || '当前项目' : `${formatSize(selectedItem?.size_bytes || 0)} · ${contentType(selectedItem!)}` }}</span>
          </div>
          <template v-if="selectedItem && !editingId">
            <button class="icon-button" type="button" title="下载文件" :disabled="downloadingId === selectedItem.id" @click="download(selectedItem)"><Download :size="17" /></button>
            <button v-if="canWriteProject" class="icon-button" type="button" title="重命名" @click="startRename(selectedItem)"><Pencil :size="17" /></button>
            <button v-if="canWriteProject" class="icon-button" type="button" title="删除文件" :disabled="deletingId === selectedItem.id" @click="remove(selectedItem)"><Trash2 :size="17" /></button>
          </template>
        </header>

        <form v-if="uploadMode" class="resource-detail-form" @submit.prevent="upload">
          <div class="resource-detail-scroll">
            <section class="access-detail-section file-upload-section">
              <label class="file-upload-drop" :class="{ selected: selectedFile }">
                <Upload :size="24" />
                <strong>{{ selectedFile?.name || '选择要上传的文件' }}</strong>
                <span>{{ selectedFile ? formatSize(selectedFile.size) : '文件将保存到当前项目' }}</span>
                <input ref="fileInput" type="file" @change="pickFile" />
              </label>
            </section>
          </div>
          <footer class="access-detail-actions">
            <button class="secondary-button" type="button" @click="uploadMode = false">取消</button>
            <button class="primary-button" :disabled="!selectedFile || uploading"><Upload :size="17" />{{ uploading ? '上传中...' : '上传' }}</button>
          </footer>
        </form>

        <form v-else-if="selectedItem && editingId === selectedItem.id" class="resource-detail-form" @submit.prevent="saveRename(selectedItem)">
          <div class="resource-detail-scroll">
            <section class="access-detail-section resource-form-section resource-narrow-form">
              <label class="field"><span>文件名</span><input v-model="editingFilename" aria-label="文件名" required @keyup.esc="cancelRename" /></label>
            </section>
          </div>
          <footer class="access-detail-actions">
            <button class="secondary-button" type="button" :disabled="renaming" @click="cancelRename"><X :size="16" />取消</button>
            <button class="primary-button" :disabled="renaming || !editingFilename.trim()"><Check :size="16" />{{ renaming ? '保存中...' : '保存文件名' }}</button>
          </footer>
        </form>

        <div v-else-if="selectedItem" class="resource-detail-scroll">
          <section class="access-detail-section">
            <div class="access-section-head"><h2>文件信息</h2></div>
            <div class="access-fields-grid">
              <div class="access-readonly-field access-description-field"><span>文件名</span><strong>{{ selectedItem.filename }}</strong></div>
              <div class="access-readonly-field"><span>大小</span><strong>{{ formatSize(selectedItem.size_bytes) }}</strong></div>
              <div class="access-readonly-field"><span>类型</span><strong>{{ contentType(selectedItem) }}</strong></div>
              <div class="access-readonly-field"><span>上传时间</span><strong>{{ new Date(selectedItem.created_at).toLocaleString() }}</strong></div>
            </div>
          </section>
        </div>
      </main>

      <div v-else class="access-detail-empty">
        <button class="icon-button access-rail-toggle" type="button" title="打开文件列表" aria-controls="file-list-panel" :aria-expanded="drawerOpen" @click="drawerOpen = true"><Menu :size="18" /></button>
        <div class="workspace-selection-empty"><FileArchive :size="24" /><strong>选择一个文件</strong><span>查看文件信息或执行下载</span></div>
      </div>
    </section>
  </WorkbenchShell>
</template>
