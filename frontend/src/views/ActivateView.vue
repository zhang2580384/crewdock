<script setup lang="ts">
import { ArrowLeft, KeyRound } from '@lucide/vue';
import { reactive, ref } from 'vue';
import { useRoute } from 'vue-router';
import AuthShell from '../components/AuthShell.vue';
import { api } from '../api';
import { consumeAuthToken } from '../auth-token';

const route = useRoute();
const token = consumeAuthToken(route);
const submitting = ref(false);
const activatedEmail = ref('');
const error = ref(token ? '' : '激活链接缺少 token');
const form = reactive({ password: '', confirm_password: '' });

async function submit() {
  error.value = '';
  if (form.password.length < 12 || form.password.length > 128) {
    error.value = '密码长度需要在 12 到 128 位之间';
    return;
  }
  if (form.password !== form.confirm_password) {
    error.value = '两次输入的密码不一致';
    return;
  }
  submitting.value = true;
  try {
    const result = await api.activate(token, form.password);
    activatedEmail.value = result.email;
  } catch (err) {
    error.value = err instanceof Error ? err.message : '账号激活失败';
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <AuthShell>
    <header class="login-header">
      <span>账号激活</span>
      <h2>{{ activatedEmail ? '账号已激活' : '设置登录密码' }}</h2>
      <p>{{ activatedEmail ? `${activatedEmail} 可以登录工作台。` : '设置仅由你掌握的工作台密码。' }}</p>
    </header>
    <div v-if="activatedEmail" class="auth-result success"><KeyRound :size="22" /><strong>密码设置完成</strong></div>
    <form v-else class="login-form" @submit.prevent="submit">
      <label class="field"><span>新密码</span><input v-model="form.password" type="password" autocomplete="new-password" minlength="12" maxlength="128" required /></label>
      <label class="field"><span>确认密码</span><input v-model="form.confirm_password" type="password" autocomplete="new-password" minlength="12" maxlength="128" required /></label>
      <p v-if="error" class="error login-error" role="alert">{{ error }}</p>
      <button class="primary-button login-submit" :disabled="submitting || !token"><KeyRound :size="17" />{{ submitting ? '正在激活...' : '激活账号' }}</button>
    </form>
    <RouterLink class="auth-back-link" to="/login"><ArrowLeft :size="15" />返回登录</RouterLink>
  </AuthShell>
</template>
