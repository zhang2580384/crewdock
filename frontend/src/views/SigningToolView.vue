<script setup lang="ts">
import {
  ArrowLeft,
  ChevronLeft,
  ChevronRight,
  CircleCheck,
  Download,
  FileArchive,
  FileImage,
  FilePenLine,
  FilePlus2,
  Info,
  Images,
  LoaderCircle,
  Maximize2,
  RotateCcw,
  Stamp as StampIcon,
  Trash2,
  Upload,
  X,
} from '@lucide/vue';
import * as pdfjsLib from 'pdfjs-dist';
import type { PDFDocumentProxy, RenderTask } from 'pdfjs-dist';
import pdfWorker from 'pdfjs-dist/build/pdf.worker.min.mjs?url';
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue';
import { api } from '../api';
import WorkbenchShell from '../components/WorkbenchShell.vue';
import { nextPlacementId } from '../placement-id';
import { signedImageDownloadName } from '../seal-governance';
import { isSigningDocumentEditable, signingDocumentPreview } from '../signing-document';
import type {
  FinalizeSigningRequest,
  Seal,
  SigningDocument,
  StampPlacement,
} from '../types';

const pdfWorkerUrl = new URL(pdfWorker, window.location.origin);
pdfWorkerUrl.searchParams.set('v', __APP_VERSION__);
pdfjsLib.GlobalWorkerOptions.workerSrc = pdfWorkerUrl.href;

interface PendingPlacement extends StampPlacement {
  id: string;
}

const documents = ref<SigningDocument[]>([]);
const seals = ref<Seal[]>([]);
const sealImages = ref<Record<string, string>>({});
const currentDocument = ref<SigningDocument | null>(null);
const pdfDocument = shallowRef<PDFDocumentProxy | null>(null);
const currentPage = ref(1);
const canvas = ref<HTMLCanvasElement | null>(null);
const pageStage = ref<HTMLElement | null>(null);
const pageWrap = ref<HTMLElement | null>(null);
const viewportWidth = ref(0);
const viewportHeight = ref(0);
const mode = ref<'single' | 'seam'>('single');
const placements = ref<PendingPlacement[]>([]);
const selectedPlacementId = ref('');
const selectedSealId = ref('');
const stampWidth = ref(16);
const seamStartPage = ref(1);
const seamEndPage = ref(2);
const seamY = ref(35);
const seamHeight = ref(30);
const loading = ref(true);
const opening = ref(false);
const uploading = ref(false);
const finalizing = ref(false);
const exporting = ref('');
const error = ref('');
const confirmOpen = ref(false);
const stampDropActive = ref(false);
const fileInput = ref<HTMLInputElement | null>(null);
let renderTask: RenderTask | null = null;
let resizeObserver: ResizeObserver | null = null;
let dragging: {
  kind: 'single' | 'seam';
  id?: string;
  startX: number;
  startY: number;
  initialX: number;
  initialY: number;
} | null = null;

const selectedSeal = computed(() => seals.value.find((seal) => seal.id === selectedSealId.value) || null);
const selectedPlacement = computed(() => placements.value.find((item) => item.id === selectedPlacementId.value) || null);
const documentFinalized = computed(() => Boolean(
  currentDocument.value && !isSigningDocumentEditable(currentDocument.value),
));
const pagePlacements = computed(() => documentFinalized.value
  ? []
  : placements.value.filter((item) => item.page === currentPage.value));
const seamPageCount = computed(() => Math.max(2, seamEndPage.value - seamStartPage.value + 1));
const seamVisible = computed(() => (
  !documentFinalized.value
  && mode.value === 'seam'
  && selectedSeal.value
  && currentPage.value >= seamStartPage.value
  && currentPage.value <= seamEndPage.value
));
const seamSliceIndex = computed(() => currentPage.value - seamStartPage.value);
const seamWidthPercent = computed(() => {
  const seal = selectedSeal.value;
  if (!seal || !viewportWidth.value || !viewportHeight.value) return 4;
  return seamHeight.value
    * (viewportHeight.value / viewportWidth.value)
    * (seal.width_px / seal.height_px)
    / seamPageCount.value;
});
const canFinalize = computed(() => (
  currentDocument.value
  && isSigningDocumentEditable(currentDocument.value)
  && (mode.value === 'single'
    ? placements.value.length > 0
    : selectedSeal.value && seamEndPage.value > seamStartPage.value)
));

