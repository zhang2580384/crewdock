import { createRouter, createWebHistory } from 'vue-router';
import { api, getToken } from './api';
import LoginView from './views/LoginView.vue';

async function requireDepartmentManagement() {
  try {
    const { user } = await api.me();
    return user.can_manage_departments ? true : '/';
  } catch {
    return '/login';
  }
}

async function requireSystemAdmin() {
  try {
    const { user } = await api.me();
    return user.system_role === 'system_admin' ? true : '/';
  } catch {
    return '/login';
  }
}

async function requireAuditView() {
  try {
    const { user } = await api.me();
    return user.can_view_audit ? true : '/';
  } catch {
    return '/login';
  }
}

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/login', component: LoginView },
    { path: '/register', component: () => import('./views/RegisterView.vue') },
    { path: '/verify-email', component: () => import('./views/VerifyEmailView.vue') },
    { path: '/activate', component: () => import('./views/ActivateView.vue') },
    { path: '/', component: () => import('./views/DashboardView.vue') },
    { path: '/sites', component: () => import('./views/SitesView.vue') },
    { path: '/seals', component: () => import('./views/SealsView.vue') },
    { path: '/signing', component: () => import('./views/SigningToolView.vue') },
    { path: '/projects', component: () => import('./views/ProjectsView.vue') },
    { path: '/docs', component: () => import('./views/DocsView.vue') },
    { path: '/vault', component: () => import('./views/VaultView.vue') },
    { path: '/files', component: () => import('./views/FilesView.vue') },
    { path: '/tickets', component: () => import('./views/TicketsView.vue') },
    { path: '/audit', component: () => import('./views/AuditView.vue'), beforeEnter: requireAuditView },
    { path: '/users', component: () => import('./views/UsersView.vue') },
    {
      path: '/organization',
      component: () => import('./views/OrganizationView.vue'),
      beforeEnter: requireDepartmentManagement,
    },
    { path: '/settings', component: () => import('./views/SettingsView.vue'), beforeEnter: requireSystemAdmin },
    { path: '/profile', component: () => import('./views/ProfileView.vue') },
    { path: '/shares', component: () => import('./views/SharesView.vue') },
    { path: '/shared/:id', component: () => import('./views/SharedView.vue') },
  ],
});

router.beforeEach((to) => {
  const isPublic = ['/login', '/register', '/verify-email', '/activate'].includes(to.path)
    || to.path.startsWith('/shared/');
  if (!isPublic && !getToken()) {
    return '/login';
  }
  return true;
});
