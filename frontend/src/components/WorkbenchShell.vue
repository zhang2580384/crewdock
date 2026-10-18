<script setup lang="ts">
import {
  Activity,
  Briefcase,
  Building2,
  ChevronDown,
  ChevronRight,
  CircleUserRound,
  ClipboardList,
  FileArchive,
  FilePenLine,
  FileText,
  KeyRound,
  LayoutDashboard,
  LogOut,
  Menu,
  PanelLeftClose,
  PanelLeftOpen,
  PanelsTopLeft,
  Settings,
  ShieldCheck,
  Share2,
  Stamp,
  UserCog,
  X,
} from '@lucide/vue';
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { useSessionStore } from '../stores/session';
import { useWorkspaceStore } from '../stores/workspace';

const props = withDefaults(defineProps<{ layout?: 'standard' | 'workspace' }>(), {
  layout: 'standard',
});

const route = useRoute();
const router = useRouter();
const session = useSessionStore();
const workspace = useWorkspaceStore();
const sidebarOpen = ref(false);
const desktopCollapsed = ref(localStorage.getItem('crewdock-sidebar-collapsed') === 'true');
const navigationMedia = window.matchMedia('(max-width: 980px)');
const mobileViewport = ref(navigationMedia.matches);
const userMenuOpen = ref(false);
const loggingOut = ref(false);
const userMenuContainer = ref<HTMLElement | null>(null);
function readSavedGroups() {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem('crewdock-nav-groups') || '{}');
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {};
    return Object.fromEntries(
      Object.entries(parsed).filter((entry): entry is [string, boolean] => typeof entry[1] === 'boolean'),
    );
  } catch {
    return {};
  }
}

const savedGroups = readSavedGroups();
const groupOpen = ref<Record<string, boolean>>({
  workbench: savedGroups.workbench ?? true,
  signing: savedGroups.signing ?? true,
  collaboration: savedGroups.collaboration ?? true,
  administration: savedGroups.administration ?? true,
});
const appVersion = __APP_VERSION__;
const sourceUrl = `https://github.com/zhang2580384/crewdock/tree/v${appVersion}`;

const navGroups = computed(() => {
  const groups = [
    {
      id: 'workbench',
      label: '工作台',
      items: [
        { path: '/', label: '总览与导航', icon: LayoutDashboard },
        { path: '/sites', label: '站点管理', icon: PanelsTopLeft },
      ],
    },
    {
      id: 'signing',
      label: '签章助手',
      items: [
        {
          path: '/seals',
          label: session.user?.system_role === 'system_admin' ? '印章管理' : '已授权印章',
          icon: Stamp,
        },
        { path: '/signing', label: '签章工具', icon: FilePenLine },
      ],
    },
    {
      id: 'collaboration',
      label: '协作空间',
      items: [
        { path: '/projects', label: '项目', icon: Briefcase },
        { path: '/docs', label: '文档', icon: FileText },
        { path: '/files', label: '文件', icon: FileArchive },
        { path: '/tickets', label: '工单', icon: ClipboardList },
        { path: '/shares', label: '内部分享', icon: Share2 },
        { path: '/vault', label: '保险库', icon: KeyRound },
      ],
    },
  ];

  const administration = [];
  if (session.user?.can_manage_accounts) {
    administration.push({ path: '/users', label: '账号管理', icon: UserCog });
  }
  if (session.user?.can_manage_departments) {
    administration.push({ path: '/organization', label: '组织管理', icon: Building2 });
  }
  if (session.user?.can_view_audit) {
    administration.push({ path: '/audit', label: '审计日志', icon: Activity });
  }
  if (session.user?.system_role === 'system_admin') {
    administration.push({ path: '/settings', label: '系统设置', icon: Settings });
  }
  if (administration.length) {
    groups.push({ id: 'administration', label: '管理', items: administration });
  }
  return groups;
});

const activeItem = computed(() => {
  for (const group of navGroups.value) {
    const item = group.items.find((candidate) => candidate.path === route.path);
    if (item) return item;
  }
  if (route.path === '/profile') return { label: '个人账号' };
  return { label: '工作台' };
});

const projectContextVisible = computed(() =>
  ['/docs', '/files', '/tickets', '/shares', '/vault'].includes(route.path),
);
const projectContextAllowsAll = computed(() =>
  ['/tickets', '/shares', '/vault'].includes(route.path),
);

const initials = computed(() => {
  const source = session.user?.display_name || session.user?.email || 'CD';
  return source.trim().slice(0, 2).toUpperCase();
});

const roleLabel = computed(() => {
  if (session.user?.system_role === 'system_admin') return '系统管理员';
  if (session.user?.can_manage_departments || session.user?.can_manage_accounts) return '部门管理员';
  const labels: Record<string, string> = {
    project_admin: '项目管理员',
    project_member: '项目成员',
    readonly_member: '只读成员',
  };
  return labels[session.user?.role || ''] || '工作台成员';
});

function toggleDesktopSidebar() {
  desktopCollapsed.value = !desktopCollapsed.value;
  localStorage.setItem('crewdock-sidebar-collapsed', String(desktopCollapsed.value));
}

function toggleGroup(id: string) {
  groupOpen.value[id] = !groupOpen.value[id];
  localStorage.setItem('crewdock-nav-groups', JSON.stringify(groupOpen.value));
}

async function logout() {
  if (loggingOut.value) return;
  loggingOut.value = true;
  try {
    await session.logout();
    await router.push('/login');
  } finally {
    loggingOut.value = false;
  }
}

function changeProject(event: Event) {
  if (!session.user) return;
  workspace.selectProject(session.user, (event.target as HTMLSelectElement).value);
}

