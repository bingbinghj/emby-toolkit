<template>
  <div class="mp-page user-center-page">
    <n-page-header class="mp-page-header" title="用户中心" :subtitle="`欢迎回来，${accountInfo?.name || authStore.username}`" />

    <div class="account-grid">
    <n-card :bordered="false" class="dashboard-card account-card">
      <template #header>
        <span class="card-title">账户资料</span>
      </template>
      <template #header-extra><n-spin v-if="loading" size="small" /></template>

      <div v-if="accountInfo" class="profile-layout">
        <div class="profile-avatar-section">
          <n-tooltip trigger="hover" placement="right">
            <template #trigger>
              <button type="button" class="avatar-wrapper" aria-label="更换头像" :disabled="isUploadingAvatar || isDeletingAvatar" @click="triggerFileUpload">
                <n-avatar :size="isMobile ? 72 : 100" :src="avatarUrl" object-fit="cover">
                  <span v-if="!avatarUrl">
                    {{ authStore.username ? authStore.username.charAt(0).toUpperCase() : 'U' }}
                  </span>
                </n-avatar>
                <div class="avatar-overlay">
                  <n-icon size="24" color="white"><CloudUploadOutline /></n-icon>
                </div>
              </button>
            </template>
            点击更换头像
          </n-tooltip>
          <input type="file" ref="fileInput" hidden accept="image/png, image/jpeg, image/jpg" @change="handleAvatarChange" />
          <div class="profile-copy">
            <div class="username-text">{{ accountInfo?.name || authStore.username }}</div>
            <div class="profile-hint">{{ isUploadingAvatar ? '正在上传头像…' : '点击头像更换 · JPG / PNG，最大 5 MB' }}</div>
            <n-button v-if="avatarUrl" size="small" type="error" secondary :loading="isDeletingAvatar" :disabled="isUploadingAvatar" @click="deleteAvatar"><template #icon><UiIcon name="delete" /></template>删除头像</n-button>
          </div>
        </div>

        <n-descriptions label-placement="left" bordered :column="1" size="medium" class="account-descriptions">
          <n-descriptions-item label="账户状态">
            <n-tag :type="statusType" size="small">{{ statusText }}</n-tag>
          </n-descriptions-item>
          <n-descriptions-item label="注册时间">
            {{ formatDateTime(accountInfo.registration_date) }}
          </n-descriptions-item>
          <n-descriptions-item label="到期时间">
            {{ accountInfo.expiration_date ? formatDateTime(accountInfo.expiration_date) : '永久有效' }}
          </n-descriptions-item>
          <n-descriptions-item label="账户等级">
            <strong v-if="authStore.isAdmin">管理员</strong>
            <strong v-else>普通用户</strong>
          </n-descriptions-item>
          <n-descriptions-item label="等级说明">
            <span v-if="authStore.isAdmin">拥有系统所有管理权限</span>
            <span v-else>可查看账户详情并接收通知</span>
          </n-descriptions-item>
        </n-descriptions>
      </div>
      <n-empty v-else-if="!loading" description="未获取到账户信息" />
    </n-card>
    <n-card :bordered="false" class="dashboard-card notification-card">
      <template #header><span class="card-title">通知设置</span></template>
      <p class="notification-intro">绑定 Telegram，接收入库和任务通知。</p>
      <n-spin :show="loadingConfig">
      <template v-if="configModel">
      <n-form-item label="Telegram Bot Token" label-placement="top">
        <n-input v-model:value="configModel.telegram_bot_token" type="password" show-password-on="click" placeholder="从 @BotFather 获取" />
      </n-form-item>
      <n-form-item label="全局通知频道" label-placement="top">
        <n-input v-model:value="configModel.telegram_channel_id" placeholder="频道链接、@频道名或 Chat ID" />
      </n-form-item>
      <n-form-item label="通知类型" label-placement="top">
        <n-checkbox-group v-model:value="configModel.telegram_notify_types"><n-space wrap>
          <n-checkbox value="library_new">入库通知</n-checkbox>
          <n-checkbox value="intercept_notify">拦截通知</n-checkbox>
          <n-checkbox value="recognize_fail">识别失败</n-checkbox>
        </n-space></n-checkbox-group>
      </n-form-item>
      </template>
      <n-form-item label="Telegram 用户ID（私聊ID）" label-placement="top">
        <n-input v-model:value="telegramChatId" placeholder="输入 Telegram 用户ID，例如 123456789" @keyup.enter="saveChatId" :disabled="loading || isSavingChatId" />
        <template #feedback>先向 Telegram 机器人发送 /start，再填写您的 Telegram 用户ID（私聊ID），用于接收个人通知。</template>
      </n-form-item>
      <div class="notification-actions">
        <n-button :loading="isFetchingBotLink" @click="openBotChat"><template #icon><UiIcon name="link" /></template>打开机器人</n-button>
        <n-button :loading="isTestingTelegram" :disabled="!configModel?.telegram_bot_token || (!telegramChatId && !configModel?.telegram_channel_id)" @click="testTelegram"><template #icon><UiIcon name="send" /></template>发送测试</n-button>
        <n-button type="primary" :loading="isSavingChatId" :disabled="loading || loadingConfig" @click="saveChatId"><template #icon><UiIcon name="save" /></template>保存通知设置</n-button>
      </div>
      <div v-if="accountInfo?.telegram_channel_id" class="global-notification-link">
        <span>全局通知频道</span>
        <n-button text type="primary" tag="a" :href="globalChannelLink" target="_blank" rel="noopener noreferrer"><template #icon><UiIcon name="link" /></template>加入频道 / 群组</n-button>
      </div>
      </n-spin>
    </n-card>
    </div>
  </div>
