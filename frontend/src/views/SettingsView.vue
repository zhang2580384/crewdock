<script setup lang="ts">
import {
  BadgeInfo,
  Building2,
  CircleCheck,
  CircleX,
  CloudDownload,
  FileCode2,
  Download,
  LoaderCircle,
  LockKeyhole,
  Mail,
  RefreshCw,
  RotateCcw,
  Save,
  Search,
  Send,
  ShieldCheck,
} from '@lucide/vue';
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import WorkbenchShell from '../components/WorkbenchShell.vue';
import { api } from '../api';
import { useSessionStore } from '../stores/session';
import { useWorkspaceStore } from '../stores/workspace';
import type {
  AuthSettings,
  EmailSettings,
  EmailTemplate,
  EmailTemplateKey,
  HealthInfo,
  RegistrationMode,
  SmtpTlsMode,
  SystemUpdateState,
  SystemUpdateStatus,
} from '../types';

type SettingsTab = 'basic' | 'email' | 'templates' | 'auth' | 'version';

const session = useSessionStore();
const workspace = useWorkspaceStore();
const activeTab = ref<SettingsTab>('basic');
const loading = ref(true);
const savingEmail = ref(false);
const savingAuth = ref(false);
const savingTemplate = ref(false);
const testingEmail = ref(false);
const checkingUpdates = ref(false);
const requestingUpdate = ref(false);
const updateConfirmation = ref(false);
const error = ref('');
const success = ref('');
const health = ref<HealthInfo | null>(null);
const emailSettings = ref<EmailSettings | null>(null);
const authSettings = ref<AuthSettings | null>(null);
const emailTemplates = ref<EmailTemplate[]>([]);
const updateStatus = ref<SystemUpdateStatus | null>(null);
const selectedTemplateKey = ref<EmailTemplateKey>('registration_verification');
const templateEditor = ref<HTMLTextAreaElement | null>(null);
const appVersion = __APP_VERSION__;
const gitSha = __GIT_SHA__;
const buildTime = __BUILD_TIME__;
let updatePollTimer: number | null = null;
let reloadScheduled = false;

const emailForm = reactive({
  enabled: false,
  smtp_host: '',
  smtp_port: 587,
  tls_mode: 'starttls' as SmtpTlsMode,
  smtp_username: '',
  smtp_password: '',
  clear_smtp_password: false,
  from_email: '',
  from_name: '',
  reply_to: '',
});
const authForm = reactive({
  registration_mode: 'closed' as RegistrationMode,
  allow_all_email_domains: false,
  allowed_domains: '',
});
const templateForm = reactive({
  subject: '',
  html_body: '',
});

const tabs = [
  { id: 'basic' as const, label: '基本', icon: Building2 },
  { id: 'email' as const, label: '邮件', icon: Mail },
  { id: 'templates' as const, label: '邮件模板', icon: FileCode2 },
  { id: 'auth' as const, label: '登录与注册', icon: LockKeyhole },
  { id: 'version' as const, label: '更新', icon: CloudDownload },
];

