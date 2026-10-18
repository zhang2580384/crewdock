<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref } from 'vue';
import { onBeforeRouteLeave, useRoute } from 'vue-router';
import {
  Building2,
  Clipboard,
  Download,
  Globe2,
  HardDrive,
  KeyRound,
  Menu,
  Monitor,
  Pin,
  Plus,
  RefreshCw,
  Search,
  Server,
  Trash2,
  Upload,
  UserRound,
  X,
} from '@lucide/vue';
import WorkbenchShell from '../components/WorkbenchShell.vue';
import SiteCategoryPicker from '../components/SiteCategoryPicker.vue';
import { api } from '../api';
import { siteCategories } from '../site-category';
import { siteIconText, siteIconTextClass } from '../site-icon';
import { useSessionStore } from '../stores/session';
import type { Department, Site, SiteResourceType, SiteVisibility, SshAuthMethod } from '../types';

interface DepartmentRow {
  department: Department;
  depth: number;
}

const route = useRoute();
const session = useSessionStore();
const sites = ref<Site[]>([]);
const departments = ref<Department[]>([]);
const manageableDepartmentIds = ref<string[]>([]);
const selectedId = ref('');
const activeScope = ref<SiteVisibility>('public');
const creating = ref(false);
const drawerOpen = ref(false);
const query = ref('');
const loading = ref(false);
const saving = ref(false);
const deleting = ref(false);
const refreshingIcon = ref(false);
const error = ref('');
const success = ref('');
const baseline = ref('');
const importOpen = ref(false);
const importing = ref(false);
const importResult = ref('');
const importFileInput = ref<HTMLInputElement | null>(null);
const failedPreviewSource = ref('');
const importForm = reactive({ visibility: 'personal' as SiteVisibility, category: '常用', all_organization: false, department_ids: [] as string[] });

const form = reactive({
  visibility: 'personal' as SiteVisibility,
  resource_type: 'website' as SiteResourceType,
  category: '常用',
  title: '',
  url: '',
  internal_url: '',
  description: '',
  icon_url: '',
  icon_text: '',
  tags: '',
  is_pinned: false,
  host: '',
  port: 0,
  username: '',
  ssh_auth_method: 'password' as SshAuthMethod,
  identity_file: '~/.ssh/id_rsa',
  redirect_clipboard: true,
  redirect_drives: false,
  all_organization: false,
  department_ids: [] as string[],
});