</template>

<script setup>
import { ref, onMounted, onUnmounted, computed } from 'vue';
import axios from 'axios';
import { useAuthStore } from '../stores/auth';
import { useConfig } from '../composables/useConfig';
import {
  NPageHeader, NCard, NDescriptions, NDescriptionsItem, NTag, NEmpty,
  NFormItem, NInput, NButton, useMessage, NAvatar, NIcon, NTooltip, NSpin, NCheckbox, NCheckboxGroup, NSpace
} from 'naive-ui';
import { CloudUploadOutline } from '@vicons/ionicons5';

const authStore = useAuthStore();
const message = useMessage();
const { configModel, loadingConfig, handleSaveConfig, configError } = useConfig();
const isTestingTelegram = ref(false);

const loading = ref(true);
const accountInfo = computed(() => authStore.accountInfo);
const telegramChatId = ref('');
const isSavingChatId = ref(false);
const isUploadingAvatar = ref(false);
const isDeletingAvatar = ref(false);
const isFetchingBotLink = ref(false);
const fileInput = ref(null);
const isMobile = ref(false);

const checkMobile = () => {
  isMobile.value = window.innerWidth < 768;
};

const avatarUrl = computed(() => authStore.avatarUrl);

const statusMap = {
  active: { text: '正常', type: 'success' },
  pending: { text: '待审批', type: 'warning' },
  expired: { text: '已过期', type: 'error' },
  disabled: { text: '已禁用', type: 'error' },
};

const statusText = computed(() => statusMap[accountInfo.value?.status]?.text || '未知');
const statusType = computed(() => statusMap[accountInfo.value?.status]?.type || 'default');

const globalChannelLink = computed(() => {
  const channelId = accountInfo.value?.telegram_channel_id?.trim();
  if (!channelId) return '#';
  if (channelId.startsWith('https://t.me/')) return channelId;
  if (channelId.startsWith('@')) return `https://t.me/${channelId.substring(1)}`;
  return `https://t.me/${channelId}`;
});

const formatDateTime = (value) => {
  return value ? new Date(value).toLocaleDateString('zh-CN') : '—';
};

const triggerFileUpload = () => {
  fileInput.value?.click();
};

const handleAvatarChange = async (event) => {
  const file = event.target.files[0];
  if (!file || isUploadingAvatar.value || isDeletingAvatar.value) return;
  if (!['image/jpeg', 'image/png', 'image/jpg'].includes(file.type)) {
    message.error('只支持 JPG/PNG 格式的图片');
    event.target.value = '';
    return;
  }
  if (file.size > 5 * 1024 * 1024) {
    message.error('图片大小不能超过 5 MB');
    event.target.value = '';
    return;
  }

  const formData = new FormData();
  formData.append('avatar', file);
  const loadingMsg = message.loading('正在上传头像...', { duration: 0 });
  isUploadingAvatar.value = true;

  try {
    const res = await axios.post('/api/portal/upload-avatar', formData, {
      headers: { 'Content-Type': 'multipart/form-data' }
    });
    message.success('头像更新成功');
    if (accountInfo.value && res.data.new_tag) {
      accountInfo.value.profile_image_tag = res.data.new_tag;
    }
  } catch (error) {
    message.error(error.response?.data?.message || '上传失败');
  } finally {
    loadingMsg.destroy();
    isUploadingAvatar.value = false;
    event.target.value = '';
  }
};

const deleteAvatar = async () => {
  if (isUploadingAvatar.value || isDeletingAvatar.value) return;
  isDeletingAvatar.value = true;
  try {
    await axios.delete('/api/portal/upload-avatar');
    if (accountInfo.value) accountInfo.value.profile_image_tag = null;
    message.success('头像已删除');
  } catch (error) {
    message.error(error.response?.data?.message || error.response?.data?.error || '删除头像失败');
  } finally {
    isDeletingAvatar.value = false;
  }
};