const serviceOnline = computed(() => health.value?.status === 'ok');
const versionsAligned = computed(() => health.value?.version === appVersion);
const activeUpdateStates = new Set<SystemUpdateState>([
  'queued',
  'checking',
  'backing_up',
  'downloading',
  'building',
  'deploying',
  'verifying',
  'rolling_back',
]);
const updateRunning = computed(() => Boolean(updateStatus.value && activeUpdateStates.has(updateStatus.value.state)));
const updateStateLabel = computed(() => {
  const labels: Record<SystemUpdateState, string> = {
    not_configured: '未配置',
    idle: '待检查',
    queued: '已排队',
    checking: '检查中',
    available: '发现更新',
    up_to_date: '已是最新',
    backing_up: '正在备份',
    downloading: '正在下载',
    building: '正在构建',
    deploying: '正在切换',
    verifying: '正在验证',
    rolling_back: '正在回滚',
    rolled_back: '已回滚',
    completed: '更新完成',
    failed: '更新失败',
  };
  return updateStatus.value ? labels[updateStatus.value.state] : '未连接';
});
const updateStateTone = computed(() => {
  const state = updateStatus.value?.state;
  if (state === 'failed' || state === 'rolled_back') return 'danger';
  if (state === 'available') return 'notice';
  if (state === 'completed' || state === 'up_to_date') return 'success';
  if (state && activeUpdateStates.has(state)) return 'working';
  return 'neutral';
});
const selectedTemplate = computed(() => emailTemplates.value.find((item) => item.key === selectedTemplateKey.value) || null);
const previewVariables: Record<string, string> = {
  recipient_name: '团队成员',
  verification_code: '683215',
  expires_in_minutes: '15',
  expires_in_hours: '72',
  site_name: '共舟工作台',
  activation_url: 'https://work.example.com/activate',
  rejection_reason: '请补充所属部门信息后联系管理员。',
  inviter_name: '系统管理员',
};
function renderPreviewVariables(value: string) {
  let rendered = value;
  for (const [key, replacement] of Object.entries(previewVariables)) {
    rendered = rendered.replace(new RegExp(`{{\\s*${key}\\s*}}`, 'g'), replacement);
  }
  return rendered;
}
const templatePreview = computed(() => renderPreviewVariables(templateForm.html_body));
const templatePreviewSubject = computed(() => renderPreviewVariables(templateForm.subject) || '未填写主题');

function hydrateEmail(settings: EmailSettings) {
  emailSettings.value = settings;
  emailForm.enabled = settings.enabled;
  emailForm.smtp_host = settings.smtp_host;
  emailForm.smtp_port = settings.smtp_port;
  emailForm.tls_mode = settings.tls_mode;
  emailForm.smtp_username = settings.smtp_username || '';
  emailForm.smtp_password = '';
  emailForm.clear_smtp_password = false;
  emailForm.from_email = settings.from_email;
  emailForm.from_name = settings.from_name;
  emailForm.reply_to = settings.reply_to || '';
}

function hydrateAuth(settings: AuthSettings) {
  authSettings.value = settings;
  authForm.registration_mode = settings.registration_mode;
  authForm.allow_all_email_domains = settings.allow_all_email_domains;
  authForm.allowed_domains = settings.allowed_email_domains.join('\n');
}

function hydrateTemplate(template: EmailTemplate | null) {
  templateForm.subject = template?.subject || '';
  templateForm.html_body = template?.html_body || '';
}

function setMessage(nextError = '', nextSuccess = '') {
  error.value = nextError;
  success.value = nextSuccess;
}

async function load() {
  loading.value = true;
  setMessage();
  try {
    await session.loadMe();
    if (session.user?.system_role !== 'system_admin') throw new Error('仅系统管理员可以访问系统设置');
    const [company, nextHealth, nextEmail, nextAuth, nextTemplates, nextUpdateStatus] = await Promise.all([
      api.company(),
      api.health(),
      api.emailSettings(),
      api.authSettings(),
      api.emailTemplates(),
      api.updateStatus().catch(() => null),
    ]);
    workspace.company = company;
    health.value = nextHealth;
    hydrateEmail(nextEmail);
    hydrateAuth(nextAuth);
    emailTemplates.value = nextTemplates;
    if (nextUpdateStatus) acceptUpdateStatus(nextUpdateStatus);
    hydrateTemplate(nextTemplates.find((item) => item.key === selectedTemplateKey.value) || nextTemplates[0] || null);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '加载系统设置失败');
  } finally {
    loading.value = false;
  }
}