const selectedSite = computed(() => sites.value.find((site) => site.id === selectedId.value) || null);
const categoryOptions = computed(() => siteCategories(sites.value, form.visibility));
const importCategoryOptions = computed(() => siteCategories(sites.value, importForm.visibility));
const canCreateTeam = computed(() => (
  session.user?.system_role === 'system_admin' || Boolean(session.user?.can_manage_departments)
));
const isSystemAdmin = computed(() => session.user?.system_role === 'system_admin');
const departmentNames = computed(() => Object.fromEntries(departments.value.map((department) => [department.id, department.name])));
const allowedDepartmentIds = computed(() => {
  if (isSystemAdmin.value) return new Set(departments.value.map((department) => department.id));
  if (manageableDepartmentIds.value.length) return new Set(manageableDepartmentIds.value);
  return new Set(session.user?.department_id ? [session.user.department_id] : []);
});
const departmentRows = computed<DepartmentRow[]>(() => {
  const children = new Map<string, Department[]>();
  for (const department of departments.value) {
    const key = department.parent_id || '';
    children.set(key, [...(children.get(key) || []), department]);
  }
  for (const rows of children.values()) rows.sort((a, b) => a.sort_order - b.sort_order || a.name.localeCompare(b.name, 'zh-CN'));
  const result: DepartmentRow[] = [];
  const visited = new Set<string>();
  function visit(parentId: string, depth: number) {
    for (const department of children.get(parentId) || []) {
      if (visited.has(department.id)) continue;
      visited.add(department.id);
      result.push({ department, depth });
      visit(department.id, depth + 1);
    }
  }
  visit('', 0);
  for (const department of departments.value) {
    if (!visited.has(department.id)) result.push({ department, depth: 0 });
  }
  return result;
});
const assignableDepartmentRows = computed(() => departmentRows.value.filter((row) => allowedDepartmentIds.value.has(row.department.id)));
const filteredSites = computed(() => {
  const needle = query.value.trim().toLocaleLowerCase();
  return sites.value.filter((site) => {
    if (site.visibility !== activeScope.value) return false;
    if (!needle) return true;
    return [site.title, site.category, site.url, site.internal_url || '', site.description || '', ...site.tags, departmentScopeLabel(site)]
      .some((value) => value.toLocaleLowerCase().includes(needle));
  });
});
const canManageSelected = computed(() => selectedSite.value ? canManageSite(selectedSite.value) : creating.value);
const scopeValid = computed(() => form.visibility === 'personal' || form.all_organization || form.department_ids.length > 0);
const targetValid = computed(() => {
  if (form.resource_type === 'website') return Boolean(form.url.trim());
  if (!form.host.trim() || !form.username.trim() || form.port < 1 || form.port > 65535) return false;
  return form.resource_type !== 'ssh' || form.ssh_auth_method !== 'key' || Boolean(form.identity_file.trim());
});
const formValid = computed(() => Boolean(form.title.trim() && form.category.trim() && targetValid.value && scopeValid.value));
const formScopeLabel = computed(() => {
  if (form.visibility === 'personal') return '个人收藏';
  if (form.all_organization || !form.department_ids.length) return '全组织';
  const names = form.department_ids.map((id) => departmentNames.value[id]).filter(Boolean);
  return names.length <= 2 ? names.join('、') : `${names.slice(0, 2).join('、')}等 ${names.length} 个部门`;
});
const dirty = computed(() => Boolean((creating.value || selectedSite.value) && snapshotForm() !== baseline.value));
const previewImageSource = computed(() => {
  const manual = form.icon_url.trim();
  const cached = !creating.value
    && selectedSite.value?.resource_type === 'website'
    && selectedSite.value.url === form.url.trim()
    ? selectedSite.value.auto_icon_url?.trim() || ''
    : '';
  const source = manual || cached;
  return source && source !== failedPreviewSource.value ? source : '';
});
const previewText = computed(() => siteIconText(form.title, form.url, form.icon_text));

function message(err: unknown, fallback: string) {
  return err instanceof Error ? err.message : fallback;
}

function splitTags(value: string) {
  return value.split(/[,，\s]+/).map((item) => item.trim()).filter(Boolean);
}

function snapshotForm() {
  return JSON.stringify({
    ...form,
    department_ids: [...form.department_ids].sort(),
    title: form.title.trim(),
    url: form.url.trim(),
    internal_url: form.internal_url.trim(),
    description: form.description.trim(),
    icon_url: form.icon_url.trim(),
    icon_text: form.icon_text.trim(),
    tags: form.tags.trim(),
  });
}

function defaultDepartmentIds() {
  const ownDepartment = session.user?.department_id;
  if (ownDepartment && allowedDepartmentIds.value.has(ownDepartment)) return [ownDepartment];
  return assignableDepartmentRows.value[0] ? [assignableDepartmentRows.value[0].department.id] : [];
}

function resetForm(visibility: SiteVisibility) {
  form.visibility = visibility;
  form.resource_type = 'website';
  form.category = '常用';
  form.title = '';
  form.url = '';
  form.internal_url = '';
  form.description = '';
  form.icon_url = '';
  form.icon_text = '';
  form.tags = '';
  form.is_pinned = false;
  form.host = '';
  form.port = 0;
  form.username = '';
  form.ssh_auth_method = 'password';
  form.identity_file = '~/.ssh/id_rsa';
  form.redirect_clipboard = true;
  form.redirect_drives = false;
  form.all_organization = visibility === 'public' && isSystemAdmin.value;
  form.department_ids = visibility === 'public' && !form.all_organization ? defaultDepartmentIds() : [];
  failedPreviewSource.value = '';
  baseline.value = snapshotForm();
}

