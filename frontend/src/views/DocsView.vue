<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import { onBeforeRouteLeave } from 'vue-router';
import {
  Building2,
  ChevronDown,
  ChevronRight,
  EllipsisVertical,
  Eye,
  FilePlus2,
  FileText,
  Folder,
  FolderOpen,
  FolderPlus,
  Menu,
  Maximize2,
  Minimize2,
  Pencil,
  Search,
  ShieldCheck,
  WrapText,
  X,
} from '@lucide/vue';
import { MdEditor, MdPreview } from 'md-editor-v3';
import 'md-editor-v3/lib/style.css';
import WorkbenchShell from '../components/WorkbenchShell.vue';
import ProjectPicker from '../components/ProjectPicker.vue';
import DocTreeContextMenu from '../components/DocTreeContextMenu.vue';
import { api } from '../api';
import { useSessionStore } from '../stores/session';
import { useWorkspaceStore } from '../stores/workspace';
import type {
  Department,
  MarkdownAccessRole,
  MarkdownDepartmentGrant,
  MarkdownDoc,
  MarkdownFolder,
  ProjectListItem,
} from '../types';

type TreeRow = {
  kind: 'folder' | 'doc';
  id: string;
  parentId: string | null;
  name: string;
  depth: number;
  sortOrder: number;
  hasChildren: boolean;
  folder?: MarkdownFolder;
  doc?: MarkdownDoc;
};

type MenuState = { row: TreeRow; x: number; y: number };
type DialogAction = 'new-folder' | 'rename' | 'move';

const session = useSessionStore();
const workspace = useWorkspaceStore();

const projects = ref<ProjectListItem[]>([]);
const folders = ref<MarkdownFolder[]>([]);
const docs = ref<MarkdownDoc[]>([]);
const departments = ref<Department[]>([]);
const selectedProject = ref('');
const selectedDocId = ref('');
const expandedFolders = ref(new Set<string>());
const query = ref('');
const mode = ref<'preview' | 'edit'>('preview');
const creatingDoc = ref(false);
const newDocFolderId = ref<string | null>(null);
const loading = ref(false);
const saving = ref(false);
const moving = ref(false);
const error = ref('');
const treeDrawerOpen = ref(false);
const isNarrow = ref(false);
const fullscreen = ref(false);
const softWrap = ref(true);
const contextMenu = ref<MenuState | null>(null);
const dragging = ref<{ kind: 'folder' | 'doc'; id: string } | null>(null);
const dropTarget = ref<{ id: string; position: 'before' | 'inside' | 'after' } | null>(null);
const blankBody = '## 记录\n\n';
const form = reactive({ title: '', body_markdown: blankBody, tags: '' });
const dialog = reactive({
  open: false,
  action: 'new-folder' as DialogAction,
  kind: 'folder' as 'folder' | 'doc',
  id: '',
  name: '',
  parentId: '',
});
let treeRequestId = 0;
let docsMediaQuery: MediaQueryList | null = null;

watch(fullscreen, (enabled) => {
  document.body.classList.toggle('docs-fullscreen-open', enabled);
});
const permissionDialog = reactive({
  open: false,
  loading: false,
  saving: false,
  kind: 'folder' as 'folder' | 'doc',
  id: '',
  name: '',
  custom: false,
});
const permissionRoles = ref<Record<string, MarkdownAccessRole>>({});

const selectedDoc = computed(() => docs.value.find((doc) => doc.id === selectedDocId.value) || null);
const selectedProjectRecord = computed(() => projects.value.find((project) => project.id === selectedProject.value) || null);
const canWrite = computed(() => selectedProjectRecord.value?.can_write === true);
const canManageAccess = computed(() => selectedProjectRecord.value?.access_role === 'administrator');
const selectedDocCanWrite = computed(() => selectedDoc.value?.can_write === true);
const folderById = computed(() => new Map(folders.value.map((folder) => [folder.id, folder])));

const permissionDepartmentRows = computed(() => {
  const children = new Map<string, Department[]>();
  for (const department of departments.value) {
    const key = department.parent_id || '';
    children.set(key, [...(children.get(key) || []), department]);
  }
  for (const rows of children.values()) rows.sort((a, b) => a.sort_order - b.sort_order || a.name.localeCompare(b.name, 'zh-CN'));
  const result: Array<{ department: Department; depth: number }> = [];
  const visited = new Set<string>();
  function append(parentId: string, depth: number) {
    for (const department of children.get(parentId) || []) {
      if (visited.has(department.id)) continue;
      visited.add(department.id);
      result.push({ department, depth });
      append(department.id, depth + 1);
    }
  }
  append('', 0);
  for (const department of departments.value) {
    if (!visited.has(department.id)) result.push({ department, depth: 0 });
  }
  return result;
});

const selectedPermissionCount = computed(() => Object.keys(permissionRoles.value).length);

const dirty = computed(() => {
  if (mode.value !== 'edit') return false;
  if (creatingDoc.value) {
    return Boolean(form.title.trim() || form.tags.trim() || form.body_markdown !== blankBody);
  }
  const doc = selectedDoc.value;
  if (!doc) return false;
  return form.title !== doc.title
    || form.body_markdown !== doc.body_markdown
    || form.tags !== doc.tags.join(' ');
});