function revoke(url: string) {
  if (url) URL.revokeObjectURL(url);
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
    const [nextDocuments, nextSeals] = await Promise.all([api.signingDocuments(), api.seals()]);
    documents.value = nextDocuments;
    seals.value = nextSeals;
    selectedSealId.value = nextSeals[0]?.id || '';
    await loadSealImages(nextSeals);
  } catch (err) {
    error.value = err instanceof Error ? err.message : '加载签章工具失败';
  } finally {
    loading.value = false;
  }
}

async function renderPage() {
  if (!pdfDocument.value || !canvas.value || !pageStage.value) return;
  renderTask?.cancel();
  const page = await pdfDocument.value.getPage(currentPage.value);
  const base = page.getViewport({ scale: 1 });
  const availableWidth = Math.max(260, pageStage.value.clientWidth - 48);
  const availableHeight = Math.max(360, pageStage.value.clientHeight - 48);
  const scale = Math.min(1.6, availableWidth / base.width, availableHeight / base.height);
  const viewport = page.getViewport({ scale });
  const outputScale = Math.min(window.devicePixelRatio || 1, 2);
  const context = canvas.value.getContext('2d', { alpha: false });
  if (!context) return;
  canvas.value.width = Math.floor(viewport.width * outputScale);
  canvas.value.height = Math.floor(viewport.height * outputScale);
  canvas.value.style.width = `${Math.floor(viewport.width)}px`;
  canvas.value.style.height = `${Math.floor(viewport.height)}px`;
  viewportWidth.value = viewport.width;
  viewportHeight.value = viewport.height;
  renderTask = page.render({
    canvas: canvas.value,
    canvasContext: context,
    viewport,
    transform: outputScale === 1 ? undefined : [outputScale, 0, 0, outputScale, 0, 0],
  });
  try {
    await renderTask.promise;
  } catch (err) {
    if (!(err instanceof Error) || err.name !== 'RenderingCancelledException') throw err;
  }
}

async function loadDocumentPreview(document: SigningDocument, page = 1) {
  const blob = signingDocumentPreview(document) === 'signed'
    ? await api.signedDocument(document.id)
    : await api.signingDocumentSource(document.id);
  const nextPdf = await pdfjsLib.getDocument({ data: await blob.arrayBuffer() }).promise;
  renderTask?.cancel();
  renderTask = null;
  await pdfDocument.value?.destroy();
  pdfDocument.value = nextPdf;
  currentPage.value = Math.min(document.page_count, Math.max(1, page));
  await nextTick();
  await renderPage();
}

async function openDocument(document: SigningDocument) {
  opening.value = true;
  error.value = '';
  currentDocument.value = document;
  placements.value = [];
  selectedPlacementId.value = '';
  stampDropActive.value = false;
  seamStartPage.value = 1;
  seamEndPage.value = Math.min(2, document.page_count);
  try {
    await loadDocumentPreview(document);
  } catch (err) {
    await closeDocument();
    error.value = err instanceof Error ? err.message : '打开文档失败';
  } finally {
    opening.value = false;
  }
}

async function closeDocument() {
  renderTask?.cancel();
  renderTask = null;
  await pdfDocument.value?.destroy();
  pdfDocument.value = null;
  currentDocument.value = null;
  placements.value = [];
  selectedPlacementId.value = '';
  stampDropActive.value = false;
  endDrag();
}