function fillForm(site: Site) {
  form.visibility = site.visibility;
  form.resource_type = site.resource_type || 'website';
  form.category = site.category;
  form.title = site.title;
  form.url = site.url;
  form.internal_url = site.internal_url || '';
  form.description = site.description || '';
  form.icon_url = site.icon_url || '';
  form.icon_text = site.icon_text || '';
  form.tags = site.tags.join(' ');
  form.is_pinned = site.is_pinned;
  form.host = site.connection_config.host || '';
  form.port = site.connection_config.port || (site.resource_type === 'ssh' ? 22 : site.resource_type === 'rdp' ? 3389 : 0);
  form.username = site.connection_config.username || '';
  form.ssh_auth_method = site.connection_config.auth_method || 'password';
  form.identity_file = site.connection_config.identity_file || '~/.ssh/id_rsa';
  form.redirect_clipboard = site.connection_config.redirect_clipboard ?? true;
  form.redirect_drives = site.connection_config.redirect_drives ?? false;
  form.all_organization = site.visibility === 'public' && site.department_ids.length === 0;
  form.department_ids = [...site.department_ids];
  failedPreviewSource.value = '';
  baseline.value = snapshotForm();
}

function setResourceType(resourceType: SiteResourceType) {
  const previousType = form.resource_type;
  form.resource_type = resourceType;
  if (resourceType === 'ssh' && previousType !== 'ssh') form.port = 22;
  if (resourceType === 'rdp' && previousType !== 'rdp') form.port = 3389;
}

function setFormVisibility(visibility: SiteVisibility) {
  if (visibility === form.visibility || (visibility === 'public' && !canCreateTeam.value)) return;
  form.visibility = visibility;
  if (visibility === 'personal') {
    form.all_organization = false;
    form.department_ids = [];
    return;
  }
  form.all_organization = isSystemAdmin.value;
  form.department_ids = form.all_organization ? [] : defaultDepartmentIds();
}

function resourceLabel(site: Site) {
  if (site.resource_type === 'ssh') return 'Linux SSH';
  if (site.resource_type === 'rdp') return 'Windows RDP';
  return '网站';
}

function connectionConfig() {
  if (form.resource_type === 'ssh') {
    return {
      host: form.host.trim(),
      port: form.port,
      username: form.username.trim(),
      auth_method: form.ssh_auth_method,
      identity_file: form.ssh_auth_method === 'key' ? form.identity_file.trim() : null,
    };
  }
  if (form.resource_type === 'rdp') {
    return {
      host: form.host.trim(),
      port: form.port,
      username: form.username.trim(),
      redirect_clipboard: form.redirect_clipboard,
      redirect_drives: form.redirect_drives,
    };
  }
  return {};
}

function departmentScopeLabel(site: Site) {
  if (site.visibility === 'personal') return '仅自己';
  if (!site.department_ids.length) return '全组织';
  const names = site.department_ids.map((id) => departmentNames.value[id]).filter(Boolean);
  if (!names.length) return `${site.department_ids.length} 个部门`;
  return names.length <= 2 ? names.join('、') : `${names.slice(0, 2).join('、')}等 ${names.length} 个部门`;
}

function canManageSite(site: Site) {
  if (site.visibility === 'personal') return site.owner_id === session.user?.id;
  if (!canCreateTeam.value) return false;
  if (isSystemAdmin.value) return true;
  if (!site.department_ids.length) return false;
  return site.department_ids.every((departmentId) => allowedDepartmentIds.value.has(departmentId));
}

function confirmDiscard() {
  return !dirty.value || window.confirm('当前站点有未保存修改，确认放弃这些修改？');
}

function switchScope(scope: SiteVisibility) {
  if (scope === activeScope.value || !confirmDiscard()) return;
  activeScope.value = scope;
  selectedId.value = '';
  creating.value = false;
  drawerOpen.value = false;
  error.value = '';
  success.value = '';
  resetForm(scope);
}

function selectSite(site: Site) {
  if (saving.value || site.id === selectedId.value || !confirmDiscard()) return;
  selectedId.value = site.id;
  creating.value = false;
  fillForm(site);
  drawerOpen.value = false;
  error.value = '';
  success.value = '';
}

