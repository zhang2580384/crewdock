<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { History, Search } from '@lucide/vue';
import WorkbenchShell from '../components/WorkbenchShell.vue';
import { api } from '../api';
import type { AuditEvent } from '../types';

const events = ref<AuditEvent[]>([]);
const query = ref('');
const loading = ref(false);
const error = ref('');

const actionLabels: Record<string, string> = {
  'auth.login': '用户登录',
  'auth.change_password': '修改登录密码',
  'seal.create': '创建企业印章',
  'seal.update': '更新企业印章',
  'seal.replace': '替换企业印章',
  'seal.delete': '删除企业印章',
  'seal.authorization.grant': '授权员工使用印章',
  'seal.authorization.revoke': '撤销员工印章授权',
  'seal.generator_template.update': '更新印章默认样式',
  'signing.document.upload': '上传签章文档',
  'signing.document.finalize': '确认合并签章',
  'signing.document.delete': '删除签章文档',
  'user.create': '创建用户',
  'user.update': '更新用户',
  'user.reset_password': '重置用户密码',
  'site.create': '添加站点',
  'site.update': '更新站点',
  'site.delete': '删除站点',
  'site.import_bookmarks': '导入浏览器书签',
  'project.create': '创建项目',
  'project.update': '更新项目',
  'project.delete': '删除项目',
  'project_link.update': '更新项目链接',
  'project_link.delete': '删除项目链接',
  'markdown_doc.update': '更新文档',
  'markdown_doc.move': '移动文档',
  'markdown_doc.delete': '删除文档',
  'markdown_folder.create': '创建文档文件夹',
  'markdown_folder.update': '更新文档文件夹',
  'markdown_folder.move': '移动文档文件夹',
  'markdown_folder.delete': '删除文档文件夹',
  'vault.create': '创建保险库凭据',
  'vault.update': '更新保险库凭据',
  'vault.delete': '删除保险库凭据',
  'vault.reveal': '查看保险库敏感字段',
  'file.upload': '上传文件',
  'file.delete': '删除文件',
  'ticket.create': '创建工单',
  'ticket.update': '更新工单',
  'ticket.status_update': '变更工单状态',
  'ticket.delete': '删除工单',
  'share_link.create': '创建共享链接',
  'share_link.resolve': '访问共享链接',
  'share_link.file_download': '通过共享链接下载文件',
};

const targetLabels: Record<string, string> = {
  user: '用户', site: '站点', project: '项目', project_link: '项目链接', markdown_doc: '文档',
  markdown_folder: '文档文件夹',
  vault_item: '保险库凭据', file_asset: '文件', ticket: '工单', share_link: '共享链接',
  user_seal: '企业印章', signing_document: '签章文档',
};
const statusLabels: Record<string, string> = {
  pending: '待处理', in_progress: '处理中', waiting_confirm: '待确认', done: '已完成', closed: '已关闭',
};
const roleLabels: Record<string, string> = {
  system_admin: '系统管理员', department_admin: '部门管理员', project_admin: '项目管理员',
  project_member: '项目成员', readonly_member: '只读成员',
};
const visibilityLabels: Record<string, string> = { public: '团队公共', personal: '个人收藏' };
const metadataLabels: Record<string, string> = {
  email: '账号', name: '名称', title: '标题', slug: 'Slug', filename: '文件', size: '大小', status: '状态',
  role: '角色', visibility: '范围', imported: '导入', skipped: '跳过', sessions_revoked: '注销会话',
  secret_updated: '敏感字段', target_type: '共享类型',
  seal_count: '印章数量', mode: '签章模式', source_kind: '印章来源', page_count: '页数',
};

function humanize(value: string) {
  return value.replace(/[._-]+/g, ' ').replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function valueLabel(key: string, value: unknown) {
  if (key === 'status' && typeof value === 'string') return statusLabels[value] || humanize(value);
  if (key === 'role' && typeof value === 'string') return roleLabels[value] || humanize(value);
  if (key === 'visibility' && typeof value === 'string') return visibilityLabels[value] || humanize(value);
  if (key === 'target_type' && typeof value === 'string') return targetLabels[value] || humanize(value);
  if (key === 'size' && typeof value === 'number') return value < 1024 * 1024 ? `${(value / 1024).toFixed(1)} KB` : `${(value / 1024 / 1024).toFixed(1)} MB`;
  if (key === 'secret_updated' && typeof value === 'boolean') return value ? '已更新' : '未修改';
  if (typeof value === 'boolean') return value ? '是' : '否';
  if (Array.isArray(value)) return value.join('、');
  if (value && typeof value === 'object') return '已记录';
  return String(value ?? '无');
}

function metadataSummary(event: AuditEvent) {
  const parts = Object.entries(event.metadata)
    .filter(([key]) => !key.endsWith('_id') && key !== 'id')
    .map(([key, value]) => `${metadataLabels[key] || humanize(key)}：${valueLabel(key, value)}`);
  if (parts.length) return parts.join(' · ');
  return `${targetLabels[event.target_type] || humanize(event.target_type)}操作已记录`;
}

const filteredEvents = computed(() => {
  const needle = query.value.trim().toLocaleLowerCase();
  if (!needle) return events.value;
  return events.value.filter((event) => [
    actionLabels[event.action] || humanize(event.action),
    targetLabels[event.target_type] || humanize(event.target_type),
    metadataSummary(event),
    new Date(event.created_at).toLocaleString(),
  ].some((value) => value.toLocaleLowerCase().includes(needle)));
});

async function load() {
  loading.value = true;
  error.value = '';
  try {
    events.value = await api.auditEvents();
  } catch (err) {
    error.value = err instanceof Error ? err.message : '加载审计日志失败';
  } finally {
    loading.value = false;
  }
}

onMounted(load);
</script>

<template>
  <WorkbenchShell layout="workspace">
    <div class="page-head access-page-head">
      <div>
        <h1>审计日志</h1>
        <p>查看账号、项目、凭据、文件和工单的重要操作记录。</p>
      </div>
    </div>
    <p v-if="error" class="error access-message">{{ error }}</p>
    <section class="audit-surface">
      <header class="audit-toolbar">
        <label class="access-search audit-search"><Search :size="16" /><input v-model="query" type="search" placeholder="搜索操作、对象或内容" /></label>
        <span>{{ filteredEvents.length }} / {{ events.length }}</span>
      </header>
      <div class="audit-scroll">
        <p v-if="loading" class="audit-state">正在加载审计日志...</p>
        <p v-else-if="!filteredEvents.length" class="audit-state">{{ events.length ? '没有匹配的审计事件' : '暂无审计事件' }}</p>
        <article v-for="event in filteredEvents" v-else :key="event.id" class="audit-event-row">
          <span class="audit-event-icon"><History :size="17" /></span>
          <div class="audit-event-copy">
            <strong>{{ actionLabels[event.action] || humanize(event.action) }}</strong>
            <span>{{ metadataSummary(event) }}</span>
          </div>
          <span class="audit-target">{{ targetLabels[event.target_type] || humanize(event.target_type) }}</span>
          <time :datetime="event.created_at">{{ new Date(event.created_at).toLocaleString() }}</time>
        </article>
      </div>
    </section>
  </WorkbenchShell>
</template>