async function saveEmail() {
  savingEmail.value = true;
  setMessage();
  try {
    const settings = await api.updateEmailSettings({
      enabled: emailForm.enabled,
      smtp_host: emailForm.smtp_host.trim(),
      smtp_port: Number(emailForm.smtp_port),
      tls_mode: emailForm.tls_mode,
      smtp_username: emailForm.smtp_username.trim() || undefined,
      smtp_password: emailForm.smtp_password || undefined,
      clear_smtp_password: emailForm.clear_smtp_password,
      from_email: emailForm.from_email.trim(),
      from_name: emailForm.from_name.trim(),
      reply_to: emailForm.reply_to.trim() || undefined,
    });
    hydrateEmail(settings);
    setMessage('', '邮件设置已保存');
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '保存邮件设置失败');
  } finally {
    savingEmail.value = false;
  }
}

async function testEmail() {
  testingEmail.value = true;
  setMessage();
  try {
    const result = await api.testEmailSettings();
    setMessage('', `测试邮件已加入发送队列：${result.recipient}`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '测试邮件发送失败');
  } finally {
    testingEmail.value = false;
  }
}

async function refreshEmailStatus() {
  try {
    hydrateEmail(await api.emailSettings());
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '刷新邮件状态失败');
  }
}

async function saveAuth() {
  savingAuth.value = true;
  setMessage();
  try {
    const allowed_email_domains = Array.from(new Set(
      authForm.allowed_domains.split(/[\n,，\s]+/).map((item) => item.trim().toLowerCase()).filter(Boolean),
    ));
    if (
      authForm.registration_mode === 'application'
      && !authForm.allow_all_email_domains
      && !allowed_email_domains.length
    ) {
      throw new Error('开放员工申请时至少需要配置一个企业邮箱域名');
    }
    hydrateAuth(await api.updateAuthSettings({
      registration_mode: authForm.registration_mode,
      allow_all_email_domains: authForm.allow_all_email_domains,
      allowed_email_domains,
    }));
    setMessage('', '登录与注册策略已保存');
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '保存注册策略失败');
  } finally {
    savingAuth.value = false;
  }
}

async function saveTemplate() {
  const template = selectedTemplate.value;
  if (!template) return;
  savingTemplate.value = true;
  setMessage();
  try {
    const updated = await api.updateEmailTemplate(template.key, {
      subject: templateForm.subject.trim(),
      html_body: templateForm.html_body,
    });
    emailTemplates.value = emailTemplates.value.map((item) => item.key === updated.key ? updated : item);
    hydrateTemplate(updated);
    setMessage('', `${updated.label}模板已保存`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '保存邮件模板失败');
  } finally {
    savingTemplate.value = false;
  }
}

function restoreTemplateDefault() {
  const template = selectedTemplate.value;
  if (!template) return;
  templateForm.subject = template.default_subject;
  templateForm.html_body = template.default_html_body;
  setMessage('', '已恢复默认内容，保存后生效');
}

async function insertTemplateVariable(variable: string) {
  const token = `{{${variable}}}`;
  const editor = templateEditor.value;
  if (!editor) {
    templateForm.html_body += token;
    return;
  }
  const start = editor.selectionStart;
  const end = editor.selectionEnd;
  templateForm.html_body = `${templateForm.html_body.slice(0, start)}${token}${templateForm.html_body.slice(end)}`;
  await nextTick();
  editor.focus();
  editor.setSelectionRange(start + token.length, start + token.length);
}

function formatTime(value?: string | null) {
  return value ? new Date(value).toLocaleString('zh-CN', { hour12: false }) : '暂无';
}

function stopUpdatePolling() {
  if (updatePollTimer !== null) window.clearInterval(updatePollTimer);
  updatePollTimer = null;
}

function acceptUpdateStatus(status: SystemUpdateStatus) {
  updateStatus.value = status;
  if (activeUpdateStates.has(status.state)) startUpdatePolling();
  else stopUpdatePolling();
  if (status.state === 'completed' && status.current_version !== `v${appVersion}` && !reloadScheduled) {
    reloadScheduled = true;
    setMessage('', `${status.current_version} 已部署，正在载入新版界面...`);
    window.setTimeout(() => window.location.reload(), 1800);
  }
}

