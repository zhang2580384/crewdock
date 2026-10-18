<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { useRouter } from 'vue-router';
import {
  AlertCircle,
  ArrowRight,
  BookmarkPlus,
  Briefcase,
  CalendarPlus,
  Clock,
  EllipsisVertical,
  Flame,
  Globe2,
  LoaderCircle,
  Monitor,
  Network,
  Pin,
  Plus,
  Search,
  Server,
  Sparkles,
  Users,
} from '@lucide/vue';
import WorkbenchShell from '../components/WorkbenchShell.vue';
import SiteContextMenu from '../components/SiteContextMenu.vue';
import { api } from '../api';
import { siteIconSources, siteIconText, siteIconTextClass } from '../site-icon';
import { siteLaunchHref } from '../site-launch';
import { useSessionStore } from '../stores/session';
import type { Department, Site } from '../types';

type SortMode = 'smart' | 'popular' | 'recent' | 'new';
type SiteStatus = 'popular' | 'recent' | 'new';

const session = useSessionStore();
const router = useRouter();
const sites = ref<Site[]>([]);
const departments = ref<Department[]>([]);
const manageableDepartmentIds = ref<string[]>([]);
const contextMenu = ref<{ site: Site; x: number; y: number } | null>(null);
const query = ref('');
const visibility = ref<'all' | 'public' | 'personal'>('all');
const sortMode = ref<SortMode>('smart');
const loading = ref(true);
const error = ref('');
const failedIconKeys = ref<Record<string, boolean>>({});
const sortOptions = [
  { value: 'smart' as const, label: '智能', icon: Sparkles },
  { value: 'popular' as const, label: '热门', icon: Flame },
  { value: 'recent' as const, label: '最近', icon: Clock },
  { value: 'new' as const, label: '新增', icon: CalendarPlus },
];
const DAY_MS = 24 * 60 * 60 * 1000;

const siteTones = [
  { accent: '#16784a', soft: '#e8f4ed', strong: '#105f3a' },
  { accent: '#2863bd', soft: '#eaf1fb', strong: '#204f97' },
  { accent: '#a96b11', soft: '#fbf1df', strong: '#86530c' },
  { accent: '#b64d43', soft: '#f9ecea', strong: '#8f3932' },
  { accent: '#287a75', soft: '#e6f3f1', strong: '#1f605c' },
  { accent: '#7d5d48', soft: '#f3eeea', strong: '#614736' },
];

const departmentNames = computed(() => Object.fromEntries(departments.value.map((department) => [department.id, department.name])));
const allowedDepartmentIds = computed(() => {
  if (session.user?.system_role === 'system_admin') return new Set(departments.value.map((department) => department.id));
  if (manageableDepartmentIds.value.length) return new Set(manageableDepartmentIds.value);
  return new Set(session.user?.department_id ? [session.user.department_id] : []);
});

const bookmarklet = computed(() => {
  const origin = window.location.origin;
  return `javascript:(()=>{const u=encodeURIComponent(location.href);const t=encodeURIComponent(document.title);window.open('${origin}/sites?url='+u+'&title='+t,'_blank');})();`;
});

const matchingSites = computed(() => {
  const keyword = query.value.trim().toLowerCase();
  return sites.value
    .filter((site) => {
      if (visibility.value !== 'all' && site.visibility !== visibility.value) return false;
      if (!keyword) return true;
      return [
        site.title,
        site.url,
        site.description || '',
        site.category,
        site.connection_config.host || '',
        site.connection_config.username || '',
        ...site.tags,
        siteScopeLabel(site),
      ]
        .join(' ')
        .toLowerCase()
        .includes(keyword);
    });
});

const popularSiteIds = computed(() => new Set(
  [...matchingSites.value]
    .filter((site) => site.open_count >= 3)
    .sort((left, right) => right.open_count - left.open_count || dateValue(right.last_opened_at) - dateValue(left.last_opened_at))
    .slice(0, 3)
    .map((site) => site.id),
));

const filteredSites = computed(() => [...matchingSites.value].sort((left, right) => compareSites(left, right, sortMode.value)));

const siteStatusById = computed<Record<string, SiteStatus>>(() => {
  const statuses: Record<string, SiteStatus> = {};
  const now = Date.now();
  for (const site of matchingSites.value) {
    if (popularSiteIds.value.has(site.id)) statuses[site.id] = 'popular';
    else if (site.last_opened_at && now - dateValue(site.last_opened_at) <= 3 * DAY_MS) statuses[site.id] = 'recent';
    else if (now - dateValue(site.created_at) <= 14 * DAY_MS) statuses[site.id] = 'new';
  }
  return statuses;
});

