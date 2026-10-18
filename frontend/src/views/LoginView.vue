<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { useRouter } from 'vue-router';
import { ArrowRight, LockKeyhole, Mail } from '@lucide/vue';
import AuthShell from '../components/AuthShell.vue';
import { api } from '../api';
import { useSessionStore } from '../stores/session';

const router = useRouter();
const session = useSessionStore();
const email = ref('');
const password = ref('');
const error = ref('');
const registrationAvailable = ref(false);
const serviceState = ref<'checking' | 'online' | 'offline'>('checking');

async function submit() {
  error.value = '';
  try {
    await session.login(email.value, password.value);
    router.push('/');
  } catch (err) {
    error.value = err instanceof Error ? err.message : '登录失败';
  }
}

onMounted(async () => {
  const [healthResult, policyResult] = await Promise.allSettled([
    api.health(),
    api.registrationPolicy(),
  ]);
  serviceState.value = healthResult.status === 'fulfilled' && healthResult.value.status === 'ok'
    ? 'online'
    : 'offline';
  if (policyResult.status === 'fulfilled') {
    registrationAvailable.value = policyResult.value.registration_mode === 'application'
      && policyResult.value.email_ready;
  }
});
</script>

<template>
  <AuthShell :service-state="serviceState">
    <header class="login-header">
      <span>欢迎回来</span>
      <h2>登录工作台</h2>
      <p>使用分配给你的工作账号继续。</p>
    </header>

    <form class="login-form" @submit.prevent="submit">
      <label class="field">
        <span>邮箱账号</span>
        <div class="input-shell">
          <Mail :size="18" />
          <input v-model="email" type="email" autocomplete="username" placeholder="name@example.com" required />
        </div>
      </label>
      <label class="field">
        <span>密码</span>
        <div class="input-shell">
          <LockKeyhole :size="18" />
          <input
            v-model="password"
            type="password"
            autocomplete="current-password"
            placeholder="输入密码"
            required
          />
        </div>
      </label>
      <p v-if="error" class="error login-error" role="alert">{{ error }}</p>
      <button class="primary-button login-submit" :disabled="session.loading">
        <span>{{ session.loading ? '正在登录...' : '进入工作台' }}</span>
        <ArrowRight v-if="!session.loading" :size="18" />
      </button>
      <RouterLink v-if="registrationAvailable" class="auth-secondary-link" to="/register">
        申请加入工作台
      </RouterLink>
    </form>
  </AuthShell>
</template>
