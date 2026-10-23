<template>
  <!-- 使用 n-layout 自动适配主题背景色 -->
  <n-layout class="login-layout">
    <div class="login-container">
      <n-card class="dashboard-card login-card" :bordered="false" size="large">

        <div class="login-header">
          <img src="../assets/logo.png" alt="Logo" class="login-logo" />
          <h2 class="login-title">Emby Toolkit</h2>
          <p class="login-subtitle">请使用 Emby 账号登录</p>
        </div>

        <n-form @submit.prevent="handleLogin" size="large">
          <n-form-item-row label="用户名">
            <n-input
              v-model:value="credentials.username"
              placeholder="请输入用户名"
              autocomplete="username"
              :input-props="{ name: 'username', autocapitalize: 'none', spellcheck: false }"
            >
              <template #prefix>
                <n-icon :component="PersonOutline" />
              </template>
            </n-input>
          </n-form-item-row>

          <n-form-item-row label="密码">
            <n-input
              type="password"
              show-password-on="click"
              v-model:value="credentials.password"
              placeholder="请输入密码"
              autocomplete="current-password"
              :input-props="{ name: 'password' }"
            >
              <template #prefix>
                <n-icon :component="LockClosedOutline" />
              </template>
            </n-input>
          </n-form-item-row>

          <n-button type="primary" attr-type="submit" block :loading="loading" size="large" class="login-btn"><template #icon><UiIcon name="key" /></template>
            登录
          </n-button>

          <div class="footer-links">
            <n-button text type="primary" size="small" @click="showRecoveryModal = true"><template #icon><UiIcon name="key" /></template>
              无法登录？重置连接配置
            </n-button>
          </div>
        </n-form>
      </n-card>
    </div>

    <n-modal v-model:show="showRecoveryModal" preset="dialog" title="重置连接配置">
      <div v-if="!tokenSent">
        <p>如果 Emby 地址或 API Key 失效导致无法登录，可以生成一次性安全令牌进入设置页。</p>
        <p class="recovery-warning">令牌只会写入 Docker 日志，有效期 5 分钟。</p>
      </div>

      <div v-else>
        <p>安全令牌已写入 Docker 日志。</p>
        <n-input v-model:value="recoveryToken" placeholder="请输入 6 位安全令牌" />
      </div>

      <template #action>
        <n-button @click="showRecoveryModal = false"><template #icon><UiIcon name="close" /></template>取消</n-button>
        <n-button v-if="!tokenSent" type="error" @click="requestToken" :loading="recoveryLoading"><template #icon><UiIcon name="key" /></template>
          生成令牌
        </n-button>
        <n-button v-else type="primary" @click="verifyToken" :loading="recoveryLoading"><template #icon><UiIcon name="settings" /></template>
          验证并进入设置
        </n-button>
      </template>
    </n-modal>
  </n-layout>
</template>

<script setup>
import { ref } from 'vue';
import { useRouter } from 'vue-router';
import {
  NLayout, NCard, NForm, NFormItemRow, NInput, NButton,
  useMessage, NModal, NIcon
} from 'naive-ui';
import { PersonOutline, LockClosedOutline } from '@vicons/ionicons5';
import { useAuthStore } from '../stores/auth';
import axios from 'axios';

const router = useRouter();
const message = useMessage();
const authStore = useAuthStore();

const credentials = ref({ username: '', password: '' });
const loading = ref(false);
const showRecoveryModal = ref(false);
const tokenSent = ref(false);
const recoveryToken = ref('');
const recoveryLoading = ref(false);

async function handleLogin() {
  if (loading.value) return;
  if (!credentials.value.username) {
    message.warning('请输入用户名');
    return;
  }
  loading.value = true;
  try {
    await authStore.login(credentials.value);
    message.success('登录成功');
    router.push({ name: 'UserCenter' });
  } catch (error) {
    if (error.response && error.response.status === 428) {
      message.warning('系统尚未配置，请先完成设置');
      router.push({ name: 'Setup' });
    } else {
      const msg = error.response?.data?.message || '登录失败';
      message.error(msg);
    }
  } finally {
    loading.value = false;
  }
}

async function requestToken() {
  recoveryLoading.value = true;
  try {
    await axios.post('/api/auth/request_recovery');
    tokenSent.value = true;
    message.info('令牌已生成，请查看 Docker 日志');
  } catch (e) {
    message.error(e.response?.data?.message || '请求失败');
  } finally {
    recoveryLoading.value = false;
  }
}

async function verifyToken() {
  if (!recoveryToken.value) {
    message.warning('请输入安全令牌');
    return;
  }
  recoveryLoading.value = true;
  try {
    await axios.post('/api/auth/verify_recovery', { token: recoveryToken.value });
    message.success('验证成功，正在进入设置页');
    showRecoveryModal.value = false;
    router.push({ name: 'Setup' });
  } catch (e) {
    message.error(e.response?.data?.message || '令牌无效或已过期');
  } finally {
    recoveryLoading.value = false;
  }
}

</script>

<style scoped>
/* 使用 n-layout 撑满全屏，背景色会自动跟随主题 */
.login-layout {
  height:100dvh;
  width:100%;
}

.login-container {
  display: flex;
  justify-content: center;
  align-items: center;
  min-height:100dvh;
  width: 100%;
  padding:32px 20px;
}

.login-card {
  width: 100%;
  max-width: 420px; /* 经典的宽度 */
  border-radius: 12px; /* 圆角 */
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.08); /* 柔和的阴影 */
  height: auto !important;
  min-height: auto !important;

  /* 如果 dashboard-card 里有 flex 属性，可能也需要重置 */
  flex: none !important;
}

/* 暗色模式下的阴影调整 */
:deep(.n-card.n-card--bordered) {
  border: 1px solid rgba(255, 255, 255, 0.09);
}

.login-header {
  text-align: center;
  margin-bottom: 30px;
}

.login-logo {
  height: 60px;
  margin-bottom: 10px;
}

.login-title {
  font-size: 24px;
  font-weight: 600;
  margin: 0 0 8px 0;
  /* 字体颜色会自动适配主题 */
}

.login-subtitle {
  font-size: 14px;
  color: var(--ui-text-muted);
  margin: 0;
}

.login-btn {
  margin-top: 10px;
  font-weight: bold;
}

.footer-links {
  margin-top: 22px;
  text-align: center;
}

.recovery-warning {
  color:var(--ui-text);
  font-weight: 600;
  margin-top: 10px;
}
@media (max-height:650px) { .login-container { align-items:flex-start; } }
</style>
