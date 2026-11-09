<template>
  <div class="mp-page user-center-page">
    <div class="d-flex flex-wrap align-center justify-space-between ga-3 mb-6">
      <div>
        <h1 class="text-h4 mb-1">用户中心</h1>
        <p class="text-body-2 text-medium-emphasis mb-0">欢迎回来，{{ accountInfo?.name || authStore.username }}</p>
      </div>
      <VChip :color="authStore.isAdmin ? 'primary' : 'secondary'" variant="tonal" prepend-icon="mdi-account-circle-outline">
        {{ authStore.isAdmin ? '管理员' : '普通用户' }}
      </VChip>
    </div>

    <VRow>
      <VCol cols="12" lg="5">
        <VCard class="account-card h-100">
          <VCardItem title="账户资料" prepend-icon="mdi-account-outline">
            <template #append><VProgressCircular v-if="loading" indeterminate size="20" width="2" /></template>
          </VCardItem>
          <VCardText v-if="accountInfo">
            <div class="profile-avatar-section mb-6 pb-6">
              <VTooltip text="点击更换头像" location="top">
                <template #activator="{ props }">
                  <button
                    v-bind="props"
                    type="button"
                    class="avatar-wrapper"
                    aria-label="更换头像"
                    :disabled="isUploadingAvatar || isDeletingAvatar"
                    @click="triggerFileUpload"
                  >
                    <VAvatar :size="isMobile ? 72 : 96" color="primary" variant="tonal">
                      <VImg v-if="avatarUrl" :src="avatarUrl" :alt="`${authStore.username} 的头像`" cover />
                      <span v-else class="text-h4">{{ authStore.username ? authStore.username.charAt(0).toUpperCase() : 'U' }}</span>
                    </VAvatar>
                    <span class="avatar-overlay"><VIcon icon="mdi-camera-outline" size="24" /></span>
                  </button>
                </template>
              </VTooltip>
              <input ref="fileInput" type="file" hidden accept="image/png, image/jpeg, image/jpg" @change="handleAvatarChange" />
              <div class="profile-copy">
                <h2 class="text-h6 mb-1">{{ accountInfo.name || authStore.username }}</h2>
                <p class="text-caption text-medium-emphasis mb-3">{{ isUploadingAvatar ? '正在上传头像…' : '点击头像更换 · JPG / PNG，最大 5 MB' }}</p>
                <VBtn
                  v-if="avatarUrl"
                  size="small"
                  color="error"
                  variant="tonal"
                  prepend-icon="mdi-delete-outline"
                  :loading="isDeletingAvatar"
                  :disabled="isUploadingAvatar"
                  @click="deleteAvatar"
                >删除头像</VBtn>
              </div>
            </div>
            <VList class="account-details" bg-color="transparent" density="compact">
              <VListItem title="账户状态" prepend-icon="mdi-shield-check-outline">
                <template #append><VChip :color="statusType" size="small" variant="tonal">{{ statusText }}</VChip></template>
              </VListItem>
              <VListItem title="注册时间" prepend-icon="mdi-calendar-outline">
                <template #append><span class="text-body-2">{{ formatDateTime(accountInfo.registration_date) }}</span></template>
              </VListItem>
              <VListItem title="到期时间" prepend-icon="mdi-calendar-check-outline">
                <template #append><span class="text-body-2">{{ accountInfo.expiration_date ? formatDateTime(accountInfo.expiration_date) : '永久有效' }}</span></template>
              </VListItem>
              <VListItem title="账户等级" prepend-icon="mdi-account-key-outline">
                <template #append><span class="text-body-2 font-weight-medium">{{ authStore.isAdmin ? '管理员' : '普通用户' }}</span></template>
              </VListItem>
              <VListItem title="等级说明" :subtitle="authStore.isAdmin ? '拥有系统所有管理权限' : '可查看账户详情并接收通知'" prepend-icon="mdi-information-outline" />
            </VList>
          </VCardText>
          <VCardText v-else-if="loading"><VSkeletonLoader type="avatar, list-item-two-line, list-item-two-line" /></VCardText>
          <VCardText v-else><VAlert type="warning" variant="tonal" text="未获取到账户信息" /></VCardText>
        </VCard>
      </VCol>

      <VCol cols="12" lg="7">
        <VCard class="notification-card h-100">
          <VCardItem title="通知设置" subtitle="绑定 Telegram，接收入库和任务通知。" prepend-icon="mdi-bell-outline" />
          <VProgressLinear v-if="loadingConfig" indeterminate />
          <VCardText>
            <VAlert v-if="configError" type="error" variant="tonal" class="mb-4">{{ configError }}</VAlert>
            <VForm @submit.prevent="saveChatId">
              <template v-if="configModel">
                <VTextField
                  v-model="configModel.telegram_bot_token"
                  label="Telegram Bot Token"
                  :type="showBotToken ? 'text' : 'password'"
                  :append-inner-icon="showBotToken ? 'mdi-eye-off-outline' : 'mdi-eye-outline'"
                  placeholder="从 @BotFather 获取"
                  autocomplete="off"
                  :disabled="loadingConfig || isSavingChatId"
                  class="mb-4"
                  @click:append-inner="showBotToken = !showBotToken"
                />
                <VTextField
                  v-model="configModel.telegram_channel_id"
                  label="全局通知频道"
                  placeholder="频道链接、@频道名或 Chat ID"
                  :disabled="loadingConfig || isSavingChatId"
                  class="mb-3"
                />
                <div class="text-body-2 font-weight-medium mb-1">通知类型</div>
                <div class="d-flex flex-wrap ga-3 mb-4">
                  <VCheckbox v-model="configModel.telegram_notify_types" multiple value="library_new" label="入库通知" :disabled="loadingConfig || isSavingChatId" />
                  <VCheckbox v-model="configModel.telegram_notify_types" multiple value="intercept_notify" label="拦截通知" :disabled="loadingConfig || isSavingChatId" />
                  <VCheckbox v-model="configModel.telegram_notify_types" multiple value="recognize_fail" label="识别失败" :disabled="loadingConfig || isSavingChatId" />
                </div>
              </template>
              <VTextField
                v-model="telegramChatId"
                label="Telegram 用户ID（私聊ID）"
                placeholder="例如 123456789"
                hint="先向 Telegram 机器人发送 /start，再填写私聊ID，用于接收个人通知。"
                persistent-hint
                :disabled="loading || isSavingChatId"
                class="mb-6"
              />
              <div class="notification-actions">
                <VBtn variant="tonal" prepend-icon="mdi-telegram" :loading="isFetchingBotLink" @click="openBotChat">打开机器人</VBtn>
                <VBtn
                  variant="tonal"
                  prepend-icon="mdi-send-outline"
                  :loading="isTestingTelegram"
                  :disabled="!configModel?.telegram_bot_token || (!telegramChatId && !configModel?.telegram_channel_id)"
                  @click="testTelegram"
                >发送测试</VBtn>
                <VBtn type="submit" prepend-icon="mdi-content-save-outline" :loading="isSavingChatId" :disabled="loading || loadingConfig">保存通知设置</VBtn>
              </div>
            </VForm>
            <div v-if="accountInfo?.telegram_channel_id" class="global-notification-link mt-6 pt-4">
              <span class="text-body-2 text-medium-emphasis">全局通知频道</span>
              <VBtn variant="text" :href="globalChannelLink" target="_blank" rel="noopener noreferrer" prepend-icon="mdi-open-in-new">加入频道 / 群组</VBtn>
            </div>
          </VCardText>
        </VCard>
      </VCol>
    </VRow>
  </div>
