<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue';
import { Building2, ExternalLink, Monitor, Pencil, Server } from '@lucide/vue';
import { siteLaunchHref } from '../site-launch';
import type { Site } from '../types';

const props = defineProps<{
  site: Site;
  x: number;
  y: number;
  canEdit: boolean;
}>();

const emit = defineEmits<{
  close: [];
  edit: [site: Site];
  opened: [site: Site];
}>();

const menu = ref<HTMLElement | null>(null);

const style = computed(() => {
  const width = 190;
  const itemCount = 1 + (props.site.internal_url ? 1 : 0) + (props.canEdit ? 1 : 0);
  const height = 10 + itemCount * 38 + (props.canEdit ? 5 : 0);
  return {
    left: `${Math.max(8, Math.min(props.x, window.innerWidth - width - 8))}px`,
    top: `${Math.max(8, Math.min(props.y, window.innerHeight - height - 8))}px`,
  };
});

function openSite(site: Site) {
  emit('opened', site);
  const url = siteLaunchHref(site);
  if (site.resource_type === 'website') window.open(url, '_blank', 'noopener,noreferrer');
  else window.location.href = url;
  emit('close');
}

function openWebsite(url: string) {
  emit('opened', props.site);
  window.open(url, '_blank', 'noopener,noreferrer');
  emit('close');
}

function edit() {
  emit('edit', props.site);
  emit('close');
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') emit('close');
}

function close() {
  emit('close');
}

onMounted(async () => {
  document.addEventListener('pointerdown', close);
  window.addEventListener('keydown', onKeydown);
  window.addEventListener('resize', close);
  window.addEventListener('scroll', close, true);
  await nextTick();
  menu.value?.querySelector<HTMLButtonElement>('button')?.focus();
});

onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', close);
  window.removeEventListener('keydown', onKeydown);
  window.removeEventListener('resize', close);
  window.removeEventListener('scroll', close, true);
});
</script>

<template>
  <Teleport to="body">
    <div ref="menu" class="site-context-menu" :style="style" role="menu" @pointerdown.stop @contextmenu.prevent>
      <button type="button" role="menuitem" @click="openSite(site)">
        <Server v-if="site.resource_type === 'ssh'" :size="16" />
        <Monitor v-else-if="site.resource_type === 'rdp'" :size="16" />
        <ExternalLink v-else :size="16" />
        <span>{{ site.resource_type === 'website' ? '外网打开' : '连接服务器' }}</span>
      </button>
      <button v-if="site.internal_url" type="button" role="menuitem" @click="openWebsite(site.internal_url)">
        <Building2 :size="16" />
        <span>内网打开</span>
      </button>
      <button v-if="canEdit" class="context-edit" type="button" role="menuitem" @click="edit">
        <Pencil :size="16" />
        <span>编辑入口</span>
      </button>
    </div>
  </Teleport>
</template>
