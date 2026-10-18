<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue';
import { KeyRound, LoaderCircle, LogOut, MonitorSmartphone, RefreshCw } from '@lucide/vue';
import { useRouter } from 'vue-router';
import WorkbenchShell from '../components/WorkbenchShell.vue';
import { api } from '../api';
import { useSessionStore } from '../stores/session';
import type { Department, UserRole, UserSession } from '../types';

const router = useRouter();
const session = useSessionStore();
const submitting = ref(false);
const sessionsLoading = ref(true);
const revokingId = ref('');
const error = ref('');
const success = ref('');
const activeSessions = ref<UserSession[]>([]);
const departments = ref<Department[]>([]);
const form = reactive({ current_password: '', new_password: '', confirm_password: '' });

const roleLabels: Record<UserRole, string> = {
  system_admin: '系统管理员',
  department_admin: '部门管理员',
  project_admin: '项目管理员',
  project_member: '项目成员',
  readonly_member: '只读成员',
};

const roleLabel = computed(() => {
  if (session.user?.system_role === 'system_admin') return '系统管理员';
  if (session.user?.can_manage_departments || session.user?.can_manage_accounts) return '部门管理员';
  const role = session.user?.role;
  return role ? roleLabels[role] : '-';
});
const otherSessions = computed(() => activeSessions.value.filter((item) => !item.current));
const primaryDepartmentName = computed(() => {
  const departmentId = session.user?.department_id;
  if (!departmentId) return '未分配';
  return departments.value.find((department) => department.id === departmentId)?.name || '未知部门';
});

function setMessage(nextError = '', nextSuccess = '') {
  error.value = nextError;
  success.value = nextSuccess;
}

async function submit() {
  setMessage();
  if (form.new_password.length < 12 || form.new_password.length > 128) {
    error.value = '新密码长度需要在 12 到 128 位之间';
    return;
  }
  if (form.new_password !== form.confirm_password) {
    error.value = '两次输入的新密码不一致';
    return;
  }
  submitting.value = true;
  try {
    await api.changePassword(form.current_password, form.new_password);
    session.clearLocalSession();
    await router.replace('/login');
  } catch (err) {
    error.value = err instanceof Error ? err.message : '修改密码失败';
  } finally {
    submitting.value = false;
  }
}

async function loadSessions() {
  sessionsLoading.value = true;
  try {
    activeSessions.value = await api.sessions();
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '加载登录设备失败');
  } finally {
    sessionsLoading.value = false;
  }
}

async function revokeSession(item: UserSession) {
  revokingId.value = item.id;
  setMessage();
  try {
    await api.revokeSession(item.id);
    if (item.current) {
      session.clearLocalSession();
      await router.replace('/login');
      return;
    }
    activeSessions.value = activeSessions.value.filter((row) => row.id !== item.id);
    setMessage('', '设备会话已撤销');
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '撤销会话失败');
  } finally {
    revokingId.value = '';
  }
}

async function revokeOtherSessions() {
  if (!otherSessions.value.length) return;
  revokingId.value = 'others';
  setMessage();
  try {
    await Promise.all(otherSessions.value.map((item) => api.revokeSession(item.id)));
    activeSessions.value = activeSessions.value.filter((item) => item.current);
    setMessage('', '其他设备均已退出');
  } catch (err) {
    await loadSessions();
    setMessage(err instanceof Error ? err.message : '退出其他设备失败');
  } finally {
    revokingId.value = '';
  }
}

function formatTime(value: string) {
  return new Date(value).toLocaleString('zh-CN', { hour12: false });
}

onMounted(async () => {
  await session.loadMe().catch(() => undefined);
  const [, departmentRows] = await Promise.all([
    loadSessions(),
    api.departments().catch(() => [] as Department[]),
  ]);
  departments.value = departmentRows;
});
</script>