</template>

<script setup>
import { ref, onMounted, onUnmounted, computed } from 'vue';
import axios from 'axios';
import { useAuthStore } from '../stores/auth';
import { useConfig } from '../composables/useConfig';
import { useMessage } from '@/composables/useFeedback';

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
const showBotToken = ref(false);
const fileInput = ref(null);
const isMobile = ref(false);
let accountPageActive = true;
const isCurrentAccount = user => accountPageActive && authStore.isLoggedIn && authStore.user === user;

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
const statusType = computed(() => statusMap[accountInfo.value?.status]?.type || 'secondary');

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
  const requestedUser = authStore.user;

  try {
    const res = await axios.post('/api/portal/upload-avatar', formData, {
      headers: { 'Content-Type': 'multipart/form-data' }
    });
    if (!isCurrentAccount(requestedUser)) return;
    message.success('头像更新成功');
    if (accountInfo.value && res.data.new_tag) {
      accountInfo.value.profile_image_tag = res.data.new_tag;
    }
  } catch (error) {
    if (isCurrentAccount(requestedUser)) message.error(error.response?.data?.message || '上传失败');
  } finally {
    loadingMsg.destroy();
    isUploadingAvatar.value = false;
    event.target.value = '';
  }
};

const deleteAvatar = async () => {
  if (isUploadingAvatar.value || isDeletingAvatar.value) return;
  isDeletingAvatar.value = true;
  const requestedUser = authStore.user;
  try {
    await axios.delete('/api/portal/upload-avatar');
    if (!isCurrentAccount(requestedUser)) return;
    if (accountInfo.value) accountInfo.value.profile_image_tag = null;
    message.success('头像已删除');
  } catch (error) {
    if (isCurrentAccount(requestedUser)) message.error(error.response?.data?.message || error.response?.data?.error || '删除头像失败');
  } finally {
    isDeletingAvatar.value = false;
  }
};