async function uploadDocument(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = '';
  if (!file) return;
  if (file.size > 30 * 1024 * 1024) {
    error.value = '文档不能超过 30 MiB';
    return;
  }
  uploading.value = true;
  error.value = '';
  try {
    const uploaded = await api.uploadSigningDocument(file);
    documents.value = [uploaded, ...documents.value.filter((item) => item.id !== uploaded.id)];
    await openDocument(uploaded);
  } catch (err) {
    error.value = err instanceof Error ? err.message : '上传文档失败';
  } finally {
    uploading.value = false;
  }
}

function selectSeal(seal: Seal) {
  selectedSealId.value = seal.id;
  selectedPlacementId.value = '';
}

function addPlacement(seal: Seal, centerX: number, centerY: number) {
  if (!currentDocument.value || documentFinalized.value || !viewportWidth.value || !viewportHeight.value) {
    error.value = '文档页面仍在准备，请稍后再试';
    return;
  }
  const width = stampWidth.value / 100;
  const height = width * (seal.height_px / seal.width_px) * (viewportWidth.value / viewportHeight.value);
  const placementHeight = Math.min(height, 0.8);
  const placement: PendingPlacement = {
    id: nextPlacementId(),
    seal_id: seal.id,
    page: currentPage.value,
    x: Math.min(1 - width, Math.max(0, centerX - width / 2)),
    y: Math.min(1 - placementHeight, Math.max(0, centerY - placementHeight / 2)),
    width,
    height: placementHeight,
  };
  error.value = '';
  placements.value.push(placement);
  selectedPlacementId.value = placement.id;
}

function addStamp() {
  if (selectedSeal.value) addPlacement(selectedSeal.value, 0.5, 0.5);
}

function beginSealDrag(event: DragEvent, seal: Seal) {
  if (documentFinalized.value) return;
  mode.value = 'single';
  selectedSealId.value = seal.id;
  selectedPlacementId.value = '';
  if (!event.dataTransfer) return;
  event.dataTransfer.effectAllowed = 'copy';
  event.dataTransfer.setData('application/x-crewdock-seal', seal.id);
  event.dataTransfer.setData('text/plain', seal.id);
}

function addSealToCenter(seal: Seal) {
  mode.value = 'single';
  selectedSealId.value = seal.id;
  addPlacement(seal, 0.5, 0.5);
}

function leaveSealDrop(event: DragEvent) {
  const next = event.relatedTarget;
  if (!(next instanceof Node) || !pageWrap.value?.contains(next)) stampDropActive.value = false;
}

function activateStampDrop() {
  if (!documentFinalized.value) stampDropActive.value = true;
}

function dropSeal(event: DragEvent) {
  stampDropActive.value = false;
  if (documentFinalized.value || !pageWrap.value || !event.dataTransfer) return;
  const sealId = event.dataTransfer.getData('application/x-crewdock-seal')
    || event.dataTransfer.getData('text/plain');
  const seal = seals.value.find((item) => item.id === sealId);
  if (!seal) return;
  const rect = pageWrap.value.getBoundingClientRect();
  if (!rect.width || !rect.height) return;
  selectedSealId.value = seal.id;
  addPlacement(
    seal,
    (event.clientX - rect.left) / rect.width,
    (event.clientY - rect.top) / rect.height,
  );
}

function resizeSelected() {
  const placement = selectedPlacement.value;
  const seal = seals.value.find((item) => item.id === placement?.seal_id);
  if (!placement || !seal || !viewportWidth.value || !viewportHeight.value) return;
  const width = stampWidth.value / 100;
  const height = width * (seal.height_px / seal.width_px) * (viewportWidth.value / viewportHeight.value);
  placement.width = width;
  placement.height = Math.min(height, 0.8);
  placement.x = Math.min(placement.x, 1 - placement.width);
  placement.y = Math.min(placement.y, 1 - placement.height);
}

function removeSelected() {
  if (!selectedPlacementId.value) return;
  placements.value = placements.value.filter((item) => item.id !== selectedPlacementId.value);
  selectedPlacementId.value = '';
}

