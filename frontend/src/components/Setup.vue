<template>
  <div class="login-root setup-root" :data-login-visual-profile="glassEnabled ? 'glass' : 'plain'">
    <svg v-if="glassEnabled" class="login-glass-filter-defs" aria-hidden="true">
      <defs>
        <filter id="login-glass-static-refraction" x="-12%" y="-12%" width="124%" height="124%">
          <feTurbulence type="fractalNoise" baseFrequency="0.008 0.014" numOctaves="2" seed="8" result="noise" />
          <feGaussianBlur in="noise" stdDeviation="0.35" result="softNoise" />
          <feDisplacementMap in="SourceGraphic" in2="softNoise" scale="11" xChannelSelector="R" yChannelSelector="G" />
        </filter>
      </defs>
    </svg>

    <div class="auth-wrapper d-flex align-center justify-center">
      <VCard class="auth-card login-card setup-card pa-7 pa-sm-9" :class="{ 'glass-effect': glassEnabled }" max-width="36rem" width="100%" flat>
        <div class="login-card__surface" aria-hidden="true" />
        <div class="login-head">
          <img :src="logo" alt="Emby Toolkit" class="login-logo" />
          <h1 class="login-title">Emby Toolkit</h1>
          <p class="login-subtitle">初始化配置向导</p>
        </div>

        <VCardText class="pa-0">
          <VAlert type="info" variant="tonal" class="mb-6">配置 Emby 服务器连接信息，测试通过后即可登录。</VAlert>
          <VForm ref="formRef" @submit.prevent="handleSave">
            <VTextField
              v-model="formModel.url"
              label="服务器地址 (URL)"
              placeholder="例如: http://192.168.1.10:8096"
              name="emby-server-url"
              inputmode="url"
              autocapitalize="none"
              spellcheck="false"
              prepend-inner-icon="mdi-server-network-outline"
              :rules="rules.url"
              :readonly="loading"
              :mobile-layout="false"
              class="mb-4"
              autofocus
            />
            <VTextField
              v-model="formModel.api_key"
              label="API 密钥 (API Key)"
              name="emby-api-key"
              :type="isApiKeyVisible ? 'text' : 'password'"
              placeholder="请输入 Emby API 密钥"
              autocomplete="off"
              prepend-inner-icon="mdi-key-outline"
              hint="在 Emby 控制台 → 高级 → API 密钥中生成"
              persistent-hint
              :rules="rules.api_key"
              :readonly="loading"
              :mobile-layout="false"
              class="mb-6"
            >
              <template #append-inner>
                <button type="button" class="password-toggle" :aria-label="isApiKeyVisible ? '隐藏 API 密钥' : '显示 API 密钥'" @click="isApiKeyVisible = !isApiKeyVisible">
                  <VIcon :icon="isApiKeyVisible ? 'mdi-eye-off-outline' : 'mdi-eye-outline'" size="20" />
                </button>
              </template>
            </VTextField>
            <VBtn block type="submit" prepend-icon="mdi-check-network-outline" :loading="loading" size="large">测试并保存</VBtn>
            <VAlert v-if="errorMessage" type="error" variant="tonal" class="mt-4" closable @click:close="errorMessage = ''">{{ errorMessage }}</VAlert>
          </VForm>
        </VCardText>
        <div class="login-foot">Emby Toolkit · 媒体处理与管理</div>
      </VCard>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, onBeforeUnmount } from 'vue';
import { useRouter } from 'vue-router';
import { useMessage } from '@/composables/useFeedback';
import { useThemeCustomizer } from '@/composables/useThemeCustomizer';
import logo from '../assets/logo.png';
import axios from 'axios';

const router = useRouter();
const message = useMessage();
const { settings } = useThemeCustomizer();
const glassEnabled = computed(() => settings.value.glassEnabled);
const loading = ref(false);
const formRef = ref(null);
const isApiKeyVisible = ref(false);
const errorMessage = ref('');
const formModel = ref({ url: '', api_key: '' });
const rules = {
  url: [value => !!String(value || '').trim() || '请输入服务器地址'],
  api_key: [value => !!String(value || '').trim() || '请输入 API 密钥']
};
let redirectTimer;
let isPageActive = true;
onBeforeUnmount(() => {
  isPageActive = false;
  clearTimeout(redirectTimer);
});

async function handleSave() {
  if (!isPageActive || loading.value) return;
  loading.value = true;
  errorMessage.value = '';
  try {
    const validation = await formRef.value?.validate();
    if (!isPageActive || !validation?.valid) return;
    const response = await axios.post('/api/auth/setup', { ...formModel.value });
    if (!isPageActive) return;
    if (response.data.status === 'ok') {
      message.success('配置成功！即将跳转登录页...');
      redirectTimer = setTimeout(() => {
        if (isPageActive) router.push({ name: 'Login' });
      }, 1500);
    } else {
      errorMessage.value = response.data.message || response.data.error || '连接测试失败，请检查 URL 和密钥';
    }
  } catch (error) {
    if (!isPageActive) return;
    errorMessage.value = error.response?.data?.message || error.response?.data?.error || '连接测试失败，请检查 URL 和密钥';
  } finally {
    loading.value = false;
  }
}
</script>

<style scoped>
/* Auth surface adapted from MoviePilot-Frontend v3's login page. */
.login-root { display:flex; align-items:center; min-height:100dvh; width:100%; padding:32px 0; }
.auth-wrapper { position:relative; z-index:2; width:100%; padding-inline:16px; }
.login-glass-filter-defs { position:absolute; width:0; height:0; overflow:hidden; pointer-events:none; }
.login-card { position:relative; overflow:hidden; border-radius:var(--app-surface-radius,20px); box-shadow:0 20px 54px rgba(var(--app-shadow-rgb,0,0,0),.12); }
.login-card > :not(.login-card__surface) { position:relative; z-index:2; }
.glass-effect { backdrop-filter:none !important; background:transparent !important; }
.login-card__surface { position:absolute; z-index:0; inset:0; overflow:hidden; border-radius:inherit; pointer-events:none; transform:translateZ(0); }
.login-root[data-login-visual-profile='glass'] .login-card__surface { background:linear-gradient(145deg,rgba(255,255,255,.08),transparent 34%),linear-gradient(rgba(7,14,25,.04),rgba(7,14,25,.1)); }
.login-head { display:flex; flex-direction:column; align-items:center; text-align:center; margin-bottom:28px; }
.login-logo { height:64px; width:64px; object-fit:contain; margin-bottom:16px; }
.login-title { font-size:1.85rem; font-weight:600; margin:0 0 8px; }
.login-subtitle { margin:0; color:rgba(var(--v-theme-on-surface),var(--v-medium-emphasis-opacity,.6)); font-size:.875rem; }
.password-toggle { display:flex; align-items:center; color:inherit; padding:4px; border-radius:50%; cursor:pointer; }
.password-toggle:focus-visible { outline:2px solid rgb(var(--v-theme-primary)); outline-offset:2px; }
.login-foot { margin-top:24px; text-align:center; color:rgba(var(--v-theme-on-surface),var(--v-disabled-opacity,.4)); font-size:.75rem; }
@media (max-height:750px) { .login-root { align-items:flex-start; } }
</style>