const saveChatId = async () => {
  if (loading.value || isSavingChatId.value) return;
  isSavingChatId.value = true;
  try {
    const config = configModel.value;
    if (!config || !await handleSaveConfig({ telegram_bot_token: config.telegram_bot_token, telegram_channel_id: config.telegram_channel_id, telegram_notify_types: config.telegram_notify_types })) {
      throw new Error(configError.value || '通知配置保存失败');
    }
    const response = await axios.post('/api/portal/telegram-chat-id', {
      chat_id: telegramChatId.value
    });
    if (accountInfo.value) {
      accountInfo.value.telegram_chat_id = telegramChatId.value;
      accountInfo.value.telegram_channel_id = configModel.value.telegram_channel_id;
    }
    message.success(response.data.message || '保存成功');
  } catch (error) {
    message.error(error.response?.data?.message || error.message || '保存失败');
  } finally {
    isSavingChatId.value = false;
  }
};

const testTelegram = async () => {
  isTestingTelegram.value = true;
  try {
    const { data } = await axios.post('/api/telegram/test', { token: configModel.value.telegram_bot_token, chat_id: telegramChatId.value.trim() || configModel.value.telegram_channel_id });
    if (data.success === true) message.success(data.message || '测试消息已发送');
    else message.error(data.message || '测试消息发送失败');
  } catch (error) {
    message.error(error.response?.data?.message || '测试消息发送失败');
  } finally {
    isTestingTelegram.value = false;
  }
};

const openBotChat = async () => {
  isFetchingBotLink.value = true;
  try {
    const response = await axios.get('/api/portal/telegram-bot-info');
    if (response.data.bot_username) {
      window.open(`https://t.me/${response.data.bot_username}`, '_blank');
    } else {
      message.error(response.data.error || '未能获取到机器人信息');
    }
  } catch (error) {
    message.error('请求机器人信息失败');
  } finally {
    isFetchingBotLink.value = false;
  }
};

onMounted(async () => {
  checkMobile();
  window.addEventListener('resize', checkMobile);
  try {
    await authStore.fetchAccountInfo();
    telegramChatId.value = accountInfo.value?.telegram_chat_id || '';
  } catch (error) {
    message.error('加载账户信息失败');
  } finally {
    loading.value = false;
  }
});
onUnmounted(() => window.removeEventListener('resize', checkMobile));
</script>

<style scoped>
.account-grid { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); align-items:stretch; gap:24px; }
.account-grid > .n-card { height:100%; }
@container (max-width:899px) { .account-grid { grid-template-columns:minmax(0,1fr); } }
.account-card, .notification-card { min-width:0; }

.profile-layout {
  display: flex;
  flex-direction: column;
  gap: 24px;
}

.profile-avatar-section {
  width: 100%;
  display: flex;
  flex-direction: row;
  align-items: center;
  gap:20px;
  padding-bottom:24px;
  border-bottom:1px solid var(--ui-border);
}

.username-text {
  font-weight:600;
  font-size:20px;
  overflow-wrap:anywhere;
}
.profile-copy { min-width:0; }
.profile-hint { margin-top:8px; color:var(--ui-text-muted); font-size:12px; line-height:1.65; }

.avatar-wrapper {
  position: relative;
  border-radius: 50%;
  overflow: hidden;
  transition: transform 0.2s;
  box-shadow: 0 4px 12px rgba(0,0,0,0.1);
  display: flex;
  justify-content: center;
  align-items: center;
  line-height: 0;
  width: fit-content;
  flex-shrink:0;
  padding:0;
  border:0;
  background:var(--accent-glow-color);
  cursor:pointer;
}

.avatar-wrapper:hover {
  transform: scale(1.05);
}

.avatar-overlay {
  position: absolute;
  inset: 0;
  background-color: rgba(0, 0, 0, 0.4);
  display: flex;
  justify-content: center;
  align-items: center;
  opacity: 0;
  transition: opacity 0.2s;
  border-radius: 50%;
}

.avatar-wrapper:hover .avatar-overlay, .avatar-wrapper:focus-visible .avatar-overlay {
  opacity: 1;
}

.account-descriptions {
  width: 100%;
}

.user-center-page :deep(.n-descriptions) {
  --n-th-color: var(--ui-surface-muted) !important;
  --n-td-color: var(--ui-surface) !important;
  --n-border-color: var(--ui-border) !important;
  background: transparent !important;
}

.notification-intro { margin:0 0 24px; color:var(--ui-text-muted); line-height:1.7; }
.notification-actions { display:flex; flex-wrap:wrap; gap:12px; justify-content:flex-end; }
.global-notification-link { display:flex; align-items:center; justify-content:space-between; flex-wrap:wrap; gap:12px; margin-top:24px; padding-top:20px; border-top:1px solid var(--ui-border); font-size:13px; color:var(--ui-text-muted); }
.account-descriptions :deep(th) { width:104px; white-space:nowrap; }
@media (max-width:540px) {
  .profile-avatar-section { gap:16px; padding-bottom:20px; }
  .username-text { font-size:18px; }
  .notification-actions > .n-button { width:100%; }
  .account-descriptions :deep(th) { width:90px; }
}
</style>