function startCreate() {
  if (!confirmDiscard()) return;
  selectedId.value = '';
  creating.value = true;
  resetForm(activeScope.value === 'public' && !canCreateTeam.value ? 'personal' : activeScope.value);
  drawerOpen.value = false;
  error.value = '';
  success.value = '';
}

function cancelEdit() {
  if (!confirmDiscard()) return;
  if (creating.value) {
    creating.value = false;
    resetForm(activeScope.value);
  } else if (selectedSite.value) {
    fillForm(selectedSite.value);
  }
}

async function load() {
  loading.value = true;
  error.value = '';
  try {
    await session.loadMe();
    const [nextSites, nextDepartments] = await Promise.all([api.sites(), api.departments()]);
    sites.value = nextSites;
    departments.value = nextDepartments;
    if (session.user?.can_manage_accounts && !isSystemAdmin.value) {
      const accounts = await api.users();
      manageableDepartmentIds.value = accounts.capabilities.manageable_department_ids;
    } else {
      manageableDepartmentIds.value = [];
    }
    const refreshed = nextSites.find((site) => site.id === selectedId.value);
    if (refreshed) fillForm(refreshed);
    else if (selectedId.value) selectedId.value = '';
  } catch (err) {
    error.value = message(err, '加载站点失败');
  } finally {
    loading.value = false;
  }
}

async function submit() {
  if (!formValid.value || !canManageSelected.value) return;
  saving.value = true;
  error.value = '';
  success.value = '';
  try {
    const payload = {
      visibility: form.visibility,
      category: form.category.trim(),
      title: form.title.trim(),
      resource_type: form.resource_type,
      connection_config: connectionConfig(),
      url: form.resource_type === 'website' ? form.url.trim() : '',
      internal_url: form.resource_type === 'website' ? form.internal_url.trim() : '',
      description: form.description.trim(),
      icon_url: form.icon_url.trim(),
      icon_text: form.icon_text.trim(),
      tags: splitTags(form.tags),
      is_pinned: form.is_pinned,
      department_ids: form.visibility === 'public' && !form.all_organization ? [...form.department_ids] : [],
    };
    let saved = creating.value ? await api.createSite(payload) : await api.updateSite(selectedId.value, payload);
    if (saved.resource_type === 'website' && !saved.icon_url) {
      try {
        saved = await api.refreshSiteIcon(saved.id);
      } catch {
        // Saving remains successful when the target site has no reachable icon.
      }
    }
    selectedId.value = saved.id;
    activeScope.value = saved.visibility;
    creating.value = false;
    await load();
    success.value = `已保存“${saved.title}”`;
  } catch (err) {
    error.value = message(err, '保存站点失败');
  } finally {
    saving.value = false;
  }
}

async function refreshIcon() {
  const site = selectedSite.value;
  if (!site || site.resource_type !== 'website' || dirty.value) return;
  refreshingIcon.value = true;
  error.value = '';
  success.value = '';
  try {
    const refreshed = await api.refreshSiteIcon(site.id);
    const index = sites.value.findIndex((row) => row.id === refreshed.id);
    if (index >= 0) sites.value[index] = refreshed;
    fillForm(refreshed);
    success.value = refreshed.auto_icon_url ? '已更新站点图标' : '未获取到图片，已使用文字图标';
  } catch (err) {
    error.value = message(err, '获取站点图标失败');
  } finally {
    refreshingIcon.value = false;
  }
}

async function removeSite() {
  const site = selectedSite.value;
  if (!site || !canManageSelected.value || !window.confirm(`确认删除站点“${site.title}”？此操作无法撤销。`)) return;
  deleting.value = true;
  error.value = '';
  try {
    await api.deleteSite(site.id);
    selectedId.value = '';
    resetForm(activeScope.value);
    await load();
  } catch (err) {
    error.value = message(err, '删除站点失败');
  } finally {
    deleting.value = false;
  }
}

