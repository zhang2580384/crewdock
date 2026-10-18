<script setup lang="ts">
import { ShieldCheck } from '@lucide/vue';

withDefaults(defineProps<{
  serviceState?: 'checking' | 'online' | 'offline' | 'hidden';
  tall?: boolean;
}>(), {
  serviceState: 'hidden',
  tall: false,
});

const appVersion = __APP_VERSION__;
const sourceUrl = `https://github.com/zhang2580384/crewdock/tree/v${appVersion}`;
</script>

<template>
  <div class="login-page">
    <main class="login-shell" :class="{ 'auth-shell-tall': tall }">
      <section class="login-identity">
        <div class="login-brand">
          <div class="brand-mark"><ShieldCheck :size="23" /></div>
          <div>
            <strong>CrewDock</strong>
            <span>共舟工作台</span>
          </div>
        </div>

        <div class="login-statement">
          <span class="eyebrow">PRIVATE WORKSPACE</span>
          <h1>工作，从一个清晰的<br /><span class="login-title-line">入口开始。</span></h1>
          <p>常用站点、项目资料与内部协作空间。</p>
        </div>

        <div class="login-identity-footer">
          <a :href="sourceUrl" target="_blank" rel="noreferrer" title="AGPL-3.0-or-later 源代码">v{{ appVersion }} · 源码</a>
          <span v-if="serviceState !== 'hidden'" class="service-state" :class="serviceState">
            <i aria-hidden="true" />
            {{ serviceState === 'checking' ? '检测服务' : serviceState === 'online' ? '服务正常' : '服务异常' }}
          </span>
          <span v-else>内部访问</span>
        </div>
      </section>

      <section class="login-panel">
        <slot />
        <span class="auth-mobile-version">CrewDock v{{ appVersion }} · <a :href="sourceUrl" target="_blank" rel="noreferrer">源码</a></span>
      </section>
    </main>
  </div>
</template>