const saveChatId = async () => {
  if (loading.value || isSavingChatId.value) return;
  isSavingChatId.value = true;
  const requestedUser = authStore.user;
  const submittedChatId = telegramChatId.value;
  try {
    const config = configModel.value;
    if (!config || !await handleSaveConfig({ telegram_bot_token: config.telegram_bot_token, telegram_channel_id: config.telegram_channel_id, telegram_notify_types: config.telegram_notify_types })) {
      throw new Error(configError.value || '通知配置保存失败');
    }
    if (!isCurrentAccount(requestedUser)) return;
    const response = await axios.post('/api/portal/telegram-chat-id', {
      chat_id: submittedChatId
    });
    if (!isCurrentAccount(requestedUser)) return;
    if (accountInfo.value) {
      accountInfo.value.telegram_chat_id = submittedChatId;
      accountInfo.value.telegram_channel_id = configModel.value.telegram_channel_id;
    }
    message.success(response.data.message || '保存成功');
  } catch (error) {
    if (isCurrentAccount(requestedUser)) message.error(error.response?.data?.message || error.message || '保存失败');
  } finally {
    isSavingChatId.value = false;
  }
};

const testTelegram = async () => {
  if (isTestingTelegram.value) return;
  isTestingTelegram.value = true;
  const requestedUser = authStore.user;
  try {
    const { data } = await axios.post('/api/telegram/test', { token: configModel.value.telegram_bot_token, chat_id: telegramChatId.value.trim() || configModel.value.telegram_channel_id });
    if (!isCurrentAccount(requestedUser)) return;
    if (data.success === true) message.success(data.message || '测试消息已发送');
    else message.error(data.message || '测试消息发送失败');
  } catch (error) {
    if (isCurrentAccount(requestedUser)) message.error(error.response?.data?.message || '测试消息发送失败');
  } finally {
    isTestingTelegram.value = false;
  }
};

const openBotChat = async () => {
  if (isFetchingBotLink.value) return;
  isFetchingBotLink.value = true;
  const requestedUser = authStore.user;
  try {
    const response = await axios.get('/api/portal/telegram-bot-info');
    if (!isCurrentAccount(requestedUser)) return;
    if (response.data.bot_username) {
      window.open(`https://t.me/${response.data.bot_username}`, '_blank');
    } else {
      message.error(response.data.error || '未能获取到机器人信息');
    }
  } catch (error) {
    if (isCurrentAccount(requestedUser)) message.error('请求机器人信息失败');
  } finally {
    isFetchingBotLink.value = false;
  }
};

onMounted(async () => {
  checkMobile();
  window.addEventListener('resize', checkMobile);
  const requestedUser = authStore.user;
  try {
    await authStore.fetchAccountInfo();
    if (!isCurrentAccount(requestedUser)) return;
    telegramChatId.value = accountInfo.value?.telegram_chat_id || '';
  } catch (error) {
    if (isCurrentAccount(requestedUser)) message.error('加载账户信息失败');
  } finally {
    loading.value = false;
  }
});
onUnmounted(() => {
  accountPageActive = false;
  window.removeEventListener('resize', checkMobile);
});
</script>

<style scoped>
.profile-avatar-section { display:flex; align-items:center; gap:20px; border-bottom:1px solid rgba(var(--v-border-color),var(--v-border-opacity)); }
.profile-copy { min-width:0; overflow-wrap:anywhere; }
.avatar-wrapper { position:relative; display:flex; flex-shrink:0; padding:0; border:0; border-radius:50%; background:transparent; cursor:pointer; overflow:hidden; }
.avatar-wrapper:disabled { cursor:wait; }
.avatar-wrapper:focus-visible { outline:2px solid rgb(var(--v-theme-primary)); outline-offset:4px; }
.avatar-overlay { position:absolute; inset:0; display:flex; align-items:center; justify-content:center; color:white; background:rgba(0,0,0,.4); opacity:0; transition:opacity .2s ease; }
.avatar-wrapper:hover .avatar-overlay, .avatar-wrapper:focus-visible .avatar-overlay { opacity:1; }
.account-details :deep(.v-list-item) { padding-inline:0; }
.account-details :deep(.v-list-item__prepend) { margin-inline-end:8px; }
.notification-actions { display:flex; flex-wrap:wrap; justify-content:flex-end; gap:12px; }
.global-notification-link { display:flex; flex-wrap:wrap; align-items:center; justify-content:space-between; gap:12px; border-top:1px solid rgba(var(--v-border-color),var(--v-border-opacity)); }
@media (max-width:540px) {
  .profile-avatar-section { gap:16px; }
  .notification-actions > .v-btn { width:100%; }
  .account-details :deep(.v-list-item__append) { max-width:55%; text-align:end; }
}
</style>