function openImport() {
  importForm.visibility = activeScope.value;
  importForm.category = '常用';
  importForm.all_organization = activeScope.value === 'public' && isSystemAdmin.value;
  importForm.department_ids = activeScope.value === 'public' && !importForm.all_organization ? defaultDepartmentIds() : [];
  importResult.value = '';
  importOpen.value = true;
}

function closeImport() {
  if (importing.value) return;
  importOpen.value = false;
  importResult.value = '';
  if (importFileInput.value) importFileInput.value.value = '';
}

async function importFile(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) return;
  if (importForm.visibility === 'public' && !importForm.all_organization && !importForm.department_ids.length) {
    error.value = '团队站点至少需要选择一个部门';
    input.value = '';
    return;
  }
  importing.value = true;
  error.value = '';
  importResult.value = '';
  try {
    const result = await api.importBookmarks({
      html: await file.text(),
      visibility: importForm.visibility,
      category: importForm.category.trim() || undefined,
      department_ids: importForm.visibility === 'public' && !importForm.all_organization ? [...importForm.department_ids] : [],
    });
    importResult.value = `已导入 ${result.imported} 个，跳过重复 ${result.skipped} 个`;
    activeScope.value = importForm.visibility;
    await load();
  } catch (err) {
    error.value = message(err, '导入失败');
  } finally {
    importing.value = false;
    input.value = '';
  }
}

function beforeUnload(event: BeforeUnloadEvent) {
  if (!dirty.value) return;
  event.preventDefault();
  event.returnValue = '';
}

onBeforeRouteLeave(() => confirmDiscard());
onMounted(async () => {
  window.addEventListener('beforeunload', beforeUnload);
  await load();
  const editId = typeof route.query.edit === 'string' ? route.query.edit : '';
  const requestedSite = sites.value.find((site) => site.id === editId);
  if (requestedSite) {
    activeScope.value = requestedSite.visibility;
    selectSite(requestedSite);
  } else {
    resetForm(activeScope.value);
    const title = typeof route.query.title === 'string' ? route.query.title : '';
    const url = typeof route.query.url === 'string' ? route.query.url : '';
    if (title || url) {
      startCreate();
      form.title = title;
      form.url = url;
    }
  }
});
onBeforeUnmount(() => window.removeEventListener('beforeunload', beforeUnload));
</script>

