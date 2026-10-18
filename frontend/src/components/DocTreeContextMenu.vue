<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue';
import { Eye, FilePlus2, FolderInput, FolderPlus, Pencil, ShieldCheck, Trash2 } from '@lucide/vue';

const props = defineProps<{
  kind: 'folder' | 'doc';
  x: number;
  y: number;
  canEdit: boolean;
  canManageAccess: boolean;
}>();

const emit = defineEmits<{
  action: [action: 'preview' | 'edit' | 'new-folder' | 'new-doc' | 'rename' | 'move' | 'permissions' | 'delete'];
  close: [];
}>();

const menu = ref<HTMLElement | null>(null);
const style = computed(() => {
  const width = 204;
  const itemCount = props.kind === 'folder'
    ? (props.canEdit ? 5 : 0) + (props.canManageAccess ? 1 : 0)
    : 1 + (props.canEdit ? 4 : 0) + (props.canManageAccess ? 1 : 0);
  const height = 10 + itemCount * 38 + 5;
  return {
    left: `${Math.max(8, Math.min(props.x, window.innerWidth - width - 8))}px`,
    top: `${Math.max(8, Math.min(props.y, window.innerHeight - height - 8))}px`,
  };
});

function choose(action: 'preview' | 'edit' | 'new-folder' | 'new-doc' | 'rename' | 'move' | 'permissions' | 'delete') {
  emit('action', action);
  emit('close');
}

function close() {
  emit('close');
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    close();
    return;
  }
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
  const items = [...(menu.value?.querySelectorAll<HTMLButtonElement>('button') || [])];
  if (!items.length) return;
  event.preventDefault();
  const current = Math.max(0, items.indexOf(document.activeElement as HTMLButtonElement));
  const next = event.key === 'Home'
    ? 0
    : event.key === 'End'
      ? items.length - 1
      : event.key === 'ArrowDown'
        ? (current + 1) % items.length
        : (current - 1 + items.length) % items.length;
  items[next]?.focus();
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
    <div ref="menu" class="doc-tree-menu" :style="style" role="menu" @pointerdown.stop @contextmenu.prevent>
      <template v-if="kind === 'folder'">
        <template v-if="canEdit">
          <button type="button" role="menuitem" @click="choose('new-doc')"><FilePlus2 :size="16" /><span>新建文档</span></button>
          <button type="button" role="menuitem" @click="choose('new-folder')"><FolderPlus :size="16" /><span>新建子文件夹</span></button>
          <button type="button" role="menuitem" @click="choose('rename')"><Pencil :size="16" /><span>重命名</span></button>
          <button type="button" role="menuitem" @click="choose('move')"><FolderInput :size="16" /><span>移动到</span></button>
        </template>
        <button v-if="canManageAccess" type="button" role="menuitem" @click="choose('permissions')"><ShieldCheck :size="16" /><span>部门权限</span></button>
        <button v-if="canEdit" class="danger" type="button" role="menuitem" @click="choose('delete')"><Trash2 :size="16" /><span>删除文件夹</span></button>
      </template>
      <template v-else-if="kind === 'doc'">
        <button type="button" role="menuitem" @click="choose('preview')"><Eye :size="16" /><span>预览</span></button>
        <template v-if="canEdit">
          <button type="button" role="menuitem" @click="choose('edit')"><Pencil :size="16" /><span>编辑</span></button>
          <button type="button" role="menuitem" @click="choose('rename')"><Pencil :size="16" /><span>重命名</span></button>
          <button type="button" role="menuitem" @click="choose('move')"><FolderInput :size="16" /><span>移动到</span></button>
          <button class="danger" type="button" role="menuitem" @click="choose('delete')"><Trash2 :size="16" /><span>删除文档</span></button>
        </template>
        <button v-if="canManageAccess" type="button" role="menuitem" @click="choose('permissions')"><ShieldCheck :size="16" /><span>部门权限</span></button>
      </template>
    </div>
  </Teleport>
</template>

<style scoped>
.doc-tree-menu {
  position: fixed;
  z-index: 1300;
  width: 204px;
  padding: 5px;
  border: 1px solid var(--line-strong);
  border-radius: 7px;
  background: #fff;
  box-shadow: 0 16px 38px rgba(18, 27, 35, .2);
}

button {
  width: 100%;
  min-height: 38px;
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 8px 10px;
  border: 0;
  border-radius: 5px;
  background: transparent;
  color: var(--text);
  font: inherit;
  text-align: left;
  cursor: pointer;
}

button:hover,
button:focus-visible {
  outline: 0;
  background: var(--primary-soft);
  color: var(--primary-strong);
}

button.danger {
  margin-top: 4px;
  border-top: 1px solid var(--line);
  border-radius: 0 0 5px 5px;
  color: var(--red);
}
</style>