function clearPreview() {
  if (documentFinalized.value) return;
  if (mode.value === 'single') {
    placements.value = [];
    selectedPlacementId.value = '';
  } else {
    seamY.value = 35;
    seamHeight.value = 30;
  }
}

function beginDrag(event: PointerEvent, placement?: PendingPlacement) {
  if (documentFinalized.value) return;
  event.preventDefault();
  if (placement) {
    selectedPlacementId.value = placement.id;
    selectedSealId.value = placement.seal_id;
    stampWidth.value = Math.round(placement.width * 100);
    dragging = {
      kind: 'single',
      id: placement.id,
      startX: event.clientX,
      startY: event.clientY,
      initialX: placement.x,
      initialY: placement.y,
    };
  } else {
    dragging = {
      kind: 'seam',
      startX: event.clientX,
      startY: event.clientY,
      initialX: 0,
      initialY: seamY.value / 100,
    };
  }
  window.addEventListener('pointermove', moveDrag);
  window.addEventListener('pointerup', endDrag, { once: true });
  window.addEventListener('pointercancel', endDrag, { once: true });
}

function moveDrag(event: PointerEvent) {
  if (!dragging || !pageWrap.value) return;
  const rect = pageWrap.value.getBoundingClientRect();
  if (dragging.kind === 'single') {
    const placement = placements.value.find((item) => item.id === dragging?.id);
    if (!placement) return;
    placement.x = Math.min(1 - placement.width, Math.max(0, dragging.initialX + (event.clientX - dragging.startX) / rect.width));
    placement.y = Math.min(1 - placement.height, Math.max(0, dragging.initialY + (event.clientY - dragging.startY) / rect.height));
  } else {
    const nextY = dragging.initialY + (event.clientY - dragging.startY) / rect.height;
    seamY.value = Math.round(Math.min(1 - seamHeight.value / 100, Math.max(0, nextY)) * 100);
  }
}

function endDrag() {
  dragging = null;
  window.removeEventListener('pointermove', moveDrag);
  window.removeEventListener('pointerup', endDrag);
  window.removeEventListener('pointercancel', endDrag);
}

function changePage(next: number) {
  if (!currentDocument.value) return;
  currentPage.value = Math.min(currentDocument.value.page_count, Math.max(1, next));
}

function setMode(next: 'single' | 'seam') {
  if (documentFinalized.value) return;
  mode.value = next;
  selectedPlacementId.value = '';
}

function openConfirmation() {
  if (!canFinalize.value || documentFinalized.value) return;
  confirmOpen.value = true;
}

async function confirmFinalize() {
  const document = currentDocument.value;
  if (!document) return;
  const payload: FinalizeSigningRequest = mode.value === 'single'
    ? {
        mode: 'single',
        placements: placements.value.map(({ id: _id, ...placement }) => placement),
      }
    : {
        mode: 'seam',
        placements: [],
        seam: {
          seal_id: selectedSealId.value,
          start_page: seamStartPage.value,
          end_page: seamEndPage.value,
          y: seamY.value / 100,
          height: seamHeight.value / 100,
        },
      };
  finalizing.value = true;
  error.value = '';
  try {
    const updated = await api.finalizeSigningDocument(document.id, payload);
    currentDocument.value = updated;
    documents.value = documents.value.map((item) => item.id === updated.id ? updated : item);
    placements.value = [];
    selectedPlacementId.value = '';
    stampDropActive.value = false;
    endDrag();
    confirmOpen.value = false;
    opening.value = true;
    try {
      await loadDocumentPreview(updated, currentPage.value);
    } catch {
      error.value = '签章已经合并并保存，但最终文档预览刷新失败，请返回后重新打开';
    } finally {
      opening.value = false;
    }
  } catch (err) {
    error.value = err instanceof Error ? err.message : '确认签章失败';
  } finally {
    finalizing.value = false;
  }
}