<template>
  <WorkbenchShell layout="workspace">
    <div class="page-head access-page-head site-management-head">
      <div>
        <h1>站点管理</h1>
        <p>维护个人入口，或按部门发布团队站点。</p>
      </div>
      <div class="page-actions site-management-actions">
        <div class="segmented site-scope-switch" aria-label="站点范围">
          <button :class="{ active: activeScope === 'public' }" type="button" @click="switchScope('public')"><Building2 :size="15" />团队</button>
          <button :class="{ active: activeScope === 'personal' }" type="button" @click="switchScope('personal')"><UserRound :size="15" />个人</button>
        </div>
        <button class="secondary-button" type="button" :disabled="activeScope === 'public' && !canCreateTeam" @click="openImport"><Upload :size="17" /><span>导入</span></button>
        <button class="primary-button" type="button" @click="startCreate"><Plus :size="17" /><span>添加入口</span></button>
      </div>
    </div>
    <p v-if="error" class="error access-message">{{ error }}</p>
    <p v-else-if="success" class="success access-message">{{ success }}</p>

    <section class="access-workspace resource-workspace site-management-workspace" :class="{ 'rail-open': drawerOpen }">
      <button class="access-drawer-backdrop" type="button" aria-label="关闭站点列表" @click="drawerOpen = false" />
      <aside id="site-list-panel" class="access-rail resource-rail">
        <div class="resource-rail-tools">
          <label class="access-search"><Search :size="16" /><input v-model="query" type="search" placeholder="搜索站点" /></label>
          <button class="icon-button access-rail-close" type="button" title="关闭站点列表" @click="drawerOpen = false"><X :size="17" /></button>
        </div>
        <div class="resource-list-scroll">
          <p v-if="loading" class="resource-list-state">正在加载...</p>
          <p v-else-if="!filteredSites.length" class="resource-list-state">{{ activeScope === 'public' ? '暂无团队站点' : '暂无个人站点' }}</p>
          <button v-for="site in filteredSites" v-else :key="site.id" class="resource-list-item" :class="{ active: site.id === selectedId }" type="button" @click="selectSite(site)">
            <span class="resource-list-icon">
              <Server v-if="site.resource_type === 'ssh'" :size="17" />
              <Monitor v-else-if="site.resource_type === 'rdp'" :size="17" />
              <Globe2 v-else :size="17" />
            </span>
            <span class="resource-list-copy"><strong>{{ site.title }}</strong><small>{{ resourceLabel(site) }} · {{ site.category }} · {{ departmentScopeLabel(site) }}</small></span>
            <span v-if="site.is_pinned" class="resource-status-badge"><Pin :size="11" />置顶</span>
          </button>
        </div>
        <div class="access-rail-count">{{ filteredSites.length }} / {{ sites.filter((site) => site.visibility === activeScope).length }}</div>
      </aside>

      <main v-if="creating || selectedSite" class="resource-detail-surface">
        <header class="access-detail-head">
          <button class="icon-button access-rail-toggle" type="button" title="打开站点列表" aria-controls="site-list-panel" :aria-expanded="drawerOpen" @click="drawerOpen = true"><Menu :size="18" /></button>
          <div class="access-detail-title">
            <strong>{{ creating ? '添加入口' : selectedSite?.title }}</strong>
            <span>{{ formScopeLabel }}</span>
          </div>
          <button v-if="selectedSite && canManageSelected" class="icon-button danger-button" type="button" title="删除站点" :disabled="deleting" @click="removeSite"><Trash2 :size="17" /></button>
        </header>

        <form v-if="creating || canManageSelected" class="resource-detail-form" @submit.prevent="submit">
          <div class="resource-detail-scroll">
            <section class="access-detail-section resource-form-section">
              <div class="site-form-switches">
                <div class="site-form-switch">
                  <span>入口类型</span>
                  <div class="site-type-switch segmented" aria-label="入口类型">
                    <button type="button" :class="{ active: form.resource_type === 'website' }" @click="setResourceType('website')"><Globe2 :size="16" />网站</button>
                    <button type="button" :class="{ active: form.resource_type === 'ssh' }" @click="setResourceType('ssh')"><Server :size="16" /><span class="type-label-wide">Linux SSH</span><span class="type-label-compact">SSH</span></button>
                    <button type="button" :class="{ active: form.resource_type === 'rdp' }" @click="setResourceType('rdp')"><Monitor :size="16" /><span class="type-label-wide">Windows RDP</span><span class="type-label-compact">RDP</span></button>
                  </div>
                </div>
                <div class="site-form-switch">
                  <span>可见范围</span>
                  <div class="segmented site-form-visibility" aria-label="可见范围">
                    <button type="button" :class="{ active: form.visibility === 'public' }" :disabled="!canCreateTeam" @click="setFormVisibility('public')"><Building2 :size="15" />团队</button>
                    <button type="button" :class="{ active: form.visibility === 'personal' }" @click="setFormVisibility('personal')"><UserRound :size="15" />个人</button>
                  </div>
                </div>
              </div>
              <div class="access-fields-grid">
                <label class="field"><span>名称</span><input v-model="form.title" required /></label>
                <SiteCategoryPicker :key="`${selectedId}-${creating}-${form.visibility}`" v-model="form.category" :categories="categoryOptions" required />
                <template v-if="form.resource_type === 'website'">
                  <label class="field access-description-field"><span>外网地址</span><input v-model="form.url" inputmode="url" required /></label>
                  <label class="field access-description-field"><span>内网地址</span><input v-model="form.internal_url" inputmode="url" placeholder="192.168.1.10:8080" /></label>
                </template>
                <template v-else>
                  <label class="field access-description-field"><span>服务器地址</span><input v-model="form.host" autocomplete="off" placeholder="192.168.1.10" required /></label>
                  <label class="field"><span>端口</span><input v-model.number="form.port" type="number" min="1" max="65535" required /></label>
                  <label class="field access-description-field"><span>登录用户名</span><input v-model="form.username" autocomplete="off" required /></label>
                </template>
                <template v-if="form.resource_type === 'ssh'">
                  <div class="field access-description-field">
                    <span>认证方式</span>
                    <div class="segmented connection-auth-switch" aria-label="SSH 认证方式">
                      <button type="button" :class="{ active: form.ssh_auth_method === 'password' }" @click="form.ssh_auth_method = 'password'">密码（终端输入）</button>
                      <button type="button" :class="{ active: form.ssh_auth_method === 'key' }" @click="form.ssh_auth_method = 'key'"><KeyRound :size="15" />本机密钥</button>
                    </div>
                  </div>
                  <label v-if="form.ssh_auth_method === 'key'" class="field access-description-field"><span>本机密钥路径</span><input v-model="form.identity_file" autocomplete="off" placeholder="~/.ssh/id_rsa" required /></label>
                </template>
                <label class="field access-description-field"><span>说明</span><input v-model="form.description" /></label>
                <template v-if="form.resource_type === 'website'">
                  <label class="field"><span>自定义图标 URL</span><input v-model="form.icon_url" inputmode="url" /></label>
                  <label class="field"><span>图标文字</span><input v-model="form.icon_text" maxlength="12" placeholder="996post / NAS" /></label>
                  <div class="site-icon-preview-row access-description-field">
                    <div class="site-icon site-icon-preview">
                      <img v-if="previewImageSource" :src="previewImageSource" alt="" @error="failedPreviewSource = previewImageSource" />
                      <span v-else class="site-wordmark" :class="siteIconTextClass(previewText)">{{ previewText }}</span>
                    </div>
                    <div class="site-icon-preview-copy">
                      <strong>{{ form.title.trim() || '站点图标' }}</strong>
                      <span>{{ previewImageSource ? '图片图标' : '文字图标' }}</span>
                    </div>
                    <button v-if="selectedSite" class="secondary-button" type="button" :disabled="refreshingIcon || dirty" title="重新从站点获取图标" @click="refreshIcon">
                      <RefreshCw :class="{ spin: refreshingIcon }" :size="16" />
                      <span>{{ refreshingIcon ? '获取中' : '重新获取' }}</span>
                    </button>
                  </div>
                </template>
                <label class="field"><span>标签</span><input v-model="form.tags" placeholder="逗号或空格分隔" /></label>
              </div>
              <div v-if="form.resource_type === 'rdp'" class="connection-options">
                <label class="check-row"><input v-model="form.redirect_clipboard" type="checkbox" /><Clipboard :size="16" /><span>启用剪贴板</span></label>
                <label class="check-row"><input v-model="form.redirect_drives" type="checkbox" /><HardDrive :size="16" /><span>映射本地磁盘</span></label>
              </div>
              <label class="check-row site-pin-check"><input v-model="form.is_pinned" type="checkbox" /><span>置顶显示</span></label>

              <section v-if="form.visibility === 'public'" class="site-department-section">
                <div class="access-section-head"><div><h2>可见部门</h2><span>选择上级部门时包含其下级部门</span></div><strong>{{ form.all_organization ? '全组织' : `${form.department_ids.length} 个部门` }}</strong></div>
                <label v-if="isSystemAdmin" class="site-department-option all"><input v-model="form.all_organization" type="checkbox" @change="form.department_ids = []" /><span><strong>全组织可见</strong><small>所有部门成员都能看到</small></span></label>
                <div v-if="!form.all_organization" class="site-department-list">
                  <label v-for="row in assignableDepartmentRows" :key="row.department.id" class="site-department-option" :style="{ '--department-depth': row.depth }">
                    <input v-model="form.department_ids" type="checkbox" :value="row.department.id" />
                    <span>{{ row.department.name }}</span>
                  </label>
                  <p v-if="!assignableDepartmentRows.length" class="muted-text">当前账号没有可分配的部门</p>
                </div>
              </section>
            </section>
          </div>
          <footer class="access-detail-actions">
            <span v-if="dirty" class="access-dirty-state">有未保存修改</span>
            <button v-if="dirty" class="secondary-button" type="button" @click="cancelEdit">取消</button>
            <button class="primary-button" :disabled="saving || !formValid"><span>{{ saving ? '保存中...' : creating ? '创建入口' : '保存' }}</span></button>
          </footer>
        </form>

        <div v-else-if="selectedSite" class="resource-detail-scroll">
          <section class="access-detail-section">
            <div class="access-section-head"><h2>入口信息</h2></div>
            <div class="access-fields-grid">
              <div class="access-readonly-field"><span>类型</span><strong>{{ resourceLabel(selectedSite) }}</strong></div>
              <div class="access-readonly-field"><span>分组</span><strong>{{ selectedSite.category }}</strong></div>
              <div class="access-readonly-field"><span>可见范围</span><strong>{{ departmentScopeLabel(selectedSite) }}</strong></div>
              <template v-if="selectedSite.resource_type === 'website'">
                <div class="access-readonly-field access-description-field"><span>外网地址</span><strong>{{ selectedSite.url }}</strong></div>
                <div class="access-readonly-field access-description-field"><span>内网地址</span><strong>{{ selectedSite.internal_url || '未填写' }}</strong></div>
              </template>
              <template v-else>
                <div class="access-readonly-field access-description-field"><span>服务器地址</span><strong>{{ selectedSite.connection_config.host }}:{{ selectedSite.connection_config.port }}</strong></div>
                <div class="access-readonly-field"><span>登录用户名</span><strong>{{ selectedSite.connection_config.username }}</strong></div>
              </template>
              <div class="access-readonly-field access-description-field"><span>说明</span><strong>{{ selectedSite.description || '未填写' }}</strong></div>
            </div>
          </section>
        </div>
      </main>

      <div v-else class="access-detail-empty">
        <button class="icon-button access-rail-toggle" type="button" title="打开站点列表" aria-controls="site-list-panel" :aria-expanded="drawerOpen" @click="drawerOpen = true"><Menu :size="18" /></button>
        <div class="workspace-selection-empty"><Globe2 :size="24" /><strong>选择一个站点</strong><span>查看或修改站点配置</span></div>
      </div>
    </section>

    <div v-if="importOpen" class="modal-backdrop" @click.self="closeImport">
      <section class="dialog-panel access-dialog site-import-dialog" role="dialog" aria-modal="true" aria-labelledby="site-import-title">
        <div class="panel-head"><div><h2 id="site-import-title">导入浏览器书签</h2><p class="access-dialog-target">导入到{{ importForm.visibility === 'public' ? '团队站点' : '个人站点' }}</p></div><button class="icon-button" type="button" title="关闭" @click="closeImport"><X :size="17" /></button></div>
        <SiteCategoryPicker v-model="importForm.category" :categories="importCategoryOptions" />
        <section v-if="importForm.visibility === 'public'" class="site-department-section compact">
          <div class="access-section-head"><h2>可见部门</h2></div>
          <label v-if="isSystemAdmin" class="site-department-option all"><input v-model="importForm.all_organization" type="checkbox" @change="importForm.department_ids = []" /><span>全组织可见</span></label>
          <div v-if="!importForm.all_organization" class="site-department-list">
            <label v-for="row in assignableDepartmentRows" :key="row.department.id" class="site-department-option" :style="{ '--department-depth': row.depth }"><input v-model="importForm.department_ids" type="checkbox" :value="row.department.id" /><span>{{ row.department.name }}</span></label>
          </div>
        </section>
        <label class="upload-drop site-import-drop"><Download :size="20" /><span>{{ importing ? '正在导入...' : '选择 Chrome / Edge 书签 HTML 文件' }}</span><input ref="importFileInput" type="file" accept=".html,.htm,text/html" :disabled="importing" @change="importFile" /></label>
        <p v-if="importResult" class="success">{{ importResult }}</p>
      </section>
    </div>
  </WorkbenchShell>
</template>