function startUpdatePolling() {
  if (updatePollTimer !== null) return;
  updatePollTimer = window.setInterval(() => void refreshUpdateStatus(true), 2500);
}

async function refreshUpdateStatus(silent = false) {
  try {
    const status = await api.updateStatus();
    acceptUpdateStatus(status);
    if (activeUpdateStates.has(status.state)) startUpdatePolling();
  } catch (err) {
    if (!silent) setMessage(err instanceof Error ? err.message : '刷新更新状态失败');
  }
}

async function checkForUpdates() {
  checkingUpdates.value = true;
  updateConfirmation.value = false;
  setMessage();
  try {
    acceptUpdateStatus(await api.checkForUpdates());
    startUpdatePolling();
    setMessage('', '检查任务已提交');
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '检查更新失败');
  } finally {
    checkingUpdates.value = false;
  }
}

async function applyUpdate() {
  const target = updateStatus.value?.latest_version;
  if (!target) return;
  requestingUpdate.value = true;
  setMessage();
  try {
    acceptUpdateStatus(await api.applyUpdate(target));
    updateConfirmation.value = false;
    startUpdatePolling();
    setMessage('', `${target} 更新任务已提交，页面会在服务切换后自动恢复`);
  } catch (err) {
    setMessage(err instanceof Error ? err.message : '提交更新失败');
  } finally {
    requestingUpdate.value = false;
  }
}

watch(selectedTemplateKey, () => hydrateTemplate(selectedTemplate.value));
watch(activeTab, (tab) => {
  if (tab === 'version') void refreshUpdateStatus(true);
});

onMounted(load);
onBeforeUnmount(stopUpdatePolling);
</script>