const visibleRows = computed<TreeRow[]>(() => {
  const foldersByParent = new Map<string, MarkdownFolder[]>();
  const docsByFolder = new Map<string, MarkdownDoc[]>();
  for (const folder of folders.value) {
    const key = folder.parent_id || '';
    foldersByParent.set(key, [...(foldersByParent.get(key) || []), folder]);
  }
  for (const doc of docs.value) {
    const key = doc.folder_id || '';
    docsByFolder.set(key, [...(docsByFolder.get(key) || []), doc]);
  }
  const sortFolders = (items: MarkdownFolder[]) => items.sort((a, b) => a.sort_order - b.sort_order || a.name.localeCompare(b.name));
  const sortDocs = (items: MarkdownDoc[]) => items.sort((a, b) => a.sort_order - b.sort_order || a.title.localeCompare(b.title));
  const needle = query.value.trim().toLocaleLowerCase();
  const matchingFolders = new Map<string, boolean>();

  function folderMatches(folder: MarkdownFolder, trail = new Set<string>()): boolean {
    if (trail.has(folder.id)) return false;
    if (matchingFolders.has(folder.id)) return matchingFolders.get(folder.id) || false;
    const nextTrail = new Set(trail).add(folder.id);
    const ownMatch = folder.name.toLocaleLowerCase().includes(needle);
    const docMatch = (docsByFolder.get(folder.id) || []).some((doc) => doc.title.toLocaleLowerCase().includes(needle));
    const childMatch = (foldersByParent.get(folder.id) || []).some((child) => folderMatches(child, nextTrail));
    const result = ownMatch || docMatch || childMatch;
    matchingFolders.set(folder.id, result);
    return result;
  }

  const rows: TreeRow[] = [];
  function append(parentId: string | null, depth: number, trail = new Set<string>()) {
    const key = parentId || '';
    for (const folder of sortFolders([...(foldersByParent.get(key) || [])])) {
      if (trail.has(folder.id) || (needle && !folderMatches(folder))) continue;
      const children = foldersByParent.get(folder.id) || [];
      const childDocs = docsByFolder.get(folder.id) || [];
      rows.push({
        kind: 'folder',
        id: folder.id,
        parentId: folder.parent_id || null,
        name: folder.name,
        depth,
        sortOrder: folder.sort_order,
        hasChildren: children.length > 0 || childDocs.length > 0,
        folder,
      });
      if (needle || expandedFolders.value.has(folder.id)) {
        append(folder.id, depth + 1, new Set(trail).add(folder.id));
      }
    }
    for (const doc of sortDocs([...(docsByFolder.get(key) || [])])) {
      if (needle && !doc.title.toLocaleLowerCase().includes(needle)) continue;
      rows.push({
        kind: 'doc',
        id: doc.id,
        parentId: doc.folder_id || null,
        name: doc.title,
        depth,
        sortOrder: doc.sort_order,
        hasChildren: false,
        doc,
      });
    }
  }
  append(null, 0);
  return rows;
});

const folderOptions = computed(() => {
  const children = new Map<string, MarkdownFolder[]>();
  for (const folder of folders.value) {
    const key = folder.parent_id || '';
    children.set(key, [...(children.get(key) || []), folder]);
  }
  const result: Array<{ id: string; label: string; depth: number }> = [];
  function append(parentId: string | null, depth: number, trail = new Set<string>()) {
    const items = [...(children.get(parentId || '') || [])].sort((a, b) => a.sort_order - b.sort_order || a.name.localeCompare(b.name));
    for (const folder of items) {
      if (trail.has(folder.id)) continue;
      if (folder.can_write) result.push({ id: folder.id, label: folder.name, depth });
      append(folder.id, depth + 1, new Set(trail).add(folder.id));
    }
  }
  append(null, 0);
  return result;
});

const dialogTitle = computed(() => {
  if (dialog.action === 'new-folder') return dialog.parentId ? '新建子文件夹' : '新建文件夹';
  if (dialog.action === 'rename') return dialog.kind === 'folder' ? '重命名文件夹' : '重命名文档';
  return dialog.kind === 'folder' ? '移动文件夹' : '移动文档';
});

function message(err: unknown, fallback: string) {
  return err instanceof Error ? err.message : fallback;
}

function splitTags(value: string) {
  return value.split(/[,，\s]+/).map((tag) => tag.trim()).filter(Boolean);
}

function rowCanWrite(row: TreeRow) {
  return row.kind === 'folder' ? row.folder?.can_write === true : row.doc?.can_write === true;
}

function canWriteFolder(folderId: string | null) {
  return folderId ? folderById.value.get(folderId)?.can_write === true : canWrite.value;
}

function confirmDiscard() {
  return !dirty.value || window.confirm('当前修改尚未保存，确认放弃这些修改吗？');
}

function beforeUnload(event: BeforeUnloadEvent) {
  if (!dirty.value) return;
  event.preventDefault();
  event.returnValue = '';
}

