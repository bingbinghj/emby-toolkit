<template>
  <div class="mp-page notification-settings-page">
    <h1 class="sr-only">通知</h1>

    <VAlert v-if="configError || accountError" type="error" variant="tonal" class="mb-4">
      <div v-if="configError">{{ configError }}</div><div v-if="accountError">{{ accountError }}</div>
      <VBtn variant="text" :loading="loadingConfig || loading" @click="retryLoading">重新读取通知设置</VBtn>
    </VAlert>

    <VForm @submit.prevent="saveChatId">
      <SettingCard class="mb-6" title="通知渠道" subtitle="通过 Telegram 接收入库、拦截和识别失败通知。">
        <template #status><VChip size="small" variant="tonal" :color="channelStatus.color">{{ channelStatus.text }}</VChip></template>
        <VProgressLinear v-if="loadingConfig" indeterminate />
        <VCardText v-if="configModel">
          <VRow>
            <VCol cols="12" md="6">
              <VTextField v-model="configModel.telegram_bot_token" label="Telegram Bot Token" :type="showBotToken ? 'text' : 'password'"
                :append-inner-icon="showBotToken ? 'mdi-eye-off-outline' : 'mdi-eye-outline'" placeholder="从 @BotFather 获取"
                autocomplete="off" :disabled="busy || loadingConfig" @click:append-inner="showBotToken = !showBotToken" />
            </VCol>
            <VCol cols="12" md="6">
              <VTextField v-model="configModel.telegram_channel_id" label="全局通知频道" placeholder="频道链接、@频道名或 Chat ID" :disabled="busy || loadingConfig" />
            </VCol>
          </VRow>

        </VCardText>
        <VCardText v-else><VSkeletonLoader v-if="loadingConfig" type="list-item-two-line" /><VAlert v-else type="warning" variant="tonal">尚未读取通知配置，请重新读取后保存。</VAlert></VCardText>
      </SettingCard>

      <SettingCard title="通知发送范围" :subtitle="`接收账号：${authStore.username}`">
        <VProgressLinear v-if="loading" indeterminate />
        <VCardText>
          <div v-if="configModel">
            <div class="text-body-2 font-weight-medium mb-2">消息类型</div>
            <div class="d-flex flex-wrap ga-3">
              <VCheckbox v-model="configModel.telegram_notify_types" multiple value="library_new" label="入库通知" :disabled="busy" />
              <VCheckbox v-model="configModel.telegram_notify_types" multiple value="intercept_notify" label="拦截通知" :disabled="busy" />
              <VCheckbox v-model="configModel.telegram_notify_types" multiple value="recognize_fail" label="识别失败" :disabled="busy" />
            </div>
            <VDivider class="my-4" />
          </div>
          <VTextField v-model="telegramChatId" label="Telegram 用户ID（私聊ID）" placeholder="例如 123456789"
            hint="先向 Telegram 机器人发送 /start，再填写私聊ID，用于接收个人通知。" persistent-hint :disabled="busy || loading || Boolean(accountError)" />
          <div v-if="globalChannelLink" class="global-notification-link mt-4">
            <span class="text-body-2 text-medium-emphasis">全局通知频道</span>
            <VBtn variant="text" :href="globalChannelLink" target="_blank" rel="noopener noreferrer" prepend-icon="mdi-open-in-new">加入频道 / 群组</VBtn>
          </div>
        </VCardText>
        <VCardActions class="notification-actions pa-4">
          <VBtn variant="tonal" prepend-icon="mdi-telegram" :loading="isFetchingBotLink" :disabled="busy || !configModel?.telegram_bot_token" @click="openBotChat">打开机器人</VBtn>
          <VBtn variant="tonal" prepend-icon="mdi-send-outline" :loading="isTestingTelegram" :disabled="busy || !canTest" @click="testTelegram">发送测试</VBtn>
          <VSpacer />
          <VBtn type="submit" variant="flat" prepend-icon="mdi-content-save-outline" :loading="isSavingChatId" :disabled="busy || !canSave">保存通知设置</VBtn>
        </VCardActions>
      </SettingCard>
    </VForm>
  </div>
</template>

<script setup>
import { ref, computed, onMounted, onBeforeUnmount } from 'vue';
import axios from 'axios';
import { useAuthStore } from '@/stores/auth';
import { useConfig } from '@/composables/useConfig';
import { useMessage } from '@/composables/useFeedback';
import SettingCard from './general/SettingCard.vue';