function saveBlob(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = filename;
  anchor.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function filenameStem(value: string) {
  return value.replace(/\.[^.]+$/, '') || 'signed-document';
}

async function downloadResult(kind: 'pdf' | 'images') {
  const document = currentDocument.value;
  if (!document) return;
  exporting.value = kind;
  error.value = '';
  try {
    if (kind === 'pdf') {
      saveBlob(await api.signedDocument(document.id), `${filenameStem(document.original_filename)}-signed.pdf`);
    } else {
      saveBlob(
        await api.signedDocumentImages(document.id),
        signedImageDownloadName(filenameStem(document.original_filename), document.page_count),
      );
    }
  } catch (err) {
    error.value = err instanceof Error ? err.message : '导出签章结果失败';
  } finally {
    exporting.value = '';
  }
}

async function removeDocument(document: SigningDocument) {
  if (!window.confirm(`确定删除“${document.original_filename}”及其签章结果吗？`)) return;
  error.value = '';
  try {
    if (currentDocument.value?.id === document.id) await closeDocument();
    await api.deleteSigningDocument(document.id);
    documents.value = documents.value.filter((item) => item.id !== document.id);
  } catch (err) {
    error.value = err instanceof Error ? err.message : '删除文档失败';
  }
}

function formatSize(value: number) {
  return value < 1024 * 1024 ? `${Math.ceil(value / 1024)} KiB` : `${(value / 1024 / 1024).toFixed(1)} MiB`;
}

function formatTime(value?: string | null) {
  return value ? new Date(value).toLocaleString('zh-CN', { hour12: false }) : '暂无记录';
}

watch(currentPage, () => renderPage().catch((err) => {
  error.value = err instanceof Error ? err.message : '渲染 PDF 页面失败';
}));
watch(pageStage, (next, previous) => {
  if (previous) resizeObserver?.unobserve(previous);
  if (next) resizeObserver?.observe(next);
});
watch(seamHeight, () => {
  seamY.value = Math.min(seamY.value, 100 - seamHeight.value);
});
watch(seamStartPage, (value) => {
  if (!currentDocument.value) return;
  seamStartPage.value = Math.min(Math.max(1, value), Math.max(1, currentDocument.value.page_count - 1));
  seamEndPage.value = Math.max(seamEndPage.value, Math.min(currentDocument.value.page_count, seamStartPage.value + 1));
});

onMounted(async () => {
  await load();
  resizeObserver = new ResizeObserver(() => {
    if (pdfDocument.value) renderPage().catch(() => undefined);
  });
  if (pageStage.value) resizeObserver.observe(pageStage.value);
});

onBeforeUnmount(() => {
  renderTask?.cancel();
  pdfDocument.value?.destroy();
  resizeObserver?.disconnect();
  window.removeEventListener('pointermove', moveDrag);
  window.removeEventListener('pointerup', endDrag);
  window.removeEventListener('pointercancel', endDrag);
  Object.values(sealImages.value).forEach(revoke);
});
</script>

<template>
  <WorkbenchShell layout="workspace">
    <div class="signing-tool">
      <header class="signing-toolbar">
        <div class="signing-toolbar-title">
          <button v-if="currentDocument" class="icon-button" type="button" title="返回文档列表" aria-label="返回文档列表" @click="closeDocument"><ArrowLeft :size="17" /></button>
          <FilePenLine :size="19" />
          <div><strong>{{ currentDocument?.original_filename || '签章工具' }}</strong><span v-if="currentDocument">{{ currentDocument.page_count }} 页 · {{ currentDocument.source_format.toUpperCase() }}</span><span v-else>PDF、Word、PNG、JPG、WebP</span></div>
        </div>
        <div v-if="currentDocument && !documentFinalized" class="signing-mode" aria-label="签章模式">
          <button type="button" :class="{ active: mode === 'single' }" @click="setMode('single')"><StampIcon :size="16" />单个签章</button>
          <button type="button" :class="{ active: mode === 'seam' }" @click="setMode('seam')"><FileArchive :size="16" />骑缝章</button>
        </div>
        <div v-else-if="currentDocument" class="signing-finalized-status"><CircleCheck :size="17" /><span><strong>签章已合并</strong><small>{{ formatTime(currentDocument.signed_at) }}</small></span></div>
        <div class="signing-toolbar-actions">
          <button class="secondary-button" type="button" :disabled="uploading" @click="fileInput?.click()"><LoaderCircle v-if="uploading" class="spin" :size="16" /><FilePlus2 v-else :size="16" />{{ uploading ? '处理中...' : '新建文档' }}</button>
          <input ref="fileInput" hidden type="file" accept=".pdf,.doc,.docx,.png,.jpg,.jpeg,.webp,application/pdf,application/msword,application/vnd.openxmlformats-officedocument.wordprocessingml.document,image/png,image/jpeg,image/webp" @change="uploadDocument" />
          <button v-if="currentDocument && !documentFinalized" class="primary-button" type="button" :disabled="!canFinalize || finalizing" @click="openConfirmation"><CircleCheck :size="17" />确认签章</button>
        </div>
      </header>

      <p v-if="error" class="signing-error error" role="alert">{{ error }}</p>

      <div v-if="loading" class="loading-state signing-loading"><LoaderCircle class="spin" :size="22" /><span>正在打开签章工作区...</span></div>
      <div v-else-if="!currentDocument" class="signing-start">
        <section class="signing-dropzone" @click="fileInput?.click()">
          <div class="signing-drop-icon"><Upload :size="30" /></div>
          <h1>选择需要签章的文档</h1>
          <p>支持 PDF、Word、PNG、JPG、WebP，单个文件最大 30 MiB。</p>
          <button class="primary-button" type="button" :disabled="uploading"><FilePlus2 :size="17" />选择文档</button>
        </section>
        <section v-if="documents.length" class="signing-recents">
          <header><h2>文档记录</h2><span>{{ documents.length }} 个</span></header>
          <div class="signing-recent-list">
            <article v-for="document in documents" :key="document.id" class="signing-recent-item">
              <button class="signing-recent-open" type="button" @click="openDocument(document)">
                <span class="document-type">{{ document.source_format.toUpperCase() }}</span>
                <span><strong>{{ document.original_filename }}</strong><small>{{ document.page_count }} 页 · {{ formatSize(document.source_size_bytes) }} · {{ document.has_signed_result ? `已签章 ${formatTime(document.signed_at)}` : '待签章' }}</small></span>
                <CircleCheck v-if="document.has_signed_result" class="document-signed" :size="17" />
                <ChevronRight :size="17" />
              </button>
              <button class="icon-button danger-icon" type="button" title="删除文档" aria-label="删除文档" @click="removeDocument(document)"><Trash2 :size="16" /></button>
            </article>
          </div>
        </section>
      </div>

      <div v-else class="signing-workspace" :class="{ opening }">
        <section ref="pageStage" class="pdf-stage">
          <div v-if="opening" class="pdf-opening"><LoaderCircle class="spin" :size="24" /><span>正在解析文档...</span></div>
          <div
            ref="pageWrap"
            class="pdf-page-wrap"
            :class="{ 'stamp-drop-active': stampDropActive, finalized: documentFinalized }"
            :style="{ width: `${viewportWidth}px`, height: `${viewportHeight}px` }"
            @dragenter.prevent="activateStampDrop"
            @dragover.prevent
            @dragleave="leaveSealDrop"
            @drop.prevent="dropSeal"
          >
            <canvas ref="canvas" />
            <template v-if="!documentFinalized && mode === 'single'">
              <button
                v-for="placement in pagePlacements"
                :key="placement.id"
                class="stamp-overlay"
                :class="{ selected: selectedPlacementId === placement.id }"
                type="button"
                :aria-label="`移动${seals.find((item) => item.id === placement.seal_id)?.name || '印章'}`"
                :style="{ left: `${placement.x * 100}%`, top: `${placement.y * 100}%`, width: `${placement.width * 100}%`, height: `${placement.height * 100}%` }"
                @pointerdown="beginDrag($event, placement)"
              ><img :src="sealImages[placement.seal_id]" alt="" draggable="false" /></button>
            </template>
            <button
              v-else-if="!documentFinalized && seamVisible"
              class="seam-overlay"
              type="button"
              aria-label="移动骑缝章"
              :style="{ top: `${seamY}%`, width: `${seamWidthPercent}%`, height: `${seamHeight}%` }"
              @pointerdown="beginDrag($event)"
            >
              <img :src="sealImages[selectedSealId]" alt="" draggable="false" :style="{ width: `${seamPageCount * 100}%`, left: `${-seamSliceIndex * 100}%` }" />
            </button>
          </div>
          <div class="pdf-pager">
            <button class="icon-button" type="button" title="上一页" aria-label="上一页" :disabled="currentPage <= 1" @click="changePage(currentPage - 1)"><ChevronLeft :size="17" /></button>
            <span><input v-model.number="currentPage" type="number" min="1" :max="currentDocument.page_count" @change="changePage(currentPage)" /> / {{ currentDocument.page_count }}</span>
            <button class="icon-button" type="button" title="下一页" aria-label="下一页" :disabled="currentPage >= currentDocument.page_count" @click="changePage(currentPage + 1)"><ChevronRight :size="17" /></button>
          </div>
        </section>

        <aside class="signing-panel">
          <template v-if="documentFinalized">
            <div class="signing-panel-head"><div><strong>签章记录</strong><span>最终文档已固化，不能再移动印章</span></div><CircleCheck class="document-signed" :size="20" /></div>
            <section class="signing-record">
              <div class="signing-record-state"><span><CircleCheck :size="19" /></span><div><strong>签章合并完成</strong><small>最终 PDF 已保存</small></div></div>
              <dl>
                <div><dt>完成时间</dt><dd>{{ formatTime(currentDocument.signed_at) }}</dd></div>
                <div><dt>文档页数</dt><dd>{{ currentDocument.page_count }} 页</dd></div>
                <div><dt>原始格式</dt><dd>{{ currentDocument.source_format.toUpperCase() }}</dd></div>
                <div><dt>记录状态</dt><dd>已保存</dd></div>
              </dl>
            </section>
            <div class="signing-panel-footer">
              <div class="signed-exports">
                <span><CircleCheck :size="16" />下载最终签章文件</span>
                <button class="secondary-button" type="button" :disabled="!!exporting" @click="downloadResult('pdf')"><LoaderCircle v-if="exporting === 'pdf'" class="spin" :size="16" /><Download v-else :size="16" />PDF</button>
                <button class="secondary-button" type="button" :disabled="!!exporting" @click="downloadResult('images')"><LoaderCircle v-if="exporting === 'images'" class="spin" :size="16" /><Images v-else :size="16" />{{ currentDocument.page_count === 1 ? 'PNG' : '图片 ZIP' }}</button>
              </div>
              <div class="signing-boundary compact"><Info :size="15" /><span>可视化签章，不含 CA 证书。</span></div>
            </div>
          </template>
          <template v-else>
            <div class="signing-panel-head"><div><strong>可用印章</strong><span>{{ seals.length }} 枚已授权印章</span></div><RouterLink class="icon-button" to="/seals" title="查看印章" aria-label="查看印章"><Maximize2 :size="16" /></RouterLink></div>
            <div v-if="seals.length" class="tool-seal-list">
              <button
                v-for="seal in seals"
                :key="seal.id"
                type="button"
                draggable="true"
                title="拖到文件，或双击放到当前页"
                :class="{ active: selectedSealId === seal.id }"
                @click="selectSeal(seal)"
                @dblclick="addSealToCenter(seal)"
                @dragstart="beginSealDrag($event, seal)"
                @dragend="stampDropActive = false"
              >
                <span><img v-if="sealImages[seal.id]" :src="sealImages[seal.id]" alt="" /><FileImage v-else :size="21" /></span><strong>{{ seal.name }}</strong><small>{{ seal.width_px }} × {{ seal.height_px }}</small>
              </button>
            </div>
            <div v-else class="tool-seal-empty"><StampIcon :size="24" /><strong>没有可用印章</strong><RouterLink class="secondary-button as-link" to="/seals">查看授权状态</RouterLink></div>

            <div v-if="selectedSeal && mode === 'single'" class="signing-controls">
              <button class="primary-button signing-add-stamp" type="button" @click="addStamp"><StampIcon :size="17" />放到当前页</button>
              <label><span>印章宽度 <strong>{{ stampWidth }}%</strong></span><input v-model.number="stampWidth" type="range" min="6" max="40" step="1" @input="resizeSelected" /></label>
              <div v-if="selectedPlacement" class="selected-placement-row"><span>已选中当前页印章</span><button class="icon-button danger-icon" type="button" title="移除印章" aria-label="移除印章" @click="removeSelected"><Trash2 :size="16" /></button></div>
            </div>

            <div v-else-if="selectedSeal" class="signing-controls seam-controls">
              <div class="seam-page-range"><label class="field"><span>起始页</span><input v-model.number="seamStartPage" type="number" min="1" :max="Math.max(1, currentDocument.page_count - 1)" /></label><label class="field"><span>结束页</span><input v-model.number="seamEndPage" type="number" :min="Math.min(currentDocument.page_count, seamStartPage + 1)" :max="currentDocument.page_count" /></label></div>
              <label><span>垂直位置 <strong>{{ seamY }}%</strong></span><input v-model.number="seamY" type="range" min="0" :max="100 - seamHeight" step="1" /></label>
              <label><span>印章高度 <strong>{{ seamHeight }}%</strong></span><input v-model.number="seamHeight" type="range" min="10" max="55" step="1" /></label>
              <div class="seam-summary"><FileArchive :size="17" /><span>印章会切为 {{ seamPageCount }} 份并贴合所选页面右边缘。</span></div>
            </div>

            <div class="signing-panel-footer">
              <button class="secondary-button" type="button" @click="clearPreview"><RotateCcw :size="16" />重置预览</button>
              <div class="signing-boundary compact"><Info :size="15" /><span>可视化签章，不含 CA 证书。</span></div>
            </div>
          </template>
        </aside>
      </div>

      <div v-if="confirmOpen" class="modal-backdrop" role="presentation" @mousedown.self="confirmOpen = false">
        <section class="signing-confirm-dialog" role="alertdialog" aria-modal="true" aria-label="确认合并签章">
          <header><div class="confirm-mark"><StampIcon :size="22" /></div><button class="icon-button" type="button" title="关闭" aria-label="关闭" :disabled="finalizing" @click="confirmOpen = false"><X :size="18" /></button></header>
          <h2>确定合并签章？</h2>
          <p>确认后会生成最终签章 PDF 并保存本次记录；印章将固化，不能继续拖动或覆盖。</p>
          <dl><div><dt>签章模式</dt><dd>{{ mode === 'single' ? '单个签章' : '骑缝章' }}</dd></div><div><dt>作用范围</dt><dd>{{ mode === 'single' ? `${placements.length} 个位置` : `第 ${seamStartPage}–${seamEndPage} 页` }}</dd></div></dl>
          <footer><button class="secondary-button" type="button" :disabled="finalizing" @click="confirmOpen = false">返回调整</button><button class="primary-button" type="button" :disabled="finalizing" @click="confirmFinalize"><LoaderCircle v-if="finalizing" class="spin" :size="16" /><CircleCheck v-else :size="16" />{{ finalizing ? '正在合并...' : '确定并合并' }}</button></footer>
        </section>
      </div>
    </div>
  </WorkbenchShell>
</template>