function handleDocumentPointer(event: PointerEvent) {
  if (!userMenuContainer.value?.contains(event.target as Node)) userMenuOpen.value = false;
}

function handleDocumentKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') userMenuOpen.value = false;
}

function handleNavigationMedia(event: MediaQueryListEvent) {
  mobileViewport.value = event.matches;
}

watch(
  [
    () => route.fullPath,
    () => navGroups.value.map((group) => `${group.id}:${group.items.map((item) => item.path).join(',')}`).join('|'),
  ],
  () => {
    sidebarOpen.value = false;
    userMenuOpen.value = false;
    const activeGroup = navGroups.value.find((group) => group.items.some((item) => item.path === route.path));
    if (activeGroup) groupOpen.value[activeGroup.id] = true;
  },
  { immediate: true },
);

onMounted(() => {
  if (!session.user) session.loadMe().catch(() => undefined);
  document.addEventListener('pointerdown', handleDocumentPointer);
  document.addEventListener('keydown', handleDocumentKeydown);
  navigationMedia.addEventListener('change', handleNavigationMedia);
});

onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', handleDocumentPointer);
  document.removeEventListener('keydown', handleDocumentKeydown);
  navigationMedia.removeEventListener('change', handleNavigationMedia);
});
</script>

<template>
  <div class="shell" :class="{ 'sidebar-collapsed': desktopCollapsed }">
    <button
      v-if="sidebarOpen"
      class="sidebar-backdrop"
      type="button"
      aria-label="关闭导航"
      @click="sidebarOpen = false"
    />

    <aside class="sidebar" :class="{ open: sidebarOpen }">
      <div class="brand">
        <div class="brand-mark"><ShieldCheck :size="20" /></div>
        <div class="brand-copy"><strong>CrewDock</strong><span>共舟工作台</span></div>
        <button class="sidebar-collapse" type="button" :title="desktopCollapsed ? '展开侧栏' : '收起侧栏'" :aria-label="desktopCollapsed ? '展开侧栏' : '收起侧栏'" @click="toggleDesktopSidebar">
          <PanelLeftOpen v-if="desktopCollapsed" :size="18" />
          <PanelLeftClose v-else :size="18" />
        </button>
        <button class="sidebar-close" type="button" title="关闭导航" aria-label="关闭导航" @click="sidebarOpen = false"><X :size="19" /></button>
      </div>

      <nav class="nav" aria-label="主导航">
        <section v-for="group in navGroups" :key="group.id" class="nav-group">
          <button class="nav-group-toggle" type="button" :aria-expanded="desktopCollapsed || groupOpen[group.id]" :title="group.label" @click="toggleGroup(group.id)">
            <span>{{ group.label }}</span><ChevronDown :size="14" />
          </button>
          <div v-show="(!mobileViewport && desktopCollapsed) || groupOpen[group.id]" class="nav-group-items">
            <RouterLink v-for="item in group.items" :key="item.path" :to="item.path" class="nav-link" :class="{ active: route.path === item.path }" :title="desktopCollapsed ? item.label : undefined" :aria-label="item.label">
              <component :is="item.icon" :size="18" />
              <span>{{ item.label }}</span>
              <ChevronRight class="nav-chevron" :size="15" />
            </RouterLink>
          </div>
        </section>
      </nav>

      <div ref="userMenuContainer" class="sidebar-user-wrap">
        <div v-if="userMenuOpen" class="sidebar-user-menu" role="menu">
          <RouterLink to="/profile" role="menuitem"><CircleUserRound :size="17" /><span>个人账号</span></RouterLink>
          <button type="button" role="menuitem" :disabled="loggingOut" @click="logout"><LogOut :size="17" /><span>{{ loggingOut ? '正在退出' : '退出登录' }}</span></button>
        </div>
        <button class="sidebar-user" type="button" :aria-expanded="userMenuOpen" aria-haspopup="menu" title="账号菜单" @click.stop="userMenuOpen = !userMenuOpen">
          <span class="user-avatar">{{ initials }}</span>
          <span class="sidebar-user-copy"><strong>{{ session.user?.display_name || '正在加载' }}</strong><span>{{ roleLabel }}</span></span>
          <ChevronDown class="sidebar-user-chevron" :size="16" />
        </button>
        <a class="sidebar-version" :href="sourceUrl" target="_blank" rel="noreferrer" :title="`CrewDock v${appVersion} · AGPL-3.0-or-later 源代码`">v{{ appVersion }} · 源码</a>
      </div>
    </aside>

    <section class="main">
      <header class="topbar">
        <button class="mobile-menu" type="button" title="打开导航" aria-label="打开导航" @click="sidebarOpen = true"><Menu :size="21" /></button>
        <div class="topbar-context"><span>{{ workspace.company?.name || '共舟工作台' }}</span><ChevronRight :size="14" /><strong>{{ activeItem.label }}</strong></div>
        <div class="topbar-actions">
          <label v-if="projectContextVisible && workspace.projects.length" class="topbar-project" title="当前项目">
            <Briefcase :size="15" />
            <select :value="workspace.projectId" :disabled="workspace.loading" @change="changeProject">
              <option v-if="projectContextAllowsAll" value="">全部项目</option>
              <option v-for="project in workspace.projects" :key="project.id" :value="project.id">{{ project.name }}</option>
            </select>
          </label>
          <RouterLink class="topbar-user" to="/profile" title="个人账号"><span class="user-avatar small">{{ initials }}</span><span>{{ session.user?.display_name || '工作台成员' }}</span></RouterLink>
        </div>
      </header>
      <main class="content" :class="{ 'content-workspace': props.layout === 'workspace' }"><slot /></main>
    </section>
  </div>
</template>
