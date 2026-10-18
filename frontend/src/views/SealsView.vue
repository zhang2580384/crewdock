<script setup lang="ts">
import {
  ChevronDown,
  Circle,
  Download,
  FileImage,
  ImageUp,
  Info,
  LoaderCircle,
  Pencil,
  Plus,
  RectangleHorizontal,
  RotateCcw,
  Ruler,
  Save,
  ScanSearch,
  Search,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  Square,
  Stamp,
  Trash2,
  Upload,
  UserRoundCheck,
  X,
} from '@lucide/vue';
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { api } from '../api';
import WorkbenchShell from '../components/WorkbenchShell.vue';
import { canManageSeals } from '../seal-governance';
import { useSessionStore } from '../stores/session';
import {
  cleanupSealPreview,
  defaultSealGeneratorSettings,
  drawGeneratedSeal,
  generatedSealBlob,
  sealOutputDimensions,
  sealColors,
  type SealCleanupOptions,
  type SealGeneratorSettings,
  type SealTemplate,
} from '../seal-image';
import type { Seal, SealAuthorizationUser, SealGeneratorDefinition } from '../types';

type CreateMode = 'generate' | 'upload';

const session = useSessionStore();
const seals = ref<Seal[]>([]);
const sealImages = ref<Record<string, string>>({});
const companyName = ref('');
const loading = ref(true);
const saving = ref(false);
const previewing = ref(false);
const deletingId = ref('');
const downloadingId = ref('');
const error = ref('');
const modalOpen = ref(false);
const editing = ref<Seal | null>(null);
const name = ref('');
const file = ref<File | null>(null);
const previewUrl = ref('');
const fileInput = ref<HTMLInputElement | null>(null);
const dragActive = ref(false);
const generatedCanvas = ref<HTMLCanvasElement | null>(null);
const createMode = ref<CreateMode>('generate');
const previewScale = ref<'actual' | 'detail'>('actual');
const organizationText = ref('');
const purposeText = ref('合同专用章');
const serialText = ref('');
const generatorSettings = ref<SealGeneratorSettings>(cloneGeneratorSettings(defaultSealGeneratorSettings));
const savedGeneratorSettings = ref<SealGeneratorSettings>(cloneGeneratorSettings(defaultSealGeneratorSettings));
const generatorTemplateSaved = ref(false);
const generatorTemplateUpdatedAt = ref('');
const templateSaving = ref(false);
const templateNotice = ref('');
const templateNoticeError = ref(false);
const editNotice = ref('');
const authorizationOpen = ref(false);
const authorizationTarget = ref<Seal | null>(null);
const authorizationUsers = ref<SealAuthorizationUser[]>([]);
const authorizationDraft = ref<string[]>([]);
const authorizationQuery = ref('');
const authorizationLoading = ref(false);
const authorizationSaving = ref(false);
const authorizationError = ref('');
const deleteTarget = ref<Seal | null>(null);
const cleanup = ref<SealCleanupOptions>({
  remove_background: true,
  red_ink_only: false,
  ink_texture: true,
});
let renderFrame = 0;
let previewSequence = 0;

const isSystemAdmin = computed(() => canManageSeals(session.user?.system_role));

const generatorOptions = computed(() => ({
  ...generatorSettings.value,
  organization: organizationText.value,
  purpose: purposeText.value,
  serial: serialText.value,
}));

const previewDimensions = computed(() => {
  const [width, height] = sealOutputDimensions(generatorSettings.value.template);
  if (generatorSettings.value.template === 'oval') return `45 × 30 mm · 导出 ${width} × ${height} px`;
  if (generatorSettings.value.template === 'square') return `21 × 21 mm · 导出 ${width} × ${height} px`;
  return `40 × 40 mm · 导出 ${width} × ${height} px`;
});

const previewCanvasStyle = computed(() => {
  const detail = previewScale.value === 'detail';
  if (generatorSettings.value.template === 'oval') {
    return { width: detail ? '360px' : '170px', height: detail ? '240px' : '113px' };
  }
  if (generatorSettings.value.template === 'square') {
    return { width: detail ? '280px' : '79px', height: detail ? '280px' : '79px' };
  }
  return { width: detail ? '320px' : '151px', height: detail ? '320px' : '151px' };
});

const editingImage = computed(() => editing.value ? sealImages.value[editing.value.id] || '' : '');

const generatorSettingsAreSaved = computed(() =>
  JSON.stringify(generatorSettings.value) === JSON.stringify(savedGeneratorSettings.value),
);

const generatorTemplateState = computed(() => {
  if (templateNotice.value) return templateNotice.value;
  if (!generatorSettingsAreSaved.value) return '当前调整尚未保存';
  if (!generatorTemplateSaved.value) return '当前使用系统推荐样式';
  return generatorTemplateUpdatedAt.value
    ? `默认样式已保存 · ${formatTime(generatorTemplateUpdatedAt.value)}`
    : '默认样式已保存';
});