<template>
  <WorkbenchShell>
    <div class="page-head settings-page-head">
      <div><h1>系统设置</h1><p>组织级邮件、注册策略与运行版本。</p></div>
    </div>

    <p v-if="error" class="error settings-message" role="alert">{{ error }}</p>
    <p v-if="success" class="success settings-message" role="status">{{ success }}</p>

    <div v-if="loading" class="loading-state"><LoaderCircle class="spin" :size="22" /><span>正在读取设置...</span></div>
    <section v-else class="settings-surface">
      <nav class="settings-tabs" aria-label="设置分类">
        <button v-for="tab in tabs" :key="tab.id" type="button" :class="{ active: activeTab === tab.id }" @click="activeTab = tab.id">
          <component :is="tab.icon" :size="17" /><span>{{ tab.label }}</span>
        </button>
      </nav>

      <div v-if="activeTab === 'basic'" class="settings-content">
        <header class="settings-section-head"><div><h2>基本信息</h2><p>当前组织和服务连接状态。</p></div></header>
        <dl class="settings-facts">
          <div><dt>组织名称</dt><dd>{{ workspace.company?.name || '-' }}</dd></div>
          <div><dt>组织标识</dt><dd>{{ workspace.company?.slug || '-' }}</dd></div>
          <div><dt>服务状态</dt><dd class="settings-status" :class="serviceOnline ? 'online' : 'offline'"><i />{{ serviceOnline ? '运行正常' : '不可用' }}</dd></div>
          <div><dt>公开地址</dt><dd>{{ health?.public_base_url || '-' }}</dd></div>
        </dl>
        <div class="settings-actions"><a class="secondary-button as-link" href="/crewdock-launcher-install.cmd" download><Download :size="17" />安装本机连接启动器</a></div>
      </div>

      <form v-else-if="activeTab === 'email'" class="settings-content" @submit.prevent="saveEmail">
        <header class="settings-section-head">
          <div><h2>邮件发送</h2><p>用于员工邀请、邮箱验证和账号激活。</p></div>
          <label class="switch-row" :title="health?.master_key_configured ? '启用邮件' : '部署环境尚未配置 CREWDOCK_MASTER_KEY'"><input v-model="emailForm.enabled" type="checkbox" :disabled="!health?.master_key_configured && !emailForm.enabled" /><span>启用邮件</span></label>
        </header>
        <div v-if="!health?.master_key_configured" class="inline-alert error-alert settings-blocker" role="alert"><CircleX :size="18" /><span>部署环境未配置 CREWDOCK_MASTER_KEY，无法安全保存 SMTP 密码或启用邮件发送。</span></div>
        <div class="settings-form-grid">
          <label class="field settings-wide"><span>SMTP 主机</span><input v-model="emailForm.smtp_host" :required="emailForm.enabled" /></label>
          <label class="field"><span>端口</span><input v-model.number="emailForm.smtp_port" type="number" min="1" max="65535" required /></label>
          <label class="field"><span>传输安全</span><select v-model="emailForm.tls_mode"><option value="starttls">STARTTLS</option><option value="tls">TLS</option><option value="none">无加密（仅本机中继）</option></select></label>
          <label class="field"><span>SMTP 用户名</span><input v-model="emailForm.smtp_username" autocomplete="off" /></label>
          <label class="field"><span>SMTP 密码</span><input v-model="emailForm.smtp_password" type="password" autocomplete="new-password" :disabled="!health?.master_key_configured" :placeholder="emailSettings?.has_password ? '已保存，留空保持不变' : '尚未设置'" /></label>
          <label class="field"><span>发件邮箱</span><input v-model="emailForm.from_email" type="email" :required="emailForm.enabled" /></label>
          <label class="field"><span>发件人名称</span><input v-model="emailForm.from_name" /></label>
          <label class="field"><span>回复地址</span><input v-model="emailForm.reply_to" type="email" /></label>
          <label v-if="emailSettings?.has_password" class="check-row settings-wide"><input v-model="emailForm.clear_smtp_password" type="checkbox" /><span>清除已保存的 SMTP 密码</span></label>
        </div>
        <div class="settings-test-status">
          <CircleCheck v-if="emailSettings?.last_test_succeeded" :size="18" />
          <CircleX v-else-if="emailSettings?.last_test_succeeded === false" :size="18" />
          <Mail v-else :size="18" />
          <div><strong>{{ emailSettings?.last_test_succeeded ? '最近测试成功' : emailSettings?.last_test_succeeded === false ? '最近测试失败' : '尚未测试' }}</strong><span>{{ emailSettings?.last_test_error || formatTime(emailSettings?.last_test_at) }}</span></div>
          <button class="icon-button" type="button" title="刷新测试结果" aria-label="刷新测试结果" @click="refreshEmailStatus"><RefreshCw :size="16" /></button>
        </div>
        <footer class="settings-actions">
          <button class="primary-button" type="submit" :disabled="savingEmail || (emailForm.enabled && !health?.master_key_configured)"><Save :size="17" />{{ savingEmail ? '保存中...' : '保存邮件设置' }}</button>
          <button class="secondary-button" type="button" :disabled="testingEmail || !health?.master_key_configured || !emailSettings?.email_ready" @click="testEmail"><Send :size="17" />{{ testingEmail ? '正在排队...' : '发送测试邮件' }}</button>
        </footer>
      </form>

      <form v-else-if="activeTab === 'templates'" class="settings-content email-template-settings" @submit.prevent="saveTemplate">
        <header class="settings-section-head">
          <div><h2>邮件模板</h2><p>配置账号准入流程的邮件主题、HTML 内容和事件变量。</p></div>
          <span v-if="selectedTemplate?.updated_at" class="template-updated">更新于 {{ formatTime(selectedTemplate.updated_at) }}</span>
        </header>
        <label class="field template-event-select"><span>触发事件</span><select v-model="selectedTemplateKey"><option v-for="template in emailTemplates" :key="template.key" :value="template.key">{{ template.label }}</option></select></label>
        <div v-if="selectedTemplate" class="email-template-workspace">
          <section class="template-editor-pane">
            <label class="field"><span>邮件主题</span><input v-model="templateForm.subject" maxlength="300" /></label>
            <div class="template-variable-row">
              <span>可用变量</span>
              <button v-for="variable in selectedTemplate.variables" :key="variable" type="button" :title="`插入 {{${variable}}}`" @click="insertTemplateVariable(variable)">{{ variable }}</button>
            </div>
            <label class="field template-code-field"><span>HTML 模板</span><textarea ref="templateEditor" v-model="templateForm.html_body" rows="24" spellcheck="false" /></label>
          </section>
          <section class="template-preview-pane">
            <header><div><strong>实时预览</strong><span>{{ templatePreviewSubject }}</span></div><span>沙箱</span></header>
            <iframe :srcdoc="templatePreview" sandbox="" title="邮件模板实时预览" />
          </section>
        </div>
        <footer class="settings-actions template-actions">
          <button class="secondary-button" type="button" :disabled="savingTemplate" @click="restoreTemplateDefault"><RotateCcw :size="17" />恢复默认</button>
          <button class="primary-button" type="submit" :disabled="savingTemplate || !selectedTemplate"><LoaderCircle v-if="savingTemplate" class="spin" :size="17" /><Save v-else :size="17" />{{ savingTemplate ? '保存中...' : '保存模板' }}</button>
        </footer>
      </form>

      <form v-else-if="activeTab === 'auth'" class="settings-content" @submit.prevent="saveAuth">
        <header class="settings-section-head"><div><h2>登录与注册</h2><p>控制员工是否可以从登录页提交访问申请。</p></div></header>
        <div class="settings-choice-list">
          <label><input v-model="authForm.registration_mode" type="radio" value="closed" /><span><strong>关闭员工申请</strong><small>仅管理员可直接创建或邀请成员。</small></span></label>
          <label><input v-model="authForm.registration_mode" type="radio" value="application" :disabled="!emailSettings?.email_ready" /><span><strong>开放申请并审核</strong><small>{{ emailSettings?.email_ready ? '邮箱验证后进入账号管理待审核列表。' : '请先配置并启用邮件发送。' }}</small></span></label>
        </div>
        <label class="settings-policy-switch">
          <span><strong>允许所有邮箱</strong><small>任何邮箱通过验证码后都可提交申请，仍需管理员审核。</small></span>
          <span class="settings-toggle-control">
            <input v-model="authForm.allow_all_email_domains" type="checkbox" />
            <span aria-hidden="true" />
          </span>
        </label>
        <label class="field settings-domains">
          <span>允许的企业邮箱域名 <small>{{ authForm.allow_all_email_domains ? '当前由“允许所有邮箱”策略覆盖' : '每行一个；开放申请时至少填写一个' }}</small></span>
          <textarea v-model="authForm.allowed_domains" rows="6" placeholder="example.com" :disabled="authForm.allow_all_email_domains" />
        </label>
        <footer class="settings-actions"><button class="primary-button" type="submit" :disabled="savingAuth"><Save :size="17" />{{ savingAuth ? '保存中...' : '保存注册策略' }}</button></footer>
      </form>

      <div v-else class="settings-content update-settings">
        <header class="settings-section-head">
          <div><h2>版本与更新</h2><p>检查 GitHub 发布标签，并由主机更新服务执行备份、部署和健康验证；回滚备份仅保留最近 1 份。</p></div>
          <button class="icon-button" type="button" title="刷新更新状态" aria-label="刷新更新状态" @click="refreshUpdateStatus()"><RefreshCw :class="{ spin: updateRunning }" :size="16" /></button>
        </header>

        <div class="update-version-strip">
          <div><span>当前运行</span><strong>{{ updateStatus?.current_version || `v${health?.version || appVersion}` }}</strong><small>前端 v{{ appVersion }} · 后端 v{{ health?.version || '-' }}</small></div>
          <div><span>最新发布</span><strong>{{ updateStatus?.latest_version || '尚未检查' }}</strong><small>{{ updateStatus?.checked_at ? `检查于 ${formatTime(updateStatus.checked_at)}` : '来源：GitHub 稳定标签' }}</small></div>
          <div><span>更新状态</span><strong class="update-state" :class="updateStateTone"><i />{{ updateStateLabel }}</strong><small>{{ updateStatus?.message || '正在连接主机更新服务' }}</small></div>
        </div>

        <div v-if="updateRunning" class="update-progress" role="status" aria-live="polite">
          <div><span>{{ updateStatus?.message }}</span><strong>{{ updateStatus?.progress || 0 }}%</strong></div>
          <span><i :style="{ width: `${updateStatus?.progress || 0}%` }" /></span>
        </div>

        <div v-if="updateStatus && !updateStatus.updater_configured" class="inline-alert error-alert update-alert" role="alert">
          <CircleX :size="18" /><span>主机更新服务尚未安装。部署端需要安装 systemd 更新器并挂载控制目录。</span>
        </div>
        <div v-else-if="!versionsAligned" class="inline-alert update-alert" role="alert">
          <BadgeInfo :size="18" /><span>前端与后端版本不一致，建议刷新页面；若仍不一致，请重新部署同一发布标签。</span>
        </div>
        <div v-if="updateStatus?.error" class="inline-alert error-alert update-alert" role="alert">
          <CircleX :size="18" /><span>{{ updateStatus.error }}</span>
        </div>

        <dl class="settings-facts update-facts">
          <div><dt>自动检查</dt><dd>{{ updateStatus?.auto_check_enabled ? '每 6 小时' : '未启用' }}</dd></div>
          <div><dt>前端提交</dt><dd>{{ gitSha || '-' }}</dd></div>
          <div><dt>后端提交</dt><dd>{{ health?.git_sha || '-' }}</dd></div>
          <div><dt>前端构建时间</dt><dd>{{ formatTime(buildTime) }}</dd></div>
          <div><dt>后端构建时间</dt><dd>{{ formatTime(health?.build_time) }}</dd></div>
          <div><dt>最近完成</dt><dd>{{ formatTime(updateStatus?.finished_at) }}</dd></div>
        </dl>

        <div v-if="updateConfirmation && updateStatus?.latest_version" class="update-confirmation" role="alert">
          <ShieldCheck :size="21" />
          <div><strong>更新到 {{ updateStatus.latest_version }}</strong><span>更新前会创建新备份并清理更早的发布备份；部署期间页面可能短暂断开，健康检查失败将恢复旧版本。</span></div>
          <button class="secondary-button" type="button" :disabled="requestingUpdate" @click="updateConfirmation = false">取消</button>
          <button class="primary-button" type="button" :disabled="requestingUpdate" @click="applyUpdate"><LoaderCircle v-if="requestingUpdate" class="spin" :size="17" /><CloudDownload v-else :size="17" />确认更新</button>
        </div>

        <footer v-else class="settings-actions update-actions">
          <button class="secondary-button" type="button" :disabled="checkingUpdates || !updateStatus?.can_check" @click="checkForUpdates"><LoaderCircle v-if="checkingUpdates || updateStatus?.state === 'checking'" class="spin" :size="17" /><Search v-else :size="17" />{{ updateStatus?.state === 'checking' ? '正在检查' : '检查更新' }}</button>
          <button class="primary-button" type="button" :disabled="!updateStatus?.can_apply" @click="updateConfirmation = true"><CloudDownload :size="17" />{{ updateStatus?.update_available ? `更新到 ${updateStatus.latest_version}` : '暂无可用更新' }}</button>
        </footer>
      </div>
    </section>
  </WorkbenchShell>
</template>
