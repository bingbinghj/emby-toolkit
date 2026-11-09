<template>
  <div class="login-root" :data-login-visual-profile="glassEnabled ? 'glass' : 'plain'">
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
      <VCard class="auth-card login-card pa-7 pa-sm-9" :class="{ 'glass-effect': glassEnabled }" max-width="24rem" width="100%" flat>
        <div class="login-card__surface" aria-hidden="true" />
        <div class="login-head">
          <img :src="logo" alt="Emby Toolkit" class="login-logo" />
          <h1 class="login-title">Emby Toolkit</h1>
          <p class="login-subtitle">请使用 Emby 账号登录</p>
        </div>

        <VCardText class="login-body pa-0">
          <VForm ref="formRef" class="login-form" autocomplete="on" @submit.prevent="handleLogin">
            <VTextField
              v-model="credentials.username"
              label="用户名"
              placeholder="请输入用户名"
              name="username"
              autocomplete="username"
              autocapitalize="none"
              spellcheck="false"
              enterkeyhint="next"
              prepend-inner-icon="mdi-account-outline"
              :rules="usernameRules"
              :readonly="loading"
              :mobile-layout="false"
              class="mb-4"
              autofocus
            />
            <VTextField
              v-model="credentials.password"
              label="密码"
              placeholder="请输入密码"
              name="password"
              autocomplete="current-password"
              :type="isPasswordVisible ? 'text' : 'password'"
              prepend-inner-icon="mdi-lock-outline"
              :readonly="loading"
              :mobile-layout="false"
              class="mb-5"
            >
              <template #append-inner>
                <button type="button" class="password-toggle" :aria-label="isPasswordVisible ? '隐藏密码' : '显示密码'" @click="isPasswordVisible = !isPasswordVisible">
                  <VIcon :icon="isPasswordVisible ? 'mdi-eye-off-outline' : 'mdi-eye-outline'" size="20" />
                </button>
              </template>
            </VTextField>
            <VBtn block type="submit" prepend-icon="mdi-login" :loading="loading" size="large" class="login-submit">登录</VBtn>
            <VAlert v-if="errorMessage" type="error" variant="tonal" class="mt-4" closable @click:close="errorMessage = ''">{{ errorMessage }}</VAlert>
            <div class="footer-links">
              <VBtn variant="text" size="small" @click="showRecoveryModal = true">无法登录？重置连接配置</VBtn>
            </div>
          </VForm>
        </VCardText>
        <div class="login-foot">Emby Toolkit · 媒体处理与管理</div>
      </VCard>
    </div>

    <VDialog v-model="showRecoveryModal" max-width="480" :persistent="recoveryLoading">
      <VCard title="重置连接配置">
        <VCardText>
          <template v-if="!tokenSent">
            <p class="mb-4">如果 Emby 地址或 API Key 失效导致无法登录，可以生成一次性安全令牌进入设置页。</p>
            <VAlert type="warning" variant="tonal">令牌只会写入 Docker 日志，有效期 5 分钟。</VAlert>
          </template>
          <template v-else>
            <p class="mb-4">安全令牌已写入 Docker 日志，请填写后验证。</p>
            <VTextField v-model="recoveryToken" label="安全令牌" placeholder="请输入 6 位安全令牌" maxlength="6" inputmode="text" autocapitalize="characters" spellcheck="false" autocomplete="one-time-code" :readonly="recoveryLoading" :mobile-layout="false" @keyup.enter="verifyToken" />
          </template>
        </VCardText>
        <VCardActions class="px-6 pb-6 flex-wrap">
          <VSpacer />
          <VBtn variant="text" :disabled="recoveryLoading" @click="showRecoveryModal = false">取消</VBtn>
          <VBtn v-if="!tokenSent" color="error" variant="flat" :loading="recoveryLoading" @click="requestToken">生成令牌</VBtn>
          <VBtn v-else variant="flat" prepend-icon="mdi-cog-outline" :loading="recoveryLoading" @click="verifyToken">验证并进入设置</VBtn>
        </VCardActions>
      </VCard>
    </VDialog>
  </div>
</template>

<script setup>
import { ref, computed, onBeforeUnmount } from 'vue';
import { useRouter } from 'vue-router';
import { useMessage } from '@/composables/useFeedback';
import { useThemeCustomizer } from '@/composables/useThemeCustomizer';
import { useAuthStore } from '../stores/auth';
import logo from '../assets/logo.png';
import axios from 'axios';

const router = useRouter();
const message = useMessage();
const authStore = useAuthStore();
const { settings } = useThemeCustomizer();
const glassEnabled = computed(() => settings.value.glassEnabled);
const formRef = ref(null);
const credentials = ref({ username: '', password: '' });
const loading = ref(false);
const isPasswordVisible = ref(false);
const errorMessage = ref('');
const showRecoveryModal = ref(false);
const tokenSent = ref(false);
const recoveryToken = ref('');
const recoveryLoading = ref(false);
const usernameRules = [value => !!String(value || '').trim() || '请输入用户名'];
let isPageActive = true;
onBeforeUnmount(() => { isPageActive = false; });

async function handleLogin() {
  if (!isPageActive || loading.value) return;
  loading.value = true;
  errorMessage.value = '';
  try {
    const validation = await formRef.value?.validate();
    if (!isPageActive || !validation?.valid) return;
    await authStore.login({ ...credentials.value });
    if (!isPageActive) return;
    message.success('登录成功');
    router.push({ name: 'UserCenter' });
  } catch (error) {
    if (!isPageActive) return;
    if (error.response?.status === 428) {
      message.warning('系统尚未配置，请先完成设置');
      router.push({ name: 'Setup' });
    } else {
      errorMessage.value = error.response?.data?.message || error.response?.data?.error || '登录失败';
    }
  } finally {
    loading.value = false;
  }
}

async function requestToken() {
  if (!isPageActive || recoveryLoading.value) return;
  recoveryLoading.value = true;
  try {
    await axios.post('/api/auth/request_recovery');
    if (!isPageActive) return;
    tokenSent.value = true;
    message.info('令牌已生成，请查看 Docker 日志');
  } catch (error) {
    if (!isPageActive) return;
    message.error(error.response?.data?.message || error.response?.data?.error || '请求失败');
  } finally {
    recoveryLoading.value = false;
  }
}

async function verifyToken() {
  if (!isPageActive || recoveryLoading.value) return;
  if (!recoveryToken.value) {
    message.warning('请输入安全令牌');
    return;
  }
  recoveryLoading.value = true;
  try {
    await axios.post('/api/auth/verify_recovery', { token: recoveryToken.value });
    if (!isPageActive) return;
    message.success('验证成功，正在进入设置页');
    showRecoveryModal.value = false;
    router.push({ name: 'Setup' });
  } catch (error) {
    if (!isPageActive) return;
    message.error(error.response?.data?.message || error.response?.data?.error || '令牌无效或已过期');
  } finally {
    recoveryLoading.value = false;
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
.login-submit { font-weight:600; }
.footer-links { margin-top:16px; text-align:center; }
.login-foot { margin-top:24px; text-align:center; color:rgba(var(--v-theme-on-surface),var(--v-disabled-opacity,.4)); font-size:.75rem; }
@media (max-height:650px) { .login-root { align-items:flex-start; } }
</style>