const canSave = computed(() => {
  if (!isSystemAdmin.value) return false;
  if (!name.value.trim()) return false;
  if (createMode.value === 'generate') return !!organizationText.value.trim();
  if (editing.value) return true;
  return !!file.value;
});

const filteredAuthorizationUsers = computed(() => {
  const needle = authorizationQuery.value.trim().toLocaleLowerCase();
  if (!needle) return authorizationUsers.value;
  return authorizationUsers.value.filter((user) => [
    user.display_name,
    user.email,
    user.department_name || '',
  ].some((value) => value.toLocaleLowerCase().includes(needle)));
});

function revoke(url: string) {
  if (url) URL.revokeObjectURL(url);
}

function cloneGeneratorSettings(settings: SealGeneratorSettings): SealGeneratorSettings {
  return {
    ...defaultSealGeneratorSettings,
    ...settings,
    schema_version: 2,
  };
}

function applyGeneratorSettings(settings: SealGeneratorSettings) {
  generatorSettings.value = cloneGeneratorSettings(settings);
}

async function loadSealImages(rows: Seal[]) {
  const next: Record<string, string> = {};
  await Promise.all(rows.map(async (seal) => {
    try {
      next[seal.id] = URL.createObjectURL(await api.sealImage(seal.id));
    } catch {
      next[seal.id] = '';
    }
  }));
  Object.values(sealImages.value).forEach(revoke);
  sealImages.value = next;
}

async function load() {
  loading.value = true;
  error.value = '';
  try {
    if (!session.user) await session.loadMe();
    const [nextSeals, company, generatorTemplate] = await Promise.all([
      api.seals(),
      api.company().catch(() => null),
      isSystemAdmin.value ? api.sealGeneratorTemplate() : Promise.resolve(null),
    ]);
    seals.value = nextSeals;
    if (company) companyName.value = company.name;
    if (generatorTemplate) {
      savedGeneratorSettings.value = cloneGeneratorSettings(generatorTemplate.settings);
      generatorTemplateSaved.value = generatorTemplate.saved;
      generatorTemplateUpdatedAt.value = generatorTemplate.updated_at || '';
    }
    await loadSealImages(nextSeals);
  } catch (err) {
    error.value = err instanceof Error ? err.message : '加载印章失败';
  } finally {
    loading.value = false;
  }
}

function openCreate() {
  if (!isSystemAdmin.value) return;
  editing.value = null;
  createMode.value = 'generate';
  previewScale.value = 'actual';
  applyGeneratorSettings(savedGeneratorSettings.value);
  organizationText.value = companyName.value;
  purposeText.value = '';
  serialText.value = '';
  templateNotice.value = '';
  templateNoticeError.value = false;
  editNotice.value = '';
  cleanup.value = {
    remove_background: true,
    red_ink_only: false,
    ink_texture: true,
  };
  name.value = '公章';
  file.value = null;
  revoke(previewUrl.value);
  previewUrl.value = '';
  error.value = '';
  modalOpen.value = true;
  queueGeneratedPreview();
}

function openEdit(seal: Seal) {
  if (!isSystemAdmin.value) return;
  editing.value = seal;
  name.value = seal.name;
  previewScale.value = 'actual';
  file.value = null;
  revoke(previewUrl.value);
  previewUrl.value = '';
  templateNotice.value = '';
  templateNoticeError.value = false;
  editNotice.value = '';
  cleanup.value = {
    remove_background: true,
    red_ink_only: false,
    ink_texture: true,
  };
  const definition = seal.generator_definition;
  if (definition) {
    createMode.value = 'generate';
    applyGeneratorSettings(definition.settings);
    organizationText.value = definition.organization;
    purposeText.value = definition.purpose;
    serialText.value = definition.serial;
  } else if (seal.source_kind === 'legacy_generated') {
    createMode.value = 'generate';
    applyGeneratorSettings(savedGeneratorSettings.value);
    organizationText.value = companyName.value;
    purposeText.value = '';
    serialText.value = '';
    editNotice.value = '这枚印章由旧版本生成，原文字参数未保存；本次保存后即可完整反复编辑。';
  } else {
    createMode.value = 'upload';
    applyGeneratorSettings(savedGeneratorSettings.value);
    organizationText.value = companyName.value;
    purposeText.value = '';
    serialText.value = '';
  }
  error.value = '';
  modalOpen.value = true;
  queueGeneratedPreview();
}

function closeModal() {
  if (saving.value) return;
  previewSequence += 1;
  revoke(previewUrl.value);
  previewUrl.value = '';
  file.value = null;
  modalOpen.value = false;
}

function selectCreateMode(mode: CreateMode) {
  createMode.value = mode;
  error.value = '';
  if (mode === 'generate') {
    if (!organizationText.value.trim()) organizationText.value = companyName.value;
    if (!name.value.trim()) name.value = purposeText.value || '自定义印章';
    queueGeneratedPreview();
  }
}