const groupedSites = computed(() => {
  const groups = new Map<string, Site[]>();
  for (const site of filteredSites.value) {
    const category = site.category || '未分组';
    groups.set(category, [...(groups.get(category) || []), site]);
  }
  return Array.from(groups.entries()).map(([category, rows]) => ({ category, rows }));
});

const emptyActions = computed(() => {
  const actions = [
    { to: '/sites', title: '添加常用站点', detail: '个人收藏或团队公共入口', icon: Plus },
    { to: '/sites', title: '导入浏览器书签', detail: '批量迁移 Chrome / Edge 书签', icon: BookmarkPlus },
    { to: '/projects', title: '创建项目空间', detail: '集中项目入口、文档和工单', icon: Briefcase },
  ];
  if (session.user?.system_role === 'system_admin') {
    actions.push({ to: '/users', title: '维护成员账号', detail: '添加员工并分配工作角色', icon: Users });
  }
  return actions;
});

function dateValue(value?: string | null) {
  const parsed = value ? Date.parse(value) : 0;
  return Number.isFinite(parsed) ? parsed : 0;
}

function smartScore(site: Site) {
  const now = Date.now();
  const openedAt = dateValue(site.last_opened_at);
  const createdAt = dateValue(site.created_at);
  const recentBoost = openedAt
    ? now - openedAt <= DAY_MS ? 28 : now - openedAt <= 7 * DAY_MS ? 14 : now - openedAt <= 30 * DAY_MS ? 5 : 0
    : 0;
  const newBoost = createdAt && now - createdAt <= 14 * DAY_MS ? 4 : 0;
  return Math.min(site.open_count, 50) * 4 + recentBoost + newBoost;
}

function compareSites(left: Site, right: Site, mode: SortMode) {
  if (mode === 'popular') {
    return right.open_count - left.open_count
      || dateValue(right.last_opened_at) - dateValue(left.last_opened_at)
      || left.title.localeCompare(right.title, 'zh-CN');
  }
  if (mode === 'recent') {
    return dateValue(right.last_opened_at) - dateValue(left.last_opened_at)
      || right.open_count - left.open_count
      || left.title.localeCompare(right.title, 'zh-CN');
  }
  if (mode === 'new') {
    return dateValue(right.created_at) - dateValue(left.created_at)
      || left.title.localeCompare(right.title, 'zh-CN');
  }
  if (left.is_pinned !== right.is_pinned) return left.is_pinned ? -1 : 1;
  return smartScore(right) - smartScore(left)
    || left.sort_order - right.sort_order
    || left.title.localeCompare(right.title, 'zh-CN');
}

function siteStatusLabel(status?: SiteStatus) {
  if (status === 'popular') return '热门';
  if (status === 'recent') return '最近';
  return status === 'new' ? '新增' : '';
}

function trackSiteOpen(site: Site) {
  void api.recordSiteOpen(site.id).catch(() => undefined);
}

async function load() {
  loading.value = true;
  error.value = '';
  try {
    await session.loadMe();
    const [nextSites, nextDepartments] = await Promise.all([api.sites(), api.departments()]);
    sites.value = nextSites;
    departments.value = nextDepartments;
    void refreshMissingSiteIcons();
    if (session.user?.can_manage_accounts && session.user.system_role !== 'system_admin') {
      const accounts = await api.users();
      manageableDepartmentIds.value = accounts.capabilities.manageable_department_ids;
    }
  } catch (err) {
    error.value = err instanceof Error ? err.message : '加载工作台失败';
  } finally {
    loading.value = false;
  }
}

function canEditSite(site: Site) {
  if (site.visibility === 'personal') return site.owner_id === session.user?.id;
  if (session.user?.system_role === 'system_admin') return true;
  if (!session.user?.can_manage_departments) return false;
  if (!site.department_ids.length) return false;
  return site.department_ids.every((departmentId) => allowedDepartmentIds.value.has(departmentId));
}

