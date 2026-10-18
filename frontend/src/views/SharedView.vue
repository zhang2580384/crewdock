<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { ExternalLink, ShieldCheck } from '@lucide/vue';
import { MdPreview } from 'md-editor-v3';
import 'md-editor-v3/lib/style.css';
import { useRoute } from 'vue-router';
import { api } from '../api';
import type { ShareLink } from '../types';

const route = useRoute();
const share = ref<ShareLink | null>(null);
const target = ref<Record<string, unknown>>({});
const loading = ref(true);
const error = ref('');

const title = computed(() => String(target.value.title || target.value.name || share.value?.title || '分享内容'));
const body = computed(() => String(target.value.body_markdown || target.value.description_markdown || ''));

async function load() {
  const id = String(route.params.id || '');
  const token = String(route.query.token || '');
  if (!id || !token) {
    error.value = '分享链接无效';
    loading.value = false;
    return;
  }
  try {
    const result = await api.resolveShareLink(id, token);
    share.value = result.share;
    target.value = result.target;
  } catch (err) {
    error.value = err instanceof Error ? err.message : '分享链接无效或已过期';
  } finally {
    loading.value = false;
  }
}

onMounted(load);
</script>

<template>
  <main class="shared-page">
    <header class="shared-header">
      <div class="brand-mark"><ShieldCheck :size="20" /></div>
      <strong>CrewDock</strong>
      <span>内部分享</span>
      <a href="https://github.com/zhang2580384/crewdock" target="_blank" rel="noreferrer" title="AGPL-3.0-or-later 源代码">源码</a>
    </header>
    <section v-if="loading" class="shared-content"><p>正在打开分享...</p></section>
    <section v-else-if="error" class="shared-content"><h1>无法打开</h1><p class="error">{{ error }}</p></section>
    <article v-else class="shared-content">
      <div class="shared-meta">
        <span>{{ share?.title }}</span>
        <span v-if="share?.expires_at">有效期至 {{ new Date(share.expires_at).toLocaleString() }}</span>
      </div>
      <h1>{{ title }}</h1>
      <p v-if="target.description && share?.target_type === 'project'">{{ target.description }}</p>
      <a v-if="share?.target_type === 'project_link'" class="primary-button shared-link" :href="String(target.url)" target="_blank" rel="noreferrer">
        <ExternalLink :size="17" />打开链接
      </a>
      <MdPreview v-if="body" :model-value="body" />
    </article>
  </main>
</template>

<style scoped>
.shared-page { min-height: 100vh; background: #f4f6f8; color: #17202a; }
.shared-header { min-height: 64px; display: flex; align-items: center; gap: 10px; padding: 0 max(20px, calc((100vw - 920px) / 2)); border-bottom: 1px solid #dce2e8; background: #fff; }
.shared-header span { margin-left: auto; color: #65717d; font-size: 13px; }
.shared-content { width: min(920px, calc(100% - 32px)); margin: 0 auto; padding: 42px 0 64px; }
.shared-content h1 { margin: 12px 0 22px; font-size: 30px; letter-spacing: 0; overflow-wrap: anywhere; }
.shared-meta { display: flex; justify-content: space-between; gap: 12px; color: #65717d; font-size: 13px; }
.shared-link { width: fit-content; text-decoration: none; }
@media (max-width: 600px) { .shared-meta { flex-direction: column; } .shared-content h1 { font-size: 24px; } }
</style>