function selectTemplate(template: SealTemplate) {
  generatorSettings.value.template = template;
  if (template === 'round') {
    if (!organizationText.value.trim()) organizationText.value = companyName.value;
    purposeText.value = '';
    name.value = '公章';
  } else if (template === 'oval') {
    if (!organizationText.value.trim()) organizationText.value = companyName.value;
    purposeText.value = '发票专用章';
    name.value = '发票专用章';
  } else {
    if (organizationText.value === companyName.value) organizationText.value = '';
    purposeText.value = '印';
    name.value = '法定代表人名章';
  }
}

function restoreRecommendedSettings() {
  applyGeneratorSettings(defaultSealGeneratorSettings);
  templateNotice.value = '已恢复推荐参数，保存后将设为默认。';
  templateNoticeError.value = false;
}

async function saveGeneratorTemplate() {
  templateSaving.value = true;
  templateNotice.value = '';
  templateNoticeError.value = false;
  try {
    const template = await api.updateSealGeneratorTemplate(cloneGeneratorSettings(generatorSettings.value));
    savedGeneratorSettings.value = cloneGeneratorSettings(template.settings);
    generatorTemplateSaved.value = template.saved;
    generatorTemplateUpdatedAt.value = template.updated_at || '';
    templateNotice.value = '已保存为你的默认样式。';
  } catch (err) {
    templateNotice.value = err instanceof Error ? err.message : '保存默认样式失败';
    templateNoticeError.value = true;
  } finally {
    templateSaving.value = false;
  }
}

function queueGeneratedPreview() {
  if (!modalOpen.value || createMode.value !== 'generate') return;
  window.cancelAnimationFrame(renderFrame);
  void nextTick(() => {
    renderFrame = window.requestAnimationFrame(() => {
      if (generatedCanvas.value) drawGeneratedSeal(generatedCanvas.value, generatorOptions.value);
    });
  });
}

function chooseFile(next: File | null) {
  if (!next) return;
  error.value = '';
  if (next.size > 5 * 1024 * 1024) {
    error.value = '印章图片不能超过 5 MiB';
    return;
  }
  if (!['image/png', 'image/jpeg', 'image/webp'].includes(next.type)) {
    error.value = '仅支持 PNG、JPEG 或 WebP 图片';
    return;
  }
  file.value = next;
  if (!name.value.trim() || name.value === '合同专用章') name.value = next.name.replace(/\.[^.]+$/, '');
  void refreshUploadPreview();
}

async function refreshUploadPreview() {
  if (!file.value) return;
  const sequence = ++previewSequence;
  previewing.value = true;
  try {
    const preview = await cleanupSealPreview(file.value, cleanup.value);
    if (sequence !== previewSequence) return;
    const url = URL.createObjectURL(preview);
    revoke(previewUrl.value);
    previewUrl.value = url;
  } catch (err) {
    if (sequence === previewSequence) {
      revoke(previewUrl.value);
      previewUrl.value = '';
      error.value = err instanceof Error ? err.message : '无法生成处理预览';
    }
  } finally {
    if (sequence === previewSequence) previewing.value = false;
  }
}

function onFileChange(event: Event) {
  chooseFile((event.target as HTMLInputElement).files?.[0] || null);
}

function onDrop(event: DragEvent) {
  dragActive.value = false;
  chooseFile(event.dataTransfer?.files?.[0] || null);
}

async function save() {
  const cleanName = name.value.trim();
  if (!cleanName) {
    error.value = '请输入印章名称';
    return;
  }
  if (createMode.value === 'generate' && !organizationText.value.trim()) {
    error.value = generatorSettings.value.template === 'square' ? '请输入姓名或印文' : '请输入单位全称';
    return;
  }
  if (!editing.value && createMode.value === 'upload' && !file.value) {
    error.value = '请选择印章图片';
    return;
  }
  saving.value = true;
  error.value = '';
  let saved = false;
  try {
    if (createMode.value === 'generate') {
      const blob = await generatedSealBlob(generatorOptions.value);
      const definition: SealGeneratorDefinition = {
        schema_version: 1,
        organization: organizationText.value.trim(),
        purpose: purposeText.value.trim(),
        serial: serialText.value.trim(),
        settings: cloneGeneratorSettings(generatorSettings.value),
      };
      const options = {
        remove_background: false,
        red_ink_only: false,
        ink_texture: false,
      };
      if (editing.value) await api.replaceSeal(editing.value.id, cleanName, blob, options, definition);
      else await api.createSeal(cleanName, blob, options, definition);
    } else if (editing.value && file.value) {
      await api.replaceSeal(editing.value.id, cleanName, file.value, cleanup.value);
    } else if (editing.value) {
      await api.updateSeal(editing.value.id, cleanName);
    } else {
      await api.createSeal(cleanName, file.value!, cleanup.value);
    }
    await load();
    saved = true;
  } catch (err) {
    error.value = err instanceof Error ? err.message : '保存印章失败';
  } finally {
    saving.value = false;
  }
  if (saved) closeModal();
}