function siteDisplayUrl(url: string) {
  return url.trim().replace(/^https?:\/\//i, '').replace(/\/$/, '');
}

function siteSummary(site: Site) {
  if (site.description) return site.description;
  if (site.resource_type === 'website') return siteDisplayUrl(site.url);
  const config = site.connection_config;
  return `${config.username || ''}@${config.host || ''}:${config.port || (site.resource_type === 'ssh' ? 22 : 3389)}`;
}

function siteImageKey(site: Site, source: string) {
  return `${site.id}:${source === site.icon_url?.trim() ? 'manual' : 'auto'}`;
}

function siteImageSource(site: Site) {
  return siteIconSources(site).find((source) => !failedIconKeys.value[siteImageKey(site, source)]) || '';
}

function markSiteImageFailed(site: Site) {
  const source = siteImageSource(site);
  if (!source) return;
  failedIconKeys.value = { ...failedIconKeys.value, [siteImageKey(site, source)]: true };
}

async function refreshMissingSiteIcons() {
  const pending = sites.value.filter((site) => (
    site.resource_type === 'website'
    && !site.icon_url
    && !site.auto_icon_url
    && !site.icon_fetched_at
  ));
  let cursor = 0;
  const worker = async () => {
    while (cursor < pending.length) {
      const site = pending[cursor++];
      try {
        const refreshed = await api.refreshSiteIcon(site.id);
        const index = sites.value.findIndex((row) => row.id === site.id);
        if (index >= 0) {
          sites.value[index] = {
            ...sites.value[index],
            auto_icon_url: refreshed.auto_icon_url,
            icon_fetched_at: refreshed.icon_fetched_at,
          };
        }
      } catch {
        // Icon retrieval is best-effort and must not interrupt the dashboard.
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(3, pending.length) }, worker));
}

function siteTone(site: Site) {
  if (site.resource_type === 'ssh') {
    return { '--site-accent': '#16784a', '--site-icon-bg': '#e8f4ed', '--site-icon-color': '#105f3a' };
  }
  if (site.resource_type === 'rdp') {
    return { '--site-accent': '#2863bd', '--site-icon-bg': '#eaf1fb', '--site-icon-color': '#204f97' };
  }
  const hash = Array.from(site.title).reduce((value, character) => (value * 31 + (character.codePointAt(0) || 0)) >>> 0, 0);
  const tone = siteTones[hash % siteTones.length];
  return {
    '--site-accent': tone.accent,
    '--site-icon-bg': tone.soft,
    '--site-icon-color': tone.strong,
  };
}

function siteScopeLabel(site: Site) {
  if (site.visibility === 'personal') return '个人';
  if (!site.department_ids.length) return '全组织';
  const names = site.department_ids.map((id) => departmentNames.value[id]).filter(Boolean);
  if (!names.length) return `${site.department_ids.length} 个部门`;
  return names.length === 1 ? names[0] : `${names[0]}等 ${names.length} 个部门`;
}

function openContextMenu(site: Site, event: MouseEvent) {
  contextMenu.value = { site, x: event.clientX, y: event.clientY };
}

function editSite(site: Site) {
  router.push({ path: '/sites', query: { edit: site.id } });
}

onMounted(load);
</script>

<template>
  <WorkbenchShell>
    <section class="dashboard-control-card">
      <header class="dashboard-commandbar">
        <div class="dashboard-title">
          <h1>常用入口</h1>
          <span class="dashboard-count" :aria-label="`${filteredSites.length} 个入口`">{{ filteredSites.length }}</span>
        </div>
        <div class="dashboard-actions">
          <RouterLink class="secondary-button as-link dashboard-project-action" to="/projects" title="项目空间" aria-label="项目空间">
            <Briefcase :size="17" />
            <span class="dashboard-project-label">项目空间</span>
          </RouterLink>
          <RouterLink class="primary-button as-link" to="/sites">
            <Plus :size="17" />
            <span>添加站点</span>
          </RouterLink>
        </div>
      </header>

      <div class="dashboard-toolbar">
        <label class="search-box dashboard-search">
          <Search :size="16" />
          <input v-model="query" placeholder="搜索站点" />
        </label>
        <div class="segmented" aria-label="可见范围">
          <button :class="{ active: visibility === 'all' }" @click="visibility = 'all'">全部</button>
          <button :class="{ active: visibility === 'public' }" @click="visibility = 'public'">团队</button>
          <button :class="{ active: visibility === 'personal' }" @click="visibility = 'personal'">个人</button>
        </div>
        <div class="dashboard-sort" aria-label="入口排序">
          <button v-for="option in sortOptions" :key="option.value" type="button" :class="{ active: sortMode === option.value }" @click="sortMode = option.value">
            <component :is="option.icon" :size="14" />
            <span>{{ option.label }}</span>
          </button>
        </div>
      </div>
    </section>

    <section class="dashboard-section">
      <div v-if="loading" class="loading-state">
        <LoaderCircle class="spin" :size="22" />
        <span>正在整理工作台...</span>
      </div>

      <div v-else-if="error" class="inline-alert error-alert">
        <AlertCircle :size="19" />
        <span>{{ error }}</span>
        <button class="secondary-button" type="button" @click="load">重新加载</button>
      </div>

      <section v-else-if="sites.length === 0" class="empty-workspace">
        <div class="empty-copy">
          <div class="empty-icon"><Globe2 :size="24" /></div>
          <div>
            <h2>从第一个常用入口开始</h2>
            <p>添加后台、文档、监控或其他日常工作地址。</p>
          </div>
        </div>
        <div class="onboarding-grid">
          <RouterLink v-for="action in emptyActions" :key="action.title" :to="action.to" class="onboarding-action">
            <component :is="action.icon" :size="20" />
            <div>
              <strong>{{ action.title }}</strong>
              <span>{{ action.detail }}</span>
            </div>
            <ArrowRight :size="17" />
          </RouterLink>
        </div>
      </section>

      <section v-else-if="filteredSites.length === 0" class="compact-empty">
        <Search :size="22" />
        <strong>没有匹配的站点</strong>
        <span>调整搜索词或可见范围后再试。</span>
      </section>

      <template v-else>
        <section v-for="group in groupedSites" :key="group.category" class="site-group">
          <div class="group-head">
            <h3>{{ group.category }}</h3>
            <span>{{ group.rows.length }}</span>
          </div>
          <div class="site-grid">
            <article
              v-for="site in group.rows"
              :key="site.id"
              class="site-tile"
              :class="`resource-${site.resource_type}`"
              :style="siteTone(site)"
              @contextmenu.prevent="openContextMenu(site, $event)"
            >
              <a
                class="site-hit-area"
                :href="siteLaunchHref(site)"
                :target="site.resource_type === 'website' ? '_blank' : undefined"
                :rel="site.resource_type === 'website' ? 'noreferrer' : undefined"
                :aria-label="`${site.resource_type === 'website' ? '打开' : '连接'} ${site.title}`"
                @click="trackSiteOpen(site)"
              />
              <div class="site-icon">
                <Server v-if="site.resource_type === 'ssh'" :size="22" />
                <Monitor v-else-if="site.resource_type === 'rdp'" :size="22" />
                <img v-else-if="siteImageSource(site)" :src="siteImageSource(site)" alt="" @error="markSiteImageFailed(site)" />
                <span v-else class="site-wordmark" :class="siteIconTextClass(siteIconText(site.title, site.url, site.icon_text))">{{ siteIconText(site.title, site.url, site.icon_text) }}</span>
              </div>
              <div class="site-main">
                <div class="site-title">
                  <strong :title="site.title">{{ site.title }}</strong>
                  <Pin v-if="site.is_pinned" class="site-pin" :size="13" aria-label="置顶" />
                </div>
                <span class="site-summary" :title="siteSummary(site)">{{ siteSummary(site) }}</span>
              </div>
              <button class="site-more-button" type="button" title="更多操作" :aria-label="`${site.title} 更多操作`" @click.stop="openContextMenu(site, $event)"><EllipsisVertical :size="17" /></button>
              <small class="site-meta">
                <span class="site-scope" :class="site.visibility" :title="siteScopeLabel(site)">{{ siteScopeLabel(site) }}</span>
                <span v-if="siteStatusById[site.id]" class="site-rank-badge" :class="siteStatusById[site.id]" :title="siteStatusLabel(siteStatusById[site.id])">
                  {{ siteStatusLabel(siteStatusById[site.id]) }}
                </span>
                <span v-else-if="site.resource_type === 'website' && site.internal_url" class="site-internal" :title="`内网地址：${site.internal_url}`"><Network :size="11" />内网</span>
                <span v-else-if="site.open_count > 0" class="site-heat" :title="`已打开 ${site.open_count} 次`"><Flame :size="10" />{{ site.open_count }}</span>
              </small>
            </article>
          </div>
        </section>
      </template>
    </section>

    <section v-if="sites.length > 0" class="bookmark-strip">
      <div>
        <BookmarkPlus :size="18" />
        <span>把任意网页快速收藏到工作台</span>
      </div>
      <a class="secondary-button" :href="bookmarklet">收藏到 CrewDock</a>
    </section>
    <SiteContextMenu
      v-if="contextMenu"
      :site="contextMenu.site"
      :x="contextMenu.x"
      :y="contextMenu.y"
      :can-edit="canEditSite(contextMenu.site)"
      @opened="trackSiteOpen"
      @edit="editSite"
      @close="contextMenu = null"
    />
  </WorkbenchShell>
</template>
