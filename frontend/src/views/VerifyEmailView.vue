<script setup lang="ts">
import { ArrowLeft, CircleCheck, KeyRound, LoaderCircle, Mail, RefreshCw } from '@lucide/vue';
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import AuthShell from '../components/AuthShell.vue';
import { api } from '../api';

const email = ref(sessionStorage.getItem('crewdock-verification-email') || '');
const code = ref('');
const submitting = ref(false);
const resending = ref(false);
const verified = ref(false);
const error = ref('');
const cooldown = ref(0);
let timer: number | undefined;

const canSubmit = computed(() => /\S+@\S+\.\S+/.test(email.value.trim()) && /^\d{6}$/.test(code.value));

function updateCooldown() {
  const availableAt = Number(sessionStorage.getItem('crewdock-verification-resend-at') || 0);
  cooldown.value = Math.max(0, Math.ceil((availableAt - Date.now()) / 1000));
}

function startCooldown(seconds: number) {
  sessionStorage.setItem('crewdock-verification-resend-at', String(Date.now() + seconds * 1000));
  updateCooldown();
}

function normalizeCode() {
  code.value = code.value.replace(/\D/g, '').slice(0, 6);
}

async function submit() {
  if (!canSubmit.value) return;
  submitting.value = true;
  error.value = '';
  try {
    await api.verifyEmail(email.value.trim().toLowerCase(), code.value);
    verified.value = true;
    sessionStorage.removeItem('crewdock-verification-email');
    sessionStorage.removeItem('crewdock-verification-resend-at');
  } catch (err) {
    error.value = err instanceof Error ? err.message : '邮箱验证失败';
  } finally {
    submitting.value = false;
  }
}

async function resend() {
  const cleanEmail = email.value.trim().toLowerCase();
  if (!cleanEmail || cooldown.value > 0) return;
  resending.value = true;
  error.value = '';
  try {
    const result = await api.resendVerification(cleanEmail);
    sessionStorage.setItem('crewdock-verification-email', cleanEmail);
    startCooldown(result.resend_cooldown_seconds);
    code.value = '';
  } catch (err) {
    error.value = err instanceof Error ? err.message : '重新发送验证码失败';
  } finally {
    resending.value = false;
  }
}

onMounted(() => {
  updateCooldown();
  timer = window.setInterval(updateCooldown, 1000);
});

onBeforeUnmount(() => {
  if (timer) window.clearInterval(timer);
});
</script>

<template>
  <AuthShell tall>
    <header class="login-header">
      <span>邮箱验证</span>
      <h2>{{ verified ? '申请已进入审核' : '输入邮箱验证码' }}</h2>
      <p>{{ verified ? '管理员审核通过后，你会收到账号激活邮件。' : '验证码有效期为 15 分钟，请勿转发给其他人。' }}</p>
    </header>

    <div v-if="verified" class="auth-result success">
      <CircleCheck :size="24" />
      <strong>邮箱已验证</strong>
      <span>申请已进入管理员审核，无需重复提交。</span>
    </div>
    <form v-else class="login-form auth-form verification-form" @submit.prevent="submit">
      <label class="field"><span>申请邮箱</span><div class="auth-input-wrap"><Mail :size="17" /><input v-model="email" type="email" autocomplete="email" required /></div></label>
      <label class="field"><span>六位验证码</span><div class="auth-input-wrap verification-code-input"><KeyRound :size="17" /><input v-model="code" inputmode="numeric" autocomplete="one-time-code" maxlength="6" placeholder="000000" required @input="normalizeCode" /></div></label>
      <div class="verification-resend-row"><span>没有收到邮件？</span><button type="button" :disabled="resending || cooldown > 0 || !email.trim()" @click="resend"><LoaderCircle v-if="resending" class="spin" :size="14" /><RefreshCw v-else :size="14" />{{ cooldown > 0 ? `${cooldown} 秒后可重发` : resending ? '发送中...' : '重新发送' }}</button></div>
      <p v-if="error" class="error login-error" role="alert">{{ error }}</p>
      <button class="primary-button login-submit" :disabled="submitting || !canSubmit"><span>{{ submitting ? '正在验证...' : '确认验证码' }}</span><LoaderCircle v-if="submitting" class="spin" :size="17" /><CircleCheck v-else :size="17" /></button>
    </form>

    <RouterLink class="auth-back-link" to="/login"><ArrowLeft :size="15" />返回登录</RouterLink>
  </AuthShell>
</template>