async function loadProjects() {
  loading.value = true;
  error.value = '';
  try {
    if (!session.user) await session.loadMe();
    projects.value = workspace.projects;
    selectedProject.value = workspace.projectId || projects.value[0]?.id || '';
    if (session.user && selectedProject.value !== workspace.projectId) {
      workspace.selectProject(session.user, selectedProject.value);
    }
    await loadTree();
  } catch (err) {
    error.value = message(err, '加载文档失败');
  } finally {
    loading.value = false;
  }
}

async function loadTree() {
  const projectId = selectedProject.value;
  const requestId = ++treeRequestId;
  if (!projectId) {
    folders.value = [];
    docs.value = [];
    return;
  }
  const [nextFolders, nextDocs] = await Promise.all([
    api.folders(projectId),
    api.docs(projectId),
  ]);
  if (requestId !== treeRequestId || projectId !== selectedProject.value) return;
  folders.value = nextFolders;
  docs.value = nextDocs;
  if (selectedDocId.value && !nextDocs.some((doc) => doc.id === selectedDocId.value)) clearWorkspace();
}

async function refreshTree(fallback: string) {
  loading.value = true;
  error.value = '';
  try {
    await loadTree();
  } catch (err) {
    error.value = message(err, fallback);
  } finally {
    loading.value = false;
  }
}

async function changeProject(projectId: string, persist = true) {
  if (saving.value || projectId === selectedProject.value) return;
  if (!confirmDiscard()) {
    if (!persist && session.user) workspace.selectProject(session.user, selectedProject.value);
    return;
  }
  selectedProject.value = projectId;
  if (persist && session.user) workspace.selectProject(session.user, projectId);
  expandedFolders.value = new Set();
  clearWorkspace();
  folders.value = [];
  docs.value = [];
  await refreshTree('加载文档失败');
}

watch(
  () => workspace.projectId,
  (projectId) => {
    if (projectId && projectId !== selectedProject.value && !loading.value) {
      void changeProject(projectId, false);
    }
  },
);

function clearWorkspace() {
  selectedDocId.value = '';
  creatingDoc.value = false;
  newDocFolderId.value = null;
  mode.value = 'preview';
  form.title = '';
  form.body_markdown = blankBody;
  form.tags = '';
}

function fillForm(doc: MarkdownDoc) {
  form.title = doc.title;
  form.body_markdown = doc.body_markdown;
  form.tags = doc.tags.join(' ');
}

function openDoc(doc: MarkdownDoc, nextMode: 'preview' | 'edit' = 'preview') {
  if (saving.value) return;
  const isCurrent = doc.id === selectedDocId.value && !creatingDoc.value;
  if (isCurrent && nextMode === mode.value) return;
  if (!confirmDiscard()) return;
  selectedDocId.value = doc.id;
  creatingDoc.value = false;
  newDocFolderId.value = null;
  fillForm(doc);
  mode.value = nextMode;
  treeDrawerOpen.value = false;
  error.value = '';
}

function startNewDoc(folderId: string | null) {
  if (saving.value || !canWriteFolder(folderId) || !confirmDiscard()) return;
  selectedDocId.value = '';
  creatingDoc.value = true;
  newDocFolderId.value = folderId;
  form.title = '';
  form.body_markdown = blankBody;
  form.tags = '';
  mode.value = 'edit';
  treeDrawerOpen.value = false;
  error.value = '';
}

function beginEdit() {
  if (saving.value || !selectedDocCanWrite.value || !selectedDoc.value || mode.value === 'edit') return;
  fillForm(selectedDoc.value);
  mode.value = 'edit';
}

function cancelEdit() {
  if (saving.value) return;
  if (creatingDoc.value) {
    if (confirmDiscard()) clearWorkspace();
    return;
  }
  if (!selectedDoc.value || !confirmDiscard()) return;
  fillForm(selectedDoc.value);
  mode.value = 'preview';
}

async function saveDoc() {
  if (saving.value || !selectedProject.value || !form.title.trim()) return;
  if (creatingDoc.value ? !canWriteFolder(newDocFolderId.value) : !selectedDocCanWrite.value) return;
  saving.value = true;
  error.value = '';
  try {
    const payload = { title: form.title.trim(), body_markdown: form.body_markdown, tags: splitTags(form.tags) };
    const saved = creatingDoc.value
      ? await api.createDoc(selectedProject.value, { ...payload, folder_id: newDocFolderId.value || undefined })
      : await api.updateDoc(selectedProject.value, selectedDocId.value, payload);
    await loadTree();
    selectedDocId.value = saved.id;
    creatingDoc.value = false;
    newDocFolderId.value = null;
    const current = docs.value.find((doc) => doc.id === saved.id) || saved;
    fillForm(current);
    mode.value = 'preview';
  } catch (err) {
    error.value = message(err, '保存文档失败');
  } finally {
    saving.value = false;
  }
}

