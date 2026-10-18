<script setup lang="ts">
import { ArrowLeft, Send } from '@lucide/vue';
import { onMounted, reactive, ref } from 'vue';
import { useRouter } from 'vue-router';
import AuthShell from '../components/AuthShell.vue';
import { api } from '../api';
import type { RegistrationPolicy } from '../types';

const policy = ref<RegistrationPolicy | null>(null);
const router = useRouter();
const loadingPolicy = ref(true);
const submitting = ref(false);
const submitted = ref(false);
const error = ref('');
const form = reactive({
  display_name: '',
  email: '',
  department_interest: '',
  reason: '',
});

async function submit() {
  error.value = '';
  submitting.value = true;
  try {
    const result = await api.register({
      email: form.email.trim(),
      display_name: form.display_name.trim(),
      department_interest: form.department_interest.trim() || undefined,
      reason: form.reason.trim() || undefined,
    });
    sessionStorage.setItem('crewdock-verification-email', form.email.trim().toLowerCase());
    sessionStorage.setItem(
      'crewdock-verification-resend-at',
      String(Date.now() + result.resend_cooldown_seconds * 1000),
    );
    submitted.value = true;
    await router.push('/verify-email');
  } catch (err) {
    error.value = err instanceof Error ? err.message : '提交申请失败';
  } finally {
    submitting.value = false;
  }
}

onMounted(async () => {
  try {
    policy.value = await api.registrationPolicy();
  } catch (err) {
    error.value = err instanceof Error ? err.message : '无法读取注册策略';
  } finally {
    loadingPolicy.value = false;
  }
});
</script>

<template>
  <AuthShell tall>
    <header class="login-header">
      <span>访问申请</span>
      <h2>{{ submitted ? '检查你的邮箱' : '申请加入工作台' }}</h2>
      <p>{{ submitted ? '验证邮箱后，申请才会进入管理员审核。' : '提交员工身份信息，由管理员审核访问范围。' }}</p>
    </header>

    <div v-if="loadingPolicy" class="auth-inline-state">正在读取申请策略...</div>
    <div v-else-if="submitted" class="auth-result success">
      <Send :size="21" />
      <strong>申请已提交</strong>
      <span>验证邮件已发送至 {{ form.email }}。</span>
    </div>
    <div v-else-if="policy?.registration_mode !== 'application' || !policy.email_ready" class="auth-result">
      <strong>暂未开放员工申请</strong>
      <span>请联系系统管理员创建或邀请账号。</span>
    </div>
    <form v-else class="login-form auth-form" @submit.prevent="submit">
      <label class="field"><span>姓名</span><input v-model="form.display_name" autocomplete="name" required /></label>
      <label class="field"><span>{{ policy.allow_all_email_domains ? '邮箱' : '企业邮箱' }}</span><input v-model="form.email" type="email" autocomplete="email" required /></label>
      <label class="field"><span>部门意向 <small>选填</small></span><input v-model="form.department_interest" /></label>
      <label class="field"><span>申请理由 <small>选填</small></span><textarea v-model="form.reason" rows="3" maxlength="500" /></label>
      <p v-if="policy.allow_all_email_domains" class="field-hint">
        当前接受所有邮箱申请，验证后仍需管理员审核。
      </p>
      <p v-else-if="policy.allowed_email_domains.length" class="field-hint">
        允许的邮箱域名：{{ policy.allowed_email_domains.join('、') }}
      </p>
      <p v-if="error" class="error login-error" role="alert">{{ error }}</p>
      <button class="primary-button login-submit" :disabled="submitting">
        <span>{{ submitting ? '正在提交...' : '提交申请' }}</span><Send v-if="!submitting" :size="17" />
      </button>
    </form>

    <RouterLink class="auth-back-link" to="/login"><ArrowLeft :size="15" />返回登录</RouterLink>
  </AuthShell>
</template>