async function downloadSeal(seal: Seal) {
  if (!isSystemAdmin.value) return;
  downloadingId.value = seal.id;
  error.value = '';
  try {
    const blob = await api.sealImage(seal.id);
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = `${safeDownloadName(seal.name)}.png`;
    anchor.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 0);
  } catch (err) {
    error.value = err instanceof Error ? err.message : '下载印章失败';
  } finally {
    downloadingId.value = '';
  }
}

async function openAuthorizations(seal: Seal) {
  if (!isSystemAdmin.value) return;
  authorizationTarget.value = seal;
  authorizationUsers.value = [];
  authorizationDraft.value = [];
  authorizationQuery.value = '';
  authorizationError.value = '';
  authorizationLoading.value = true;
  authorizationOpen.value = true;
  try {
    const result = await api.sealAuthorizations(seal.id);
    if (authorizationTarget.value?.id !== seal.id) return;
    authorizationUsers.value = result.users;
    authorizationDraft.value = result.users.filter((user) => user.authorized).map((user) => user.user_id);
  } catch (err) {
    authorizationError.value = err instanceof Error ? err.message : '加载员工授权失败';
  } finally {
    authorizationLoading.value = false;
  }
}

function closeAuthorizations() {
  if (authorizationSaving.value) return;
  authorizationOpen.value = false;
  authorizationTarget.value = null;
  authorizationUsers.value = [];
  authorizationDraft.value = [];
  authorizationError.value = '';
}

function setVisibleAuthorizations(authorized: boolean) {
  const next = new Set(authorizationDraft.value);
  for (const user of filteredAuthorizationUsers.value) {
    if (authorized) next.add(user.user_id);
    else next.delete(user.user_id);
  }
  authorizationDraft.value = [...next];
}

async function saveAuthorizations() {
  const seal = authorizationTarget.value;
  if (!seal) return;
  authorizationSaving.value = true;
  authorizationError.value = '';
  try {
    const result = await api.updateSealAuthorizations(seal.id, authorizationDraft.value);
    const authorizedUserCount = result.users.filter((user) => user.authorized).length;
    seals.value = seals.value.map((item) => item.id === seal.id
      ? { ...item, authorized_user_count: authorizedUserCount }
      : item);
    authorizationSaving.value = false;
    closeAuthorizations();
  } catch (err) {
    authorizationError.value = err instanceof Error ? err.message : '保存员工授权失败';
  } finally {
    authorizationSaving.value = false;
  }
}

function requestDelete(seal: Seal) {
  if (!isSystemAdmin.value) return;
  error.value = '';
  deleteTarget.value = seal;
}

async function removeSeal() {
  const seal = deleteTarget.value;
  if (!seal) return;
  deletingId.value = seal.id;
  error.value = '';
  try {
    await api.deleteSeal(seal.id);
    revoke(sealImages.value[seal.id]);
    delete sealImages.value[seal.id];
    seals.value = seals.value.filter((item) => item.id !== seal.id);
    deleteTarget.value = null;
  } catch (err) {
    error.value = err instanceof Error ? err.message : '删除印章失败';
  } finally {
    deletingId.value = '';
  }
}