const authStore = useAuthStore();
const message = useMessage();
const { configModel, loadingConfig, handleSaveConfig, configError, fetchConfigData } = useConfig();
const loading = ref(true), accountError = ref(''), telegramChatId = ref('');
const showBotToken = ref(false), isSavingChatId = ref(false), isTestingTelegram = ref(false), isFetchingBotLink = ref(false);
let pageActive = true;
let loadedChatId = '';
const isCurrentAccount = user => pageActive && authStore.isLoggedIn && authStore.user === user;
const busy = computed(() => isSavingChatId.value || isTestingTelegram.value || isFetchingBotLink.value);
const canSave = computed(() => Boolean(configModel.value) && !loadingConfig.value && !loading.value && !accountError.value);
const canTest = computed(() => canSave.value && Boolean(configModel.value.telegram_bot_token?.trim()) && Boolean(telegramChatId.value.trim() || configModel.value.telegram_channel_id?.trim()));
const channelStatus = computed(() => configError.value ? { text: configModel.value ? '保存未确认' : '读取失败', color: 'error' }
  : loadingConfig.value ? { text: '正在读取', color: 'secondary' }
  : configModel.value?.telegram_bot_token?.trim() ? { text: '已配置', color: 'info' } : { text: '未配置', color: 'secondary' });
const globalChannelLink = computed(() => {
  const id = authStore.accountInfo?.telegram_channel_id?.trim();
  if (!id) return '';
  if (id.startsWith('https://t.me/')) return id;
  if (id.startsWith('@')) return `https://t.me/${id.substring(1)}`;
  return /^-?\d+$/.test(id) ? '' : `https://t.me/${id}`;
});

const loadAccount = async () => {
  if (!pageActive) return;
  loading.value = true;
  accountError.value = '';
  const user = authStore.user;
  try {
    const account = await authStore.fetchAccountInfo();
    if (!isCurrentAccount(user)) return;
    if (!account || !Object.keys(account).length) throw new Error('未获取到接收账号信息');
    const chatId = account.telegram_chat_id || '';
    if (telegramChatId.value === loadedChatId) telegramChatId.value = chatId;
    loadedChatId = chatId;
  } catch (error) {
    if (isCurrentAccount(user)) accountError.value = error.response?.data?.error || error.message || '加载接收账号失败';
  } finally {
    if (isCurrentAccount(user)) loading.value = false;
  }
};
const retryLoading = async () => {
  await Promise.all([fetchConfigData(), loadAccount()]);
};

const saveChatId = async () => {
  if (!canSave.value || busy.value) return;
  const user = authStore.user;
  const chatId = telegramChatId.value.trim();
  const config = configModel.value;
  const submitted = { telegram_bot_token: config.telegram_bot_token, telegram_channel_id: config.telegram_channel_id,
    telegram_notify_types: [...(config.telegram_notify_types || [])] };
  isSavingChatId.value = true;
  let globalSaved = false;
  try {
    if (!await handleSaveConfig(submitted)) throw new Error(configError.value || '通知配置保存失败');
    globalSaved = true;
    if (!isCurrentAccount(user)) return;
    const { data } = await axios.post('/api/portal/telegram-chat-id', { chat_id: chatId });
    if (!isCurrentAccount(user)) return;
    if (authStore.accountInfo) {
      authStore.accountInfo.telegram_chat_id = chatId;
      authStore.accountInfo.telegram_channel_id = submitted.telegram_channel_id;
    }
    message.success(data.message || '通知设置已保存');
  } catch (error) {
    if (isCurrentAccount(user)) message.error(`${globalSaved ? '全局配置已保存，个人通知保存失败：' : ''}${error.response?.data?.error || error.response?.data?.message || error.message || '保存失败'}`);
  } finally {
    isSavingChatId.value = false;
  }
};

const testTelegram = async () => {
  if (!canTest.value || busy.value) return;
  const user = authStore.user;
  isTestingTelegram.value = true;
  try {
    const { data } = await axios.post('/api/telegram/test', { token: configModel.value.telegram_bot_token,
      chat_id: telegramChatId.value.trim() || configModel.value.telegram_channel_id });
    if (!isCurrentAccount(user)) return;
    if (data.success === true) message.success(data.message || '测试消息已发送');
    else message.error(data.error || data.message || '测试消息发送失败');
  } catch (error) {
    if (isCurrentAccount(user)) message.error(error.response?.data?.error || error.response?.data?.message || '测试消息发送失败');
  } finally { isTestingTelegram.value = false; }
};

const openBotChat = async () => {
  if (busy.value || !configModel.value?.telegram_bot_token) return;
  const user = authStore.user;
  isFetchingBotLink.value = true;
  try {
    const { data } = await axios.get('/api/portal/telegram-bot-info');
    if (!isCurrentAccount(user)) return;
    if (data.bot_username) window.open(`https://t.me/${encodeURIComponent(data.bot_username)}`, '_blank', 'noopener,noreferrer');
    else message.error(data.error || '未能获取到机器人信息');
  } catch (error) {
    if (isCurrentAccount(user)) message.error(error.response?.data?.error || '请求机器人信息失败');
  } finally { isFetchingBotLink.value = false; }
};

onMounted(loadAccount);
onBeforeUnmount(() => { pageActive = false; });
</script>

<style scoped>
.notification-actions { display:flex; flex-wrap:wrap; gap:12px; }
.global-notification-link { display:flex; flex-wrap:wrap; align-items:center; gap:12px; }
@media (max-width:540px) { .notification-actions > .v-btn { width:100%; } }
</style>