<template>
  <WorkbenchShell>
    <div class="page-head"><div><h1>个人账号</h1><p>账号安全、登录密码与设备会话。</p></div></div>

    <p v-if="error" class="error profile-message" role="alert">{{ error }}</p>
    <p v-if="success" class="success profile-message" role="status">{{ success }}</p>

    <div class="profile-layout">
      <section class="panel profile-summary">
        <div class="panel-head"><h2>账号信息</h2></div>
        <dl class="profile-details">
          <div><dt>姓名</dt><dd>{{ session.user?.display_name || '-' }}</dd></div>
          <div><dt>邮箱</dt><dd>{{ session.user?.email || '-' }}</dd></div>
          <div><dt>角色</dt><dd>{{ roleLabel }}</dd></div>
          <div><dt>部门</dt><dd>{{ primaryDepartmentName }}</dd></div>
        </dl>
      </section>

      <section class="panel form-panel">
        <div class="panel-head"><h2>修改密码</h2></div>
        <form class="password-form" @submit.prevent="submit">
          <label class="field"><span>当前密码</span><input v-model="form.current_password" type="password" autocomplete="current-password" required /></label>
          <label class="field"><span>新密码</span><input v-model="form.new_password" type="password" autocomplete="new-password" minlength="12" maxlength="128" required /></label>
          <label class="field"><span>确认新密码</span><input v-model="form.confirm_password" type="password" autocomplete="new-password" minlength="12" maxlength="128" required /></label>
          <div class="form-actions"><button class="primary-button" type="submit" :disabled="submitting"><KeyRound :size="17" /><span>{{ submitting ? '正在修改...' : '修改密码' }}</span></button></div>
        </form>
      </section>

      <section class="panel profile-sessions">
        <div class="panel-head profile-session-head">
          <div><h2>登录设备</h2><p>{{ activeSessions.length }} 个有效会话</p></div>
          <div class="profile-session-actions">
            <button class="icon-button" type="button" title="刷新设备会话" aria-label="刷新设备会话" :disabled="sessionsLoading" @click="loadSessions"><RefreshCw :size="16" /></button>
            <button class="secondary-button" type="button" :disabled="!otherSessions.length || revokingId === 'others'" @click="revokeOtherSessions"><LogOut :size="16" />{{ revokingId === 'others' ? '正在退出...' : '退出其他设备' }}</button>
          </div>
        </div>
        <div v-if="sessionsLoading" class="profile-session-state"><LoaderCircle class="spin" :size="20" />正在加载...</div>
        <div v-else-if="!activeSessions.length" class="profile-session-state">没有有效设备会话</div>
        <div v-else class="profile-session-list">
          <article v-for="item in activeSessions" :key="item.id" class="profile-session-row">
            <span class="profile-session-icon"><MonitorSmartphone :size="19" /></span>
            <div><strong>{{ item.current ? '当前设备' : item.client_ip || '未知设备' }}</strong><span>{{ item.client_ip || '未记录 IP' }} · 最近活动 {{ formatTime(item.last_seen_at) }}</span><small>有效至 {{ formatTime(item.expires_at) }}</small></div>
            <button class="secondary-button compact-action" type="button" :disabled="Boolean(revokingId)" @click="revokeSession(item)">{{ revokingId === item.id ? '撤销中...' : item.current ? '退出此设备' : '撤销' }}</button>
          </article>
        </div>
      </section>
    </div>
  </WorkbenchShell>
</template>

<style scoped>
.profile-layout { display: grid; grid-template-columns: minmax(0, 1fr) minmax(320px, 1fr); gap: 20px; align-items: start; }
.profile-message { margin: 0 0 14px; }
.profile-details { margin: 0; }
.profile-details div { display: grid; grid-template-columns: 96px minmax(0, 1fr); gap: 16px; padding: 14px 0; border-bottom: 1px solid var(--line); }
.profile-details div:last-child { border-bottom: 0; }
.profile-details dt { color: var(--muted); }
.profile-details dd { margin: 0; overflow-wrap: anywhere; }
.password-form { display: grid; gap: 16px; }
.profile-sessions { grid-column: 1 / -1; }
.profile-session-head, .profile-session-actions { display: flex; align-items: center; }
.profile-session-head { justify-content: space-between; gap: 16px; }
.profile-session-head h2, .profile-session-head p { margin: 0; }
.profile-session-head p { margin-top: 3px; color: var(--muted); font-size: 11px; }
.profile-session-actions { gap: 8px; }
.profile-session-state { min-height: 120px; display: flex; align-items: center; justify-content: center; gap: 8px; color: var(--muted); }
.profile-session-row { min-width: 0; min-height: 78px; display: grid; grid-template-columns: 38px minmax(0, 1fr) auto; align-items: center; gap: 12px; padding: 12px 0; border-bottom: 1px solid var(--line); }
.profile-session-row:last-child { border-bottom: 0; }
.profile-session-icon { width: 38px; height: 38px; display: grid; place-items: center; border-radius: 7px; background: var(--panel-muted); color: var(--muted-strong); }
.profile-session-row strong, .profile-session-row span, .profile-session-row small { display: block; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.profile-session-row strong { font-size: 13px; }
.profile-session-row span, .profile-session-row small { margin-top: 3px; color: var(--muted); font-size: 11px; }
@media (max-width: 820px) { .profile-layout { grid-template-columns: 1fr; } .profile-sessions { grid-column: auto; } }
@media (max-width: 620px) { .profile-session-head { align-items: stretch; flex-direction: column; } .profile-session-actions { display: grid; grid-template-columns: 36px minmax(0, 1fr); } .profile-session-row { grid-template-columns: 34px minmax(0, 1fr); } .profile-session-row > button { grid-column: 2; justify-self: start; } }
</style>