function safeDownloadName(value: string) {
  return value.replace(/[\\/:*?"<>|]/g, '_').trim() || '印章';
}

function formatSize(value: number) {
  return value < 1024 * 1024 ? `${Math.ceil(value / 1024)} KiB` : `${(value / 1024 / 1024).toFixed(1)} MiB`;
}

function formatTime(value: string) {
  return new Date(value).toLocaleDateString('zh-CN');
}

watch(generatorOptions, queueGeneratedPreview);
watch(
  generatorSettings,
  () => {
    if (templateSaving.value) return;
    templateNotice.value = '';
    templateNoticeError.value = false;
  },
  { deep: true, flush: 'sync' },
);
watch([modalOpen, editing, createMode], queueGeneratedPreview);
watch(
  [
    () => cleanup.value.remove_background,
    () => cleanup.value.red_ink_only,
    () => cleanup.value.ink_texture,
  ],
  () => {
    if (modalOpen.value && createMode.value === 'upload' && file.value) {
      void refreshUploadPreview();
    }
  },
);

onMounted(load);
onBeforeUnmount(() => {
  window.cancelAnimationFrame(renderFrame);
  previewSequence += 1;
  Object.values(sealImages.value).forEach(revoke);
  revoke(previewUrl.value);
});
</script>

<template>
  <WorkbenchShell>
    <div class="page-head seal-page-head">
      <div>
        <h1>{{ isSystemAdmin ? '企业印章' : '已授权印章' }}</h1>
        <p>{{ isSystemAdmin ? '统一维护企业印章，并授权员工在签章工具中使用。' : '仅显示管理员明确授权给你的企业印章。' }}</p>
      </div>
      <button v-if="isSystemAdmin" class="primary-button" type="button" @click="openCreate"><Plus :size="17" />新建印章</button>
    </div>

    <div class="signing-boundary"><ShieldCheck v-if="isSystemAdmin" :size="16" /><Info v-else :size="16" /><span>{{ isSystemAdmin ? '系统管理员可管理全部印章；员工只能使用已授权印章。' : '如需新增或调整印章权限，请联系系统管理员。' }}</span></div>
    <p v-if="error && !modalOpen" class="error settings-message" role="alert">{{ error }}</p>

    <div v-if="loading" class="loading-state"><LoaderCircle class="spin" :size="22" /><span>正在读取印章...</span></div>
    <section v-else-if="seals.length" class="seal-grid" :aria-label="isSystemAdmin ? '企业印章列表' : '已授权印章列表'">
      <article v-for="seal in seals" :key="seal.id" class="seal-card" :class="{ readonly: !isSystemAdmin }">
        <div class="seal-preview">
          <img v-if="sealImages[seal.id]" :src="sealImages[seal.id]" :alt="seal.name" />
          <FileImage v-else :size="32" />
        </div>
        <div class="seal-card-copy">
          <strong>{{ seal.name }}</strong>
          <span>{{ seal.width_px }} × {{ seal.height_px }} px · {{ formatSize(seal.size_bytes) }}</span>
          <small>{{ isSystemAdmin ? `已授权 ${seal.authorized_user_count} 人` : '管理员已授权' }} · 更新于 {{ formatTime(seal.updated_at) }}</small>
        </div>
        <div v-if="isSystemAdmin" class="seal-card-actions">
          <button class="icon-button" type="button" title="员工授权" aria-label="员工授权" @click="openAuthorizations(seal)"><UserRoundCheck :size="16" /></button>
          <button class="icon-button" type="button" title="下载透明 PNG" aria-label="下载透明 PNG" :disabled="downloadingId === seal.id" @click="downloadSeal(seal)">
            <LoaderCircle v-if="downloadingId === seal.id" class="spin" :size="16" /><Download v-else :size="16" />
          </button>
          <button class="icon-button" type="button" title="编辑印章" aria-label="编辑印章" @click="openEdit(seal)"><Pencil :size="16" /></button>
          <button class="icon-button danger-icon" type="button" title="删除" aria-label="删除" :disabled="deletingId === seal.id" @click="requestDelete(seal)">
            <LoaderCircle v-if="deletingId === seal.id" class="spin" :size="16" /><Trash2 v-else :size="16" />
          </button>
        </div>
      </article>
    </section>
    <section v-else class="seal-empty">
      <div class="seal-empty-mark"><Stamp :size="30" /></div>
      <strong>{{ isSystemAdmin ? '还没有企业印章' : '暂未获得印章授权' }}</strong>
      <span>{{ isSystemAdmin ? '输入印文生成，或导入已有印章图片。' : '管理员授权后，印章会自动显示在这里和签章工具中。' }}</span>
      <button v-if="isSystemAdmin" class="secondary-button" type="button" @click="openCreate"><Sparkles :size="17" />生成第一枚印章</button>
    </section>

    <div v-if="modalOpen && isSystemAdmin" class="modal-backdrop" role="presentation" @mousedown.self="closeModal">
      <section class="seal-dialog create" role="dialog" aria-modal="true" :aria-label="editing ? '编辑印章' : '新建印章'">
        <header>
          <div><h2>{{ editing ? '编辑印章' : '新建印章' }}</h2><p>{{ editing ? '调整图稿、文字与结构，保存后更新印章。' : '生成透明印章，或处理已有图片。' }}</p></div>
          <button class="icon-button" type="button" title="关闭" aria-label="关闭" @click="closeModal"><X :size="18" /></button>
        </header>

          <div class="segmented seal-create-tabs" :aria-label="editing ? '编辑方式' : '新建方式'">
            <button type="button" :class="{ active: createMode === 'generate' }" @click="selectCreateMode('generate')"><Sparkles :size="16" />自动生成</button>
            <button type="button" :class="{ active: createMode === 'upload' }" @click="selectCreateMode('upload')"><ImageUp :size="16" />上传处理</button>
          </div>
          <div v-if="editNotice" class="signing-boundary seal-edit-notice"><Info :size="15" /><span>{{ editNotice }}</span></div>

          <div class="seal-create-layout">
            <div class="seal-create-controls">
              <label class="field"><span>印章名称</span><input v-model="name" maxlength="120" autocomplete="off" /></label>

              <template v-if="createMode === 'generate'">
                <div class="field">
                  <span>印章样式</span>
                  <div class="seal-shape-grid" role="group" aria-label="印章样式">
                    <button type="button" :class="{ active: generatorSettings.template === 'round' }" @click="selectTemplate('round')"><Circle :size="20" /><span>圆形公章</span></button>
                    <button type="button" :class="{ active: generatorSettings.template === 'oval' }" @click="selectTemplate('oval')"><RectangleHorizontal :size="22" /><span>椭圆专用章</span></button>
                    <button type="button" :class="{ active: generatorSettings.template === 'square' }" @click="selectTemplate('square')"><Square :size="19" /><span>方形名章</span></button>
                  </div>
                </div>
                <label class="field">
                  <span>{{ generatorSettings.template === 'square' ? '姓名或印文' : '单位全称' }}</span>
                  <input v-model="organizationText" :maxlength="generatorSettings.template === 'square' ? 5 : 30" autocomplete="off" />
                </label>
                <div class="seal-field-grid">
                  <label class="field"><span>{{ generatorSettings.template === 'square' ? '附加印文' : '用途文字' }}</span><input v-model="purposeText" :maxlength="generatorSettings.template === 'square' ? 1 : 14" autocomplete="off" /></label>
                  <label class="field"><span>印章编号</span><input v-model="serialText" maxlength="32" inputmode="numeric" autocomplete="off" placeholder="13 位印章编码（可选）" /></label>
                </div>
                <div class="seal-appearance-row">
                  <div><span>印泥颜色</span><div class="seal-color-swatches" role="group" aria-label="印泥颜色"><button v-for="color in sealColors" :key="color" type="button" :class="{ active: generatorSettings.color === color }" :style="{ '--seal-color': color }" :title="color" :aria-label="`选择印泥颜色 ${color}`" @click="generatorSettings.color = color"><span /></button></div></div>
                  <label><span>印泥磨损 <output>{{ generatorSettings.wear }}</output></span><input v-model.number="generatorSettings.wear" type="range" min="0" max="30" step="1" /></label>
                </div>

                <label v-if="generatorSettings.template !== 'square'" class="seal-quick-range"><span>文字与外圈净距 <output>{{ generatorSettings.organization_inset }} px</output></span><input v-model.number="generatorSettings.organization_inset" type="range" min="0" max="70" step="1" /></label>

                <details class="seal-tuning-panel">
                  <summary><span><SlidersHorizontal :size="16" />结构微调</span><span class="seal-tuning-summary-meta"><small>{{ generatorSettings.template === 'round' ? '14 项' : generatorSettings.template === 'oval' ? '13 项' : '1 项' }}</small><ChevronDown class="seal-tuning-chevron" :size="15" /></span></summary>
                  <div class="seal-tuning-grid">
                    <label class="seal-range-field"><span>外圈粗细 <output>{{ generatorSettings.outer_border_width }} px</output></span><input v-model.number="generatorSettings.outer_border_width" type="range" min="8" max="24" step="1" /></label>
                    <template v-if="generatorSettings.template !== 'square'">
                      <label class="seal-range-field"><span>单位文字大小 <output>{{ generatorSettings.organization_font_size }} px</output></span><input v-model.number="generatorSettings.organization_font_size" type="range" min="38" max="68" step="1" /></label>
                      <label class="seal-range-field"><span>单位文字间距 <output>{{ generatorSettings.organization_spacing }}</output></span><input v-model.number="generatorSettings.organization_spacing" type="range" min="-12" max="18" step="1" /></label>
                      <label class="seal-range-field"><span>单位文字高度 <output>{{ generatorSettings.organization_height }}%</output></span><input v-model.number="generatorSettings.organization_height" type="range" min="90" max="180" step="2" /></label>
                      <label class="seal-range-field"><span>单位文字字宽 <output>{{ generatorSettings.organization_width }}%</output></span><input v-model.number="generatorSettings.organization_width" type="range" min="65" max="115" step="1" /></label>
                      <label class="seal-range-field"><span>文字笔画增厚 <output>{{ generatorSettings.text_stroke_width }} px</output></span><input v-model.number="generatorSettings.text_stroke_width" type="range" min="0" max="6" step="1" /></label>
                      <label v-if="generatorSettings.template === 'round'" class="seal-range-field"><span>五角星大小 <output>{{ generatorSettings.star_size }} px</output></span><input v-model.number="generatorSettings.star_size" type="range" min="70" max="140" step="2" /></label>
                      <label class="seal-range-field"><span>用途上下 <output>{{ generatorSettings.purpose_offset_y }} px</output></span><input v-model.number="generatorSettings.purpose_offset_y" type="range" min="-70" max="70" step="2" /></label>
                      <label class="seal-range-field"><span>用途文字大小 <output>{{ generatorSettings.purpose_font_size }} px</output></span><input v-model.number="generatorSettings.purpose_font_size" type="range" min="28" max="64" step="1" /></label>
                      <label class="seal-range-field"><span>用途文字间距 <output>{{ generatorSettings.purpose_spacing }}</output></span><input v-model.number="generatorSettings.purpose_spacing" type="range" min="-6" max="18" step="1" /></label>
                      <label class="seal-range-field"><span>编号大小 <output>{{ generatorSettings.serial_font_size }} px</output></span><input v-model.number="generatorSettings.serial_font_size" type="range" min="18" max="36" step="1" /></label>
                      <label class="seal-range-field"><span>编号间距 <output>{{ generatorSettings.serial_spacing }}</output></span><input v-model.number="generatorSettings.serial_spacing" type="range" min="-4" max="16" step="1" /></label>
                      <label class="seal-range-field"><span>编号与外圈净距 <output>{{ generatorSettings.serial_inset }} px</output></span><input v-model.number="generatorSettings.serial_inset" type="range" min="18" max="90" step="1" /></label>
                      <label class="seal-range-field"><span>编号字宽 <output>{{ generatorSettings.serial_width }}%</output></span><input v-model.number="generatorSettings.serial_width" type="range" min="60" max="110" step="1" /></label>
                    </template>
                  </div>
                  <div class="seal-template-bar">
                    <span class="seal-template-state" :class="{ error: templateNoticeError }" aria-live="polite">{{ generatorTemplateState }}</span>
                    <div>
                      <button class="secondary-button seal-template-button" type="button" :disabled="templateSaving" @click="restoreRecommendedSettings"><RotateCcw :size="15" />恢复推荐</button>
                      <button class="secondary-button seal-template-button" type="button" :disabled="templateSaving" @click="saveGeneratorTemplate"><LoaderCircle v-if="templateSaving" class="spin" :size="15" /><Save v-else :size="15" />{{ templateSaving ? '保存中...' : '保存为默认' }}</button>
                    </div>
                  </div>
                </details>
              </template>

              <template v-else>
                <button class="seal-file-picker" :class="{ active: dragActive }" type="button" @click="fileInput?.click()" @dragenter.prevent="dragActive = true" @dragover.prevent @dragleave.prevent="dragActive = false" @drop.prevent="onDrop">
                  <Upload :size="22" /><span><strong>{{ file?.name || (editing ? '选择新图片以替换' : '选择或拖入印章图片') }}</strong><small>{{ editing && !file ? '不选择则保留当前图片' : 'PNG、JPEG、WebP，最大 5 MiB' }}</small></span>
                </button>
                <input ref="fileInput" hidden type="file" accept="image/png,image/jpeg,image/webp" @change="onFileChange" />
                <div class="seal-cleanup-options">
                  <label class="seal-option-row"><span><strong>去除浅色底片</strong><small>白色和浅灰背景转为透明</small></span><span class="settings-toggle-control"><input v-model="cleanup.remove_background" type="checkbox" /><span aria-hidden="true" /></span></label>
                  <label class="seal-option-row"><span><strong>仅保留红色印泥</strong><small>移除黑色日期、文字和灰色水印</small></span><span class="settings-toggle-control"><input v-model="cleanup.red_ink_only" type="checkbox" /><span aria-hidden="true" /></span></label>
                  <label class="seal-option-row"><span><strong>印泥质感</strong><small>降低机械贴图感</small></span><span class="settings-toggle-control"><input v-model="cleanup.ink_texture" type="checkbox" /><span aria-hidden="true" /></span></label>
                </div>
              </template>
            </div>

            <section class="seal-live-preview" aria-label="印章预览">
              <header>
                <div><strong>{{ createMode === 'generate' ? '生成预览' : '处理预览' }}</strong><span>{{ createMode === 'generate' ? previewDimensions : (file ? `${file.type.replace('image/', '').toUpperCase()} · ${formatSize(file.size)}` : '透明 PNG') }}</span></div>
                <div v-if="createMode === 'generate'" class="segmented seal-preview-scale" aria-label="预览比例">
                  <button type="button" title="实际尺寸" aria-label="实际尺寸" :class="{ active: previewScale === 'actual' }" @click="previewScale = 'actual'"><Ruler :size="15" /></button>
                  <button type="button" title="放大检查" aria-label="放大检查" :class="{ active: previewScale === 'detail' }" @click="previewScale = 'detail'"><ScanSearch :size="15" /></button>
                </div>
              </header>
              <div class="seal-composer-canvas">
                <canvas v-if="createMode === 'generate'" ref="generatedCanvas" role="img" aria-label="生成的印章预览" :style="previewCanvasStyle" />
                <img v-else-if="(previewUrl || editingImage) && !previewing" :src="previewUrl || editingImage" alt="处理后的印章预览" />
                <div v-else-if="previewing" class="seal-preview-status"><LoaderCircle class="spin" :size="24" /><span>正在处理...</span></div>
                <div v-else class="seal-preview-status"><ImageUp :size="26" /><span>尚未选择图片</span></div>
              </div>
            </section>
          </div>

        <p v-if="error" class="error seal-dialog-error" role="alert">{{ error }}</p>
        <footer>
          <button class="secondary-button" type="button" :disabled="saving" @click="closeModal">取消</button>
          <button class="primary-button" type="button" :disabled="saving || !canSave" @click="save"><LoaderCircle v-if="saving" class="spin" :size="16" /><Save v-else-if="editing" :size="16" /><Sparkles v-else-if="createMode === 'generate'" :size="16" />{{ saving ? '保存中...' : (editing ? '保存修改' : createMode === 'generate' ? '生成并保存' : '处理并保存') }}</button>
        </footer>
      </section>
    </div>

    <div v-if="authorizationOpen && authorizationTarget" class="seal-authorization-backdrop" role="presentation" @mousedown.self="closeAuthorizations">
      <aside class="seal-authorization-drawer" role="dialog" aria-modal="true" aria-labelledby="seal-authorization-title">
        <header>
          <div class="seal-authorization-heading">
            <span><UserRoundCheck :size="20" /></span>
            <div><h2 id="seal-authorization-title">员工授权</h2><p>{{ authorizationTarget.name }}</p></div>
          </div>
          <button class="icon-button" type="button" title="关闭" aria-label="关闭" :disabled="authorizationSaving" @click="closeAuthorizations"><X :size="18" /></button>
        </header>

        <div class="seal-authorization-toolbar">
          <label class="seal-authorization-search"><Search :size="16" /><input v-model="authorizationQuery" type="search" placeholder="搜索姓名、邮箱或部门" /></label>
          <span>{{ authorizationDraft.length }} 人已授权</span>
        </div>
        <div class="seal-authorization-actions">
          <button type="button" :disabled="authorizationLoading || !filteredAuthorizationUsers.length" @click="setVisibleAuthorizations(true)">授权当前结果</button>
          <button type="button" :disabled="authorizationLoading || !filteredAuthorizationUsers.length" @click="setVisibleAuthorizations(false)">清除当前结果</button>
        </div>

        <div class="seal-authorization-list">
          <div v-if="authorizationLoading" class="loading-state"><LoaderCircle class="spin" :size="20" /><span>正在读取员工...</span></div>
          <p v-else-if="!filteredAuthorizationUsers.length" class="seal-authorization-empty">{{ authorizationUsers.length ? '没有匹配的员工' : '暂无可授权的启用员工' }}</p>
          <label v-for="user in filteredAuthorizationUsers" v-else :key="user.user_id" class="seal-authorization-user">
            <span class="seal-user-avatar">{{ user.display_name.trim().slice(0, 1) || '?' }}</span>
            <span class="seal-user-copy"><strong>{{ user.display_name }}</strong><small>{{ user.email }}{{ user.department_name ? ` · ${user.department_name}` : '' }}</small></span>
            <input v-model="authorizationDraft" type="checkbox" :value="user.user_id" />
          </label>
        </div>

        <p v-if="authorizationError" class="error seal-dialog-error" role="alert">{{ authorizationError }}</p>
        <footer>
          <button class="secondary-button" type="button" :disabled="authorizationSaving" @click="closeAuthorizations">取消</button>
          <button class="primary-button" type="button" :disabled="authorizationLoading || authorizationSaving" @click="saveAuthorizations"><LoaderCircle v-if="authorizationSaving" class="spin" :size="16" /><Save v-else :size="16" />{{ authorizationSaving ? '保存中...' : '保存授权' }}</button>
        </footer>
      </aside>
    </div>

    <div v-if="deleteTarget" class="modal-backdrop" role="presentation" @mousedown.self="!deletingId && (deleteTarget = null)">
      <section class="signing-confirm-dialog seal-delete-dialog" role="alertdialog" aria-modal="true" aria-labelledby="seal-delete-title">
        <header><div class="confirm-mark danger"><Trash2 :size="21" /></div><button class="icon-button" type="button" title="关闭" aria-label="关闭" :disabled="!!deletingId" @click="deleteTarget = null"><X :size="18" /></button></header>
        <h2 id="seal-delete-title">删除“{{ deleteTarget.name }}”？</h2>
        <p>印章将从企业印章库移除，所有员工授权同时撤销；已经生成的签章文件不受影响。</p>
        <p v-if="error" class="error seal-dialog-error" role="alert">{{ error }}</p>
        <footer><button class="secondary-button" type="button" :disabled="!!deletingId" @click="deleteTarget = null">取消</button><button class="primary-button danger-button" type="button" :disabled="!!deletingId" @click="removeSeal"><LoaderCircle v-if="deletingId" class="spin" :size="16" /><Trash2 v-else :size="16" />{{ deletingId ? '删除中...' : '确认删除' }}</button></footer>
      </section>
    </div>
  </WorkbenchShell>
</template>