function toggleFolder(id: string) {
  const next = new Set(expandedFolders.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  expandedFolders.value = next;
}

function folderBreadcrumb(folderId?: string | null) {
  const names: string[] = [];
  const visited = new Set<string>();
  let currentId = folderId || '';
  while (currentId && !visited.has(currentId)) {
    visited.add(currentId);
    const folder = folderById.value.get(currentId);
    if (!folder) break;
    names.unshift(folder.name);
    currentId = folder.parent_id || '';
  }
  return names;
}

function showContext(row: TreeRow, event: MouseEvent) {
  if (row.kind === 'folder' && !rowCanWrite(row) && !canManageAccess.value) return;
  event.preventDefault();
  contextMenu.value = { row, x: event.clientX, y: event.clientY };
}

function showActionMenu(row: TreeRow, event: MouseEvent) {
  event.stopPropagation();
  const button = event.currentTarget as HTMLElement;
  const rect = button.getBoundingClientRect();
  contextMenu.value = { row, x: rect.right - 8, y: rect.bottom + 4 };
}

function openDialog(action: DialogAction, row?: TreeRow) {
  if (saving.value || (row ? !rowCanWrite(row) : !canWrite.value)) return;
  if (row?.kind === 'doc' && row.id === selectedDocId.value && dirty.value) {
    if (!confirmDiscard()) return;
    if (selectedDoc.value) fillForm(selectedDoc.value);
    mode.value = 'preview';
  }
  dialog.open = true;
  dialog.action = action;
  dialog.kind = row?.kind || 'folder';
  dialog.id = row?.id || '';
  dialog.name = action === 'rename' ? row?.name || '' : '';
  dialog.parentId = action === 'new-folder'
    ? row?.kind === 'folder' ? row.id : ''
    : row?.parentId || '';
}

async function openPermissionDialog(row: TreeRow) {
  if (!canManageAccess.value || !selectedProject.value) return;
  permissionDialog.open = true;
  permissionDialog.loading = true;
  permissionDialog.saving = false;
  permissionDialog.kind = row.kind;
  permissionDialog.id = row.id;
  permissionDialog.name = row.name;
  permissionDialog.custom = false;
  permissionRoles.value = {};
  error.value = '';
  try {
    const [nextDepartments, grants] = await Promise.all([
      api.departments(),
      row.kind === 'folder'
        ? api.folderPermissions(selectedProject.value, row.id)
        : api.docPermissions(selectedProject.value, row.id),
    ]);
    if (!permissionDialog.open || permissionDialog.id !== row.id) return;
    departments.value = nextDepartments;
    permissionRoles.value = Object.fromEntries(
      grants.map((grant: MarkdownDepartmentGrant) => [grant.department_id, grant.access_role]),
    );
    permissionDialog.custom = grants.length > 0;
  } catch (err) {
    error.value = message(err, '加载文档权限失败');
    permissionDialog.open = false;
  } finally {
    permissionDialog.loading = false;
  }
}

function togglePermissionDepartment(departmentId: string, checked: boolean) {
  const next = { ...permissionRoles.value };
  if (checked) next[departmentId] = next[departmentId] || 'viewer';
  else delete next[departmentId];
  permissionRoles.value = next;
}

function setPermissionRole(departmentId: string, accessRole: MarkdownAccessRole) {
  permissionRoles.value = { ...permissionRoles.value, [departmentId]: accessRole };
}

function changePermissionRole(departmentId: string, event: Event) {
  setPermissionRole(departmentId, (event.target as HTMLSelectElement).value as MarkdownAccessRole);
}

async function savePermissions() {
  if (!permissionDialog.open || permissionDialog.loading || permissionDialog.saving || !selectedProject.value) return;
  if (permissionDialog.custom && selectedPermissionCount.value === 0) return;
  permissionDialog.saving = true;
  error.value = '';
  try {
    const grants = permissionDialog.custom
      ? Object.entries(permissionRoles.value).map(([department_id, access_role]) => ({ department_id, access_role }))
      : [];
    if (permissionDialog.kind === 'folder') {
      await api.replaceFolderPermissions(selectedProject.value, permissionDialog.id, grants);
    } else {
      await api.replaceDocPermissions(selectedProject.value, permissionDialog.id, grants);
    }
    permissionDialog.open = false;
    await loadTree();
  } catch (err) {
    error.value = message(err, '保存文档权限失败');
  } finally {
    permissionDialog.saving = false;
  }
}

function isFolderDescendant(candidateId: string, folderId: string) {
  const visited = new Set<string>();
  let currentId = candidateId;
  while (currentId && !visited.has(currentId)) {
    if (currentId === folderId) return true;
    visited.add(currentId);
    currentId = folderById.value.get(currentId)?.parent_id || '';
  }
  return false;
}

function moveOptions() {
  if (dialog.kind !== 'folder' || dialog.action !== 'move') return folderOptions.value;
  return folderOptions.value.filter((option) => !isFolderDescendant(option.id, dialog.id));
}

async function submitDialog() {
  if (saving.value || !selectedProject.value) return;
  const currentRow = visibleRows.value.find((row) => row.kind === dialog.kind && row.id === dialog.id);
  if (dialog.action !== 'new-folder' && (!currentRow || !rowCanWrite(currentRow))) return;
  if (dialog.action === 'new-folder' && !canWriteFolder(dialog.parentId || null)) return;
  const preserveDraft = dirty.value;
  error.value = '';
  try {
    if (dialog.action === 'new-folder') {
      if (!dialog.name.trim()) return;
      await api.createFolder(selectedProject.value, {
        parent_id: dialog.parentId || undefined,
        name: dialog.name.trim(),
      });
      if (dialog.parentId) {
        expandedFolders.value = new Set(expandedFolders.value).add(dialog.parentId);
      }
    } else if (dialog.action === 'rename') {
      if (!dialog.name.trim()) return;
      if (dialog.kind === 'folder') {
        await api.updateFolder(selectedProject.value, dialog.id, dialog.name.trim());
      } else {
        const doc = docs.value.find((item) => item.id === dialog.id);
        if (!doc) return;
        await api.updateDoc(selectedProject.value, doc.id, {
          title: dialog.name.trim(),
          body_markdown: doc.body_markdown,
          tags: doc.tags,
        });
      }
    } else if (dialog.kind === 'folder') {
      await api.moveFolder(selectedProject.value, dialog.id, {
        parent_id: dialog.parentId || undefined,
        position: 1_000_000,
      });
    } else {
      await api.moveDoc(selectedProject.value, dialog.id, {
        folder_id: dialog.parentId || undefined,
        position: 1_000_000,
      });
    }
    dialog.open = false;
    await loadTree();
    if (selectedDoc.value && !preserveDraft) fillForm(selectedDoc.value);
  } catch (err) {
    error.value = message(err, '操作文档目录失败');
  }
}

async function removeFolder(row: TreeRow) {
  if (saving.value || !rowCanWrite(row)) return;
  if (!window.confirm(`确认删除空文件夹“${row.name}”？`)) return;
  try {
    await api.deleteFolder(selectedProject.value, row.id);
    await loadTree();
  } catch (err) {
    error.value = message(err, '删除文件夹失败');
  }
}

async function removeDoc(row: TreeRow) {
  if (saving.value || !rowCanWrite(row)) return;
  if (selectedDocId.value === row.id && !confirmDiscard()) return;
  if (!window.confirm(`确认删除文档“${row.name}”？此操作无法撤销。`)) return;
  try {
    await api.deleteDoc(selectedProject.value, row.id);
    if (selectedDocId.value === row.id) clearWorkspace();
    await loadTree();
  } catch (err) {
    error.value = message(err, '删除文档失败');
  }
}

function handleContextAction(action: 'preview' | 'edit' | 'new-folder' | 'new-doc' | 'rename' | 'move' | 'permissions' | 'delete') {
  const row = contextMenu.value?.row;
  if (!row) return;
  if (action === 'permissions') {
    void openPermissionDialog(row);
    return;
  }
  if (action !== 'preview' && !rowCanWrite(row)) return;
  if (action === 'preview' && row.doc) openDoc(row.doc, 'preview');
  else if (action === 'edit' && row.doc) openDoc(row.doc, 'edit');
  else if (action === 'new-doc') startNewDoc(row.kind === 'folder' ? row.id : row.parentId);
  else if (action === 'new-folder') openDialog('new-folder', row);
  else if (action === 'rename') openDialog('rename', row);
  else if (action === 'move') openDialog('move', row);
  else if (action === 'delete' && row.kind === 'folder') void removeFolder(row);
  else if (action === 'delete') void removeDoc(row);
}

function dragStart(row: TreeRow, event: DragEvent) {
  if (saving.value || !rowCanWrite(row)) return;
  dragging.value = { kind: row.kind, id: row.id };
  event.dataTransfer?.setData('text/plain', `${row.kind}:${row.id}`);
  if (event.dataTransfer) event.dataTransfer.effectAllowed = 'move';
}

function dragOver(row: TreeRow, event: DragEvent) {
  if (!dragging.value || dragging.value.id === row.id) return;
  if (dragging.value.kind !== row.kind) {
    if (dragging.value.kind !== 'doc' || row.kind !== 'folder') return;
    if (!rowCanWrite(row)) return;
    event.preventDefault();
    dropTarget.value = { id: row.id, position: 'inside' };
    return;
  }
  if (dragging.value.kind === 'folder' && row.kind === 'folder' && isFolderDescendant(row.id, dragging.value.id)) return;
  event.preventDefault();
  const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
  const ratio = (event.clientY - rect.top) / Math.max(rect.height, 1);
  const position = row.kind === 'folder' && ratio > .25 && ratio < .75
    ? 'inside'
    : ratio < .5 ? 'before' : 'after';
  if (position === 'inside' ? !rowCanWrite(row) : !canWriteFolder(row.parentId)) return;
  dropTarget.value = { id: row.id, position };
}

async function dropOnRow(row: TreeRow, event: DragEvent) {
  event.preventDefault();
  const source = dragging.value;
  const target = dropTarget.value;
  dragging.value = null;
  dropTarget.value = null;
  if (!source || source.id === row.id || !target) return;
  let parentId = row.parentId;
  let position = 1_000_000;
  if (target.position === 'inside' && row.kind === 'folder') {
    parentId = row.id;
    expandedFolders.value = new Set(expandedFolders.value).add(row.id);
  } else if (source.kind === row.kind) {
    position = row.sortOrder + (target.position === 'after' ? 1 : 0);
  }
  await moveTreeItem(source, parentId, position);
}

async function dropOnRoot(event: DragEvent) {
  event.preventDefault();
  const source = dragging.value;
  dragging.value = null;
  dropTarget.value = null;
  if (source) await moveTreeItem(source, null, 1_000_000);
}

async function moveTreeItem(source: { kind: 'folder' | 'doc'; id: string }, parentId: string | null, position: number) {
  if (saving.value || !selectedProject.value || moving.value || !canWriteFolder(parentId)) return;
  const sourceRow = visibleRows.value.find((row) => row.kind === source.kind && row.id === source.id);
  if (!sourceRow || !rowCanWrite(sourceRow)) return;
  moving.value = true;
  error.value = '';
  try {
    if (source.kind === 'folder') {
      await api.moveFolder(selectedProject.value, source.id, { parent_id: parentId || undefined, position });
    } else {
      await api.moveDoc(selectedProject.value, source.id, { folder_id: parentId || undefined, position });
    }
    await loadTree();
  } catch (err) {
    error.value = message(err, '移动失败');
    await loadTree();
  } finally {
    moving.value = false;
  }
}

function dragEnd() {
  dragging.value = null;
  dropTarget.value = null;
}

function syncNarrowViewport(event?: MediaQueryListEvent) {
  isNarrow.value = event?.matches ?? docsMediaQuery?.matches ?? false;
  if (!isNarrow.value) treeDrawerOpen.value = false;
}

function onWindowKeydown(event: KeyboardEvent) {
  if (event.key !== 'Escape') return;
  if (permissionDialog.open && !permissionDialog.saving) permissionDialog.open = false;
  else if (dialog.open) dialog.open = false;
  else if (treeDrawerOpen.value) treeDrawerOpen.value = false;
  else if (fullscreen.value) fullscreen.value = false;
}

function toggleFullscreen() {
  fullscreen.value = !fullscreen.value;
}

function toggleSoftWrap() {
  softWrap.value = !softWrap.value;
  window.localStorage.setItem('crewdock-doc-soft-wrap', softWrap.value ? 'on' : 'off');
}

function formatTime(value: string) {
  return new Date(value).toLocaleString();
}

onBeforeRouteLeave(() => confirmDiscard());
onMounted(() => {
  softWrap.value = window.localStorage.getItem('crewdock-doc-soft-wrap') !== 'off';
  window.addEventListener('beforeunload', beforeUnload);
  window.addEventListener('keydown', onWindowKeydown);
  docsMediaQuery = window.matchMedia('(max-width: 860px)');
  syncNarrowViewport();
  docsMediaQuery.addEventListener('change', syncNarrowViewport);
  void loadProjects();
});
onBeforeUnmount(() => {
  document.body.classList.remove('docs-fullscreen-open');
  window.removeEventListener('beforeunload', beforeUnload);
  window.removeEventListener('keydown', onWindowKeydown);
  docsMediaQuery?.removeEventListener('change', syncNarrowViewport);
});
</script>

<template>
  <WorkbenchShell layout="workspace">
    <div class="page-head docs-page-head">
      <div>
        <h1>Markdown 文档</h1>
        <p>记录笔记、交接和操作手册，敏感凭据请存入保险库。</p>
      </div>
      <ProjectPicker :model-value="selectedProject" :projects="projects" :disabled="saving" @update:model-value="changeProject" />
    </div>

    <p v-if="error" class="error">{{ error }}</p>

    <section
      class="docs-workspace"
      :class="{ 'tree-open': treeDrawerOpen, busy: saving, fullscreen, 'no-soft-wrap': !softWrap }"
    >
      <button class="docs-drawer-backdrop" type="button" aria-label="关闭目录" @click="treeDrawerOpen = false" />
      <aside id="doc-tree-panel" class="doc-tree-rail" aria-label="文档目录" :aria-hidden="isNarrow && !treeDrawerOpen" :inert="isNarrow && !treeDrawerOpen">
        <div class="doc-tree-head">
          <strong>目录</strong>
          <span>{{ folders.length }} 个文件夹 · {{ docs.length }} 篇文档</span>
          <button class="icon-button docs-tree-close" type="button" title="关闭目录" @click="treeDrawerOpen = false"><X :size="17" /></button>
        </div>
        <div class="doc-tree-tools">
          <label class="doc-tree-search">
            <Search :size="16" />
            <input v-model="query" placeholder="搜索目录" />
          </label>
          <button class="icon-button" type="button" title="新建文件夹" :disabled="!selectedProject || !canWrite" @click="openDialog('new-folder')"><FolderPlus :size="17" /></button>
          <button class="icon-button" type="button" title="新建文档" :disabled="!selectedProject || !canWrite" @click="startNewDoc(null)"><FilePlus2 :size="17" /></button>
        </div>
        <div
          class="doc-tree-root-drop"
          :class="{ active: dragging }"
          @dragover.prevent
          @drop="dropOnRoot"
        >
          <FolderOpen :size="16" />
          <span>项目根目录</span>
        </div>
        <div class="doc-tree-scroll">
          <p v-if="loading" class="muted-text doc-tree-status">正在加载...</p>
          <p v-else-if="!visibleRows.length" class="muted-text doc-tree-status">暂无文档</p>
          <div
            v-for="row in visibleRows"
            v-else
            :key="`${row.kind}:${row.id}`"
            class="doc-tree-row"
            :class="{
              selected: row.kind === 'doc' && row.id === selectedDocId,
              dragging: dragging?.id === row.id,
              'drop-before': dropTarget?.id === row.id && dropTarget.position === 'before',
              'drop-inside': dropTarget?.id === row.id && dropTarget.position === 'inside',
              'drop-after': dropTarget?.id === row.id && dropTarget.position === 'after',
            }"
            :style="{ '--tree-depth': row.depth }"
            :draggable="rowCanWrite(row)"
            @click="row.kind === 'folder' ? toggleFolder(row.id) : row.doc && openDoc(row.doc)"
            @contextmenu="showContext(row, $event)"
            @dragstart="dragStart(row, $event)"
            @dragover="dragOver(row, $event)"
            @drop="dropOnRow(row, $event)"
            @dragend="dragEnd"
          >
            <button v-if="row.kind === 'folder'" class="tree-disclosure" type="button" :title="expandedFolders.has(row.id) ? '收起' : '展开'" @click.stop="toggleFolder(row.id)">
              <ChevronDown v-if="expandedFolders.has(row.id)" :size="15" />
              <ChevronRight v-else :size="15" />
            </button>
            <span v-else class="tree-disclosure-spacer" />
            <FolderOpen v-if="row.kind === 'folder' && expandedFolders.has(row.id)" class="tree-folder-icon" :size="17" />
            <Folder v-else-if="row.kind === 'folder'" class="tree-folder-icon" :size="17" />
            <FileText v-else class="tree-file-icon" :size="16" />
            <span class="tree-row-label" :title="row.name">{{ row.name }}</span>
            <span class="tree-access-slot">
              <ShieldCheck v-if="row.folder?.has_custom_access || row.doc?.has_custom_access" class="tree-access-indicator" :size="13" aria-label="已设置部门权限" />
            </span>
            <button v-if="rowCanWrite(row) || row.kind === 'doc' || canManageAccess" class="tree-row-menu" type="button" title="更多操作" @click="showActionMenu(row, $event)"><EllipsisVertical :size="16" /></button>
          </div>
        </div>
      </aside>

      <main class="doc-surface">
        <template v-if="selectedDoc || creatingDoc">
          <header class="doc-surface-toolbar">
            <button class="icon-button docs-tree-toggle" type="button" title="打开目录" aria-controls="doc-tree-panel" :aria-expanded="treeDrawerOpen" @click="treeDrawerOpen = true"><Menu :size="18" /></button>
            <div class="doc-heading">
              <div class="doc-breadcrumb">
                <span>根目录</span>
                <template v-for="name in folderBreadcrumb(creatingDoc ? newDocFolderId : selectedDoc?.folder_id)" :key="name">
                  <ChevronRight :size="13" /><span>{{ name }}</span>
                </template>
              </div>
              <strong>{{ creatingDoc ? '新建文档' : selectedDoc?.title }}</strong>
            </div>
            <div v-if="!creatingDoc" class="mode-segment" aria-label="文档模式">
              <button type="button" title="预览" aria-label="预览" :class="{ active: mode === 'preview' }" @click="mode === 'edit' ? cancelEdit() : mode = 'preview'"><Eye :size="16" /><span>预览</span></button>
              <button type="button" title="编辑" aria-label="编辑" :class="{ active: mode === 'edit' }" :disabled="!selectedDocCanWrite" @click="beginEdit"><Pencil :size="16" /><span>编辑</span></button>
            </div>
            <div class="doc-view-tools">
              <button
                class="icon-button"
                type="button"
                :class="{ active: softWrap }"
                :title="softWrap ? '关闭自动换行' : '开启自动换行'"
                :aria-pressed="softWrap"
                @click="toggleSoftWrap"
              ><WrapText :size="17" /></button>
              <button
                class="icon-button"
                type="button"
                :title="fullscreen ? '退出全屏' : '全屏文档工作区'"
                :aria-pressed="fullscreen"
                @click="toggleFullscreen"
              >
                <Minimize2 v-if="fullscreen" :size="17" />
                <Maximize2 v-else :size="17" />
              </button>
            </div>
          </header>

          <article v-if="mode === 'preview' && selectedDoc" class="doc-preview-surface">
            <div class="doc-preview-meta">
              <span v-for="tag in selectedDoc.tags" :key="tag">{{ tag }}</span>
              <time>{{ formatTime(selectedDoc.updated_at) }}</time>
            </div>
            <MdPreview :model-value="selectedDoc.body_markdown" />
          </article>

          <form v-else class="doc-edit-surface" @submit.prevent="saveDoc">
            <div class="doc-edit-fields">
              <input v-model="form.title" class="doc-title-input" placeholder="文档标题" required />
              <input v-model="form.tags" class="doc-tags-input" placeholder="标签，用逗号或空格分隔" />
            </div>
            <MdEditor
              v-model="form.body_markdown"
              language="zh-CN"
              :height="560"
              :toolbars-exclude="['fullscreen']"
            />
            <div class="doc-edit-actions">
              <button class="primary-button" :disabled="saving || !form.title.trim()">{{ saving ? '保存中...' : '保存' }}</button>
              <button class="secondary-button" type="button" @click="cancelEdit">取消</button>
              <span v-if="dirty">未保存</span>
            </div>
          </form>
        </template>
        <button v-else class="docs-empty-tree-toggle" type="button" title="打开目录" aria-controls="doc-tree-panel" :aria-expanded="treeDrawerOpen" @click="treeDrawerOpen = true"><Menu :size="18" /></button>
      </main>
    </section>

    <DocTreeContextMenu
      v-if="contextMenu"
      :kind="contextMenu.row.kind"
      :x="contextMenu.x"
      :y="contextMenu.y"
      :can-edit="rowCanWrite(contextMenu.row)"
      :can-manage-access="canManageAccess"
      @action="handleContextAction"
      @close="contextMenu = null"
    />

    <Teleport to="body">
      <div v-if="permissionDialog.open" class="modal-backdrop" @pointerdown.self="!permissionDialog.saving && (permissionDialog.open = false)">
        <form class="dialog-panel doc-permission-dialog" role="dialog" aria-modal="true" aria-labelledby="doc-permission-title" @submit.prevent="savePermissions">
          <div class="panel-head">
            <div>
              <h2 id="doc-permission-title">部门权限</h2>
              <p>{{ permissionDialog.kind === 'folder' ? '文件夹' : '文档' }} · {{ permissionDialog.name }}</p>
            </div>
            <button class="icon-button" type="button" title="关闭" :disabled="permissionDialog.saving" @click="permissionDialog.open = false"><X :size="17" /></button>
          </div>

          <div v-if="permissionDialog.loading" class="loading-state permission-loading">正在加载权限...</div>
          <template v-else>
            <label class="permission-mode-toggle">
              <input v-model="permissionDialog.custom" type="checkbox" />
              <ShieldCheck :size="18" />
              <span>
                <strong>单独设置部门权限</strong>
                <small>关闭后继承{{ permissionDialog.kind === 'folder' ? '上级文件夹或' : '所在文件夹及' }}项目权限</small>
              </span>
            </label>

            <section v-if="permissionDialog.custom" class="permission-department-section">
              <header>
                <div>
                  <strong>可访问部门</strong>
                  <span>选择上级部门时包含其下级部门</span>
                </div>
                <b>{{ selectedPermissionCount }} 个部门</b>
              </header>
              <div class="permission-department-list">
                <div
                  v-for="row in permissionDepartmentRows"
                  :key="row.department.id"
                  class="permission-department-row"
                  :style="{ '--department-depth': row.depth }"
                >
                  <label>
                    <input
                      type="checkbox"
                      :checked="Boolean(permissionRoles[row.department.id])"
                      @change="togglePermissionDepartment(row.department.id, !permissionRoles[row.department.id])"
                    />
                    <Building2 :size="15" />
                    <span>{{ row.department.name }}</span>
                  </label>
                  <select
                    :value="permissionRoles[row.department.id] || 'viewer'"
                    :disabled="!permissionRoles[row.department.id]"
                    :aria-label="`${row.department.name}权限`"
                    @change="changePermissionRole(row.department.id, $event)"
                  >
                    <option value="viewer">只读</option>
                    <option value="editor">可编辑</option>
                  </select>
                </div>
                <p v-if="!permissionDepartmentRows.length" class="muted-text">尚未创建部门</p>
              </div>
              <p v-if="selectedPermissionCount === 0" class="permission-warning">至少选择一个部门，或关闭单独设置以恢复继承。</p>
            </section>

            <footer class="form-actions permission-actions">
              <button class="primary-button" :disabled="permissionDialog.saving || (permissionDialog.custom && selectedPermissionCount === 0)">
                {{ permissionDialog.saving ? '保存中...' : '保存权限' }}
              </button>
              <button class="secondary-button" type="button" :disabled="permissionDialog.saving" @click="permissionDialog.open = false">取消</button>
            </footer>
          </template>
        </form>
      </div>
    </Teleport>

    <Teleport to="body">
      <div v-if="dialog.open" class="modal-backdrop" @pointerdown.self="dialog.open = false">
        <form class="dialog-panel doc-tree-dialog" role="dialog" aria-modal="true" :aria-label="dialogTitle" @submit.prevent="submitDialog">
          <div class="panel-head">
            <h2>{{ dialogTitle }}</h2>
            <button class="icon-button" type="button" title="关闭" @click="dialog.open = false"><X :size="17" /></button>
          </div>
          <label v-if="dialog.action !== 'move'" class="field">
            <span>名称</span>
            <input v-model="dialog.name" autofocus required />
          </label>
          <label v-else class="field">
            <span>目标文件夹</span>
            <select v-model="dialog.parentId">
              <option value="">项目根目录</option>
              <option v-for="option in moveOptions()" :key="option.id" :value="option.id">
                {{ `${'　'.repeat(option.depth)}${option.label}` }}
              </option>
            </select>
          </label>
          <div class="form-actions">
            <button class="primary-button">确定</button>
            <button class="secondary-button" type="button" @click="dialog.open = false">取消</button>
          </div>
        </form>
      </div>
    </Teleport>
  </WorkbenchShell>
</template>
