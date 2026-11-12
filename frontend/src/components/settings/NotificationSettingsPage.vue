<template>
  <div class="mp-page notification-settings-page">
    <h1 class="sr-only">通知</h1>
    <SettingsSearchResults :query="settingsSearch.query.value" :results="settingsSearch.results.value" :notice="settingsSearch.notice.value" @select="settingsSearch.select" />
    <VAlert v-if="configError || accountError" type="error" variant="tonal" class="mb-4">
      <div v-if="configError">{{ configError }}</div><div v-if="accountError">{{ accountError }}</div>
      <VBtn variant="text" :loading="loadingConfig || loading" @click="retryLoading">重新读取通知设置</VBtn>
    </VAlert>
      <SettingCard v-show="settingsSearch.showCard('通知渠道')" class="mb-6" title="通知渠道" subtitle="设置消息发送渠道参数">
        <VProgressLinear v-if="loadingConfig" indeterminate />
        <VCardText v-if="configModel">
          <div class="grid gap-3 grid-app-card">
          <VCard class="notification-channel app-card-shell app-card-colorful" style="--app-card-accent-rgb:36,169,224" data-field="notification_channel" data-settings-label="Telegram" variant="tonal" role="button" tabindex="0" aria-label="编辑 Telegram 通知渠道" @click="editChannel" @keydown.enter.prevent="editChannel" @keydown.space.prevent="editChannel">
            <VCardText class="app-card-summary app-card-summary--title-subtitle">
              <div class="app-card-summary__content">
                <div class="app-card-summary__title-row"><VBadge v-if="configModel.telegram_enabled" dot inline color="success" class="me-1" /><span class="app-card-summary__title text-h6">{{ configModel.telegram_channel_name || 'Telegram' }}</span></div>
                <div class="app-card-summary__subtitle text-body-1">Telegram</div>
              </div>
              <div class="app-card-summary__media" aria-hidden="true"><VImg :src="telegramLogo" contain class="app-card-summary__image" /></div>
            </VCardText>
          </VCard>
          </div>
        </VCardText>
        <VCardText v-else><VSkeletonLoader v-if="loadingConfig" type="list-item-two-line" /><VAlert v-else type="warning" variant="tonal">尚未读取通知配置，请重新读取后保存。</VAlert></VCardText>
        <template #actions>
          <VBtn color="primary" variant="flat" prepend-icon="mdi-content-save" aria-label="保存通知渠道" :loading="isSavingChatId" :disabled="busy || !canSave" @click="saveChatId">保存</VBtn>
        </template>
      </SettingCard>

      <SettingCard v-show="settingsSearch.showCard('通知发送范围')" class="mb-6" title="通知发送范围" subtitle="对应消息类型只会发送给设定的用户。">
        <VCardText v-if="configModel">
          <div class="notification-scope-table" role="table" aria-label="通知发送范围">
            <div class="scope-header" role="row"><span role="columnheader">消息类型</span><span role="columnheader">范围</span></div>
            <div v-for="kind in notificationKinds" :key="kind.value" class="scope-row" role="row">
              <span role="cell">{{ kind.title }}</span>
              <VRadioGroup data-field="notification_scope" data-settings-label="通知发送范围" v-model="configModel.notification_scope[kind.value]" inline hide-details :aria-label="`${kind.title}通知发送范围`" :disabled="busy" role="cell">
                <VRadio v-for="scope in scopeOptions" :key="scope.value" :value="scope.value" :label="scope.title" />
              </VRadioGroup>
            </div>
          </div>
          <p class="text-caption text-medium-emphasis mt-3 mb-0">自动入库未关联操作用户时，按管理员范围发送。</p>
        </VCardText>
        <template #actions>
          <VBtn color="primary" variant="flat" prepend-icon="mdi-content-save" aria-label="保存通知发送范围" :loading="savingCard === 'notification_scope'" :disabled="busy || !canSaveConfig" @click="saveNotificationCard('notification_scope')">保存</VBtn>
        </template>
      </SettingCard>

      <SettingCard v-show="settingsSearch.showCard('通知发送时间')" class="mb-6" title="通知发送时间" subtitle="设定消息发送的时间范围。">
        <VCardText v-if="configModel">
          <VRow>
            <VCol cols="12" sm="6"><VTextField data-field="notification_time.start" v-model="configModel.notification_time.start" label="开始时间" type="time" prepend-inner-icon="mdi-clock-start" :disabled="busy" /></VCol>
            <VCol cols="12" sm="6"><VTextField data-field="notification_time.end" v-model="configModel.notification_time.end" label="结束时间" type="time" prepend-inner-icon="mdi-clock-end" :disabled="busy" /></VCol>
          </VRow>
        </VCardText>
        <template #actions>
          <VBtn color="primary" variant="flat" prepend-icon="mdi-content-save" aria-label="保存通知发送时间" :loading="savingCard === 'notification_time'" :disabled="busy || !canSaveConfig" @click="saveNotificationCard('notification_time')">保存</VBtn>
        </template>
      </SettingCard>

    <VDialog v-model="channelDialogOpen" scrollable max-width="40rem" :fullscreen="!display.mdAndUp.value" :persistent="busy">
      <VCard v-if="channelDraft" data-settings-card="通知渠道">
        <VCardItem class="py-2">
          <template #prepend><VIcon icon="mdi-cog" class="me-2" /></template>
          <VCardTitle>配置</VCardTitle><VCardSubtitle>{{ configModel.telegram_channel_name || 'Telegram' }}</VCardSubtitle>
        </VCardItem>
        <VBtn class="absolute right-3 top-3 z-10" icon="mdi-close" color="default" variant="text" :disabled="busy" aria-label="关闭通知渠道" @click="channelDialogOpen = false" />
        <VDivider />
        <VCardText>
          <VRow>
            <VCol cols="12" md="6"><VSwitch data-field="telegram_enabled" v-model="channelDraft.telegram_enabled" label="启用" color="primary" :disabled="busy" /></VCol>
            <VCol cols="12"><VAutocomplete data-field="telegram_notify_types" v-model="channelDraft.telegram_notify_types" :items="notificationKinds" item-title="title" item-value="value" label="类型" hint="通知渠道类型" multiple clearable chips persistent-hint prepend-inner-icon="mdi-bell-outline" :disabled="busy" /></VCol>
          </VRow>
          <VRow>
            <VCol cols="12" md="6"><VTextField data-field="telegram_channel_name" v-model="channelDraft.telegram_channel_name" label="名称" placeholder="名称" hint="通知渠道名称" persistent-hint prepend-inner-icon="mdi-label" :disabled="busy" /></VCol>
            <VCol cols="12" md="6"><VTextField data-field="telegram_bot_token" v-model="channelDraft.telegram_bot_token" label="Bot Token" :type="showBotToken ? 'text' : 'password'" :append-inner-icon="showBotToken ? 'mdi-eye-off-outline' : 'mdi-eye-outline'" hint="Telegram机器人token，格式：123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11" persistent-hint prepend-inner-icon="mdi-key" autocomplete="off" :disabled="busy" @click:append-inner="showBotToken = !showBotToken" /></VCol>
            <VCol cols="12" md="6"><VTextField data-field="telegram_channel_id" v-model="channelDraft.telegram_channel_id" label="Chat ID" hint="接受消息通知的用户、群组或频道Chat ID" persistent-hint prepend-inner-icon="mdi-chat" :disabled="busy" /></VCol>
            <VCol cols="12" md="6"><VTextField data-field="telegram_topic_id" v-model="channelDraft.telegram_topic_id" label="话题 ID" hint="可选，向该群组的指定话题发送通知；留空发送到默认话题" persistent-hint prepend-inner-icon="mdi-forum" :disabled="busy" /></VCol>
            <VCol cols="12" md="6"><VTextField data-field="telegram_users" v-model="channelDraft.telegram_users" label="用户白名单" placeholder="用户ID列表，多个ID使用,分隔" hint="可使用Telegram机器人的用户ID清单，多个用户用,分隔，不填写则所有用户都能使用" persistent-hint prepend-inner-icon="mdi-account-group" :disabled="busy" /></VCol>
            <VCol cols="12" md="6"><VTextField data-field="telegram_admins" v-model="channelDraft.telegram_admins" label="管理员白名单" placeholder="用户ID列表，多个ID使用,分隔" hint="可使用管理菜单及命令的用户ID列表，多个ID使用,分隔；维护任务还需绑定 Emby 管理员账号" persistent-hint prepend-inner-icon="mdi-account-supervisor" :disabled="busy" /></VCol>
            <VCol cols="12" md="6"><VTextField data-field="telegram_api_url" v-model="channelDraft.telegram_api_url" label="代理API地址" placeholder="https://api.telegram.org" hint="自定义代理API地址，格式：https://api.telegram.org" persistent-hint prepend-inner-icon="mdi-web" :disabled="busy" /></VCol>
          </VRow>
          <VDivider class="my-4" />
          <VTextField data-field="telegram_chat_id" v-model="channelDraft.chat_id" label="个人 Chat ID" placeholder="例如 123456789" hint="先向 Telegram 机器人发送 /start，再填写私聊ID，用于接收个人通知。" persistent-hint prepend-inner-icon="mdi-account" :disabled="busy || loading || Boolean(accountError)" />
          <div v-if="globalChannelLink" class="global-notification-link mt-4"><VBtn variant="text" :href="globalChannelLink" target="_blank" rel="noopener noreferrer" prepend-icon="mdi-open-in-new">加入频道 / 群组</VBtn></div>
        </VCardText>
        <VCardActions class="notification-actions px-6 pb-4">
          <VBtn variant="tonal" prepend-icon="mdi-telegram" :loading="isFetchingBotLink" :disabled="busy || !channelDraft?.telegram_bot_token?.trim()" @click="openBotChat">打开机器人</VBtn>
          <VBtn variant="tonal" prepend-icon="mdi-send-outline" :loading="isTestingTelegram" :disabled="busy || !canTest" @click="testTelegram">发送测试</VBtn>
          <VSpacer />
          <VBtn variant="text" :disabled="busy" @click="channelDialogOpen = false">取消</VBtn>
          <VBtn color="primary" variant="flat" prepend-icon="mdi-content-save" class="px-5" :disabled="busy" @click="applyChannelDraft">确定</VBtn>
        </VCardActions>
      </VCard>
    </VDialog>
  </div>
</template>
<script setup>
import { ref, computed, watch, onMounted, onBeforeUnmount } from 'vue';
import axios from 'axios';
import { useDisplay } from 'vuetify';
import telegramLogo from '@/assets/mp-telegram.webp';
import { useAuthStore } from '@/stores/auth';
import { useConfig } from '@/composables/useConfig';
import { useMessage } from '@/composables/useFeedback';
import SettingCard from './general/SettingCard.vue';
import SettingsSearchResults from './general/SettingsSearchResults.vue';
import { readPersonalNotificationDraft, savePersonalNotificationDraft, useSettingsSearch } from '@/composables/useSettingsSearch';
import { notificationKinds, scopeOptions } from '@/utils/notificationOptions';

const authStore = useAuthStore();
const message = useMessage();
const display = useDisplay();
const { configModel, loadingConfig, savingConfig, handleSaveConfig, configError, fetchConfigData } = useConfig();
const loading = ref(true), accountError = ref(''), telegramChatId = ref('');
const showBotToken = ref(false), isSavingChatId = ref(false), isTestingTelegram = ref(false), isFetchingBotLink = ref(false);
const savingCard = ref('');
let pageActive = true;
let loadedChatId = '';
const isCurrentAccount = user => pageActive && authStore.isLoggedIn && authStore.user === user;
const busy = computed(() => savingConfig.value || Boolean(savingCard.value) || isSavingChatId.value || isTestingTelegram.value || isFetchingBotLink.value);
const canSaveConfig = computed(() => Boolean(configModel.value) && !loadingConfig.value);
const canSave = computed(() => canSaveConfig.value && !loading.value && !accountError.value);
const canTest = computed(() => {
  if (!canSave.value) return false;
  const draft = channelDraft.value;
  return draft ? Boolean(draft.telegram_bot_token?.trim() && (draft.chat_id?.trim() || draft.telegram_channel_id?.trim()))
    : Boolean(configModel.value.telegram_bot_token?.trim() && (telegramChatId.value.trim() || configModel.value.telegram_channel_id?.trim()));
});
const channelDialogOpen = ref(false), channelDraft = ref(null);
const notificationDraftOwner = authStore.user?.id;
const settingsSearch = useSettingsSearch('notification', {
  ready: computed(() => canSave.value && !busy.value), disabled: computed(() => loadingConfig.value || busy.value),
  openChannel: () => { if (!channelDialogOpen.value) editChannel(); return channelDialogOpen.value; },
});
const personalDraft = readPersonalNotificationDraft(notificationDraftOwner);
if (personalDraft) {
  telegramChatId.value = personalDraft.chatId;
  loadedChatId = personalDraft.loadedChatId;
}
watch(configModel, config => {
  if (!config) return;
  config.telegram_enabled ??= true;
  config.telegram_channel_name ??= 'Telegram';
  config.telegram_topic_id ??= '';
  config.telegram_users ??= '';
  config.telegram_admins ??= '';
  config.telegram_api_url ??= '';
  config.telegram_notify_types ??= ['library_new', 'recognize_fail'];
  config.notification_scope ??= { library_new: 'admin', recognize_fail: 'admin' };
  config.notification_time ??= { start: '00:00', end: '23:59' };
}, { immediate: true });
function editChannel() {
  if (!canSave.value || busy.value) return;
  const config = configModel.value;
  channelDraft.value = { telegram_bot_token: config.telegram_bot_token || '', telegram_channel_id: config.telegram_channel_id || '', telegram_enabled: config.telegram_enabled,
    telegram_channel_name: config.telegram_channel_name || 'Telegram', telegram_topic_id: config.telegram_topic_id || '', telegram_users: config.telegram_users || '', telegram_admins: config.telegram_admins || '', telegram_api_url: config.telegram_api_url || '',
    telegram_notify_types: [...(config.telegram_notify_types || [])], chat_id: telegramChatId.value };
  showBotToken.value = false;
  channelDialogOpen.value = true;
}
function applyChannelDraft() {
  if (!channelDraft.value || busy.value) return;
  const name = channelDraft.value.telegram_channel_name?.trim();
  if (!name) { message.error('通知渠道名称不能为空'); return; }
  channelDraft.value.telegram_channel_name = name;
  const { chat_id, ...config } = channelDraft.value;
  Object.assign(configModel.value, config);
  telegramChatId.value = chat_id;
  channelDialogOpen.value = false;
}
const globalChannelLink = computed(() => {
  const id = (channelDraft.value?.telegram_channel_id ?? configModel.value?.telegram_channel_id)?.trim();
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
    telegram_channel_name: config.telegram_channel_name || 'Telegram', telegram_topic_id: config.telegram_topic_id || '', telegram_users: config.telegram_users || '', telegram_admins: config.telegram_admins || '', telegram_api_url: config.telegram_api_url || '',
    telegram_notify_types: (config.telegram_notify_types || []).filter(kind => ['library_new', 'recognize_fail'].includes(kind)),
    telegram_enabled: config.telegram_enabled ?? true };
  isSavingChatId.value = true;
  let globalSaved = false;
  try {
    if (!await handleSaveConfig(submitted)) throw new Error(configError.value || '通知配置保存失败');
    globalSaved = true;
    if (!isCurrentAccount(user)) return;
    await axios.post('/api/portal/telegram-chat-id', { chat_id: chatId });
    if (!isCurrentAccount(user)) return;
    if (authStore.accountInfo) {
      authStore.accountInfo.telegram_chat_id = chatId;
      authStore.accountInfo.telegram_channel_id = submitted.telegram_channel_id;
    }
    loadedChatId = chatId;
    message.success('通知渠道已保存');
  } catch (error) {
    if (isCurrentAccount(user)) message.error(`${globalSaved ? '全局配置已保存，个人通知保存失败：' : ''}${error.response?.data?.error || error.response?.data?.message || error.message || '保存失败'}`);
  } finally {
    isSavingChatId.value = false;
  }
};

const saveNotificationCard = async (field) => {
  if (!['notification_scope', 'notification_time'].includes(field) || !canSaveConfig.value || busy.value) return;
  const user = authStore.user;
  const title = field === 'notification_scope' ? '通知发送范围' : '通知发送时间';
  const submitted = { [field]: { ...configModel.value[field] } };
  savingCard.value = field;
  try {
    if (!await handleSaveConfig(submitted)) throw new Error(configError.value || `${title}保存失败`);
    if (isCurrentAccount(user)) message.success(`${title}已保存`);
  } catch (error) {
    if (isCurrentAccount(user)) message.error(error.response?.data?.error || error.message || `${title}保存失败`);
  } finally {
    savingCard.value = '';
  }
};

const testTelegram = async () => {
  if (!canTest.value || busy.value) return;
  const user = authStore.user;
  isTestingTelegram.value = true;
  try {
    const { data } = await axios.post('/api/telegram/test', { token: channelDraft.value?.telegram_bot_token ?? configModel.value.telegram_bot_token,
      chat_id: channelDraft.value ? (channelDraft.value.chat_id?.trim() || channelDraft.value.telegram_channel_id) : (telegramChatId.value.trim() || configModel.value.telegram_channel_id),
      api_url: channelDraft.value?.telegram_api_url ?? configModel.value.telegram_api_url ?? '',
      topic_id: channelDraft.value?.chat_id?.trim() || (!channelDraft.value && telegramChatId.value.trim()) ? '' : (channelDraft.value?.telegram_topic_id ?? configModel.value.telegram_topic_id ?? '') });
    if (!isCurrentAccount(user)) return;
    if (data.success === true) message.success(data.message || '测试消息已发送');
    else message.error(data.error || data.message || '测试消息发送失败');
  } catch (error) {
    if (isCurrentAccount(user)) message.error(error.response?.data?.error || error.response?.data?.message || '测试消息发送失败');
  } finally { isTestingTelegram.value = false; }
};

const openBotChat = async () => {
  const token = channelDraft.value?.telegram_bot_token ?? configModel.value?.telegram_bot_token;
  if (busy.value || !token?.trim()) return;
  const user = authStore.user;
  isFetchingBotLink.value = true;
  try {
    const { data } = await axios.post('/api/portal/telegram-bot-info', { token, api_url: channelDraft.value?.telegram_api_url ?? configModel.value?.telegram_api_url ?? '' });
    if (!isCurrentAccount(user)) return;
    if (data.bot_username) window.open(`https://t.me/${encodeURIComponent(data.bot_username)}`, '_blank', 'noopener,noreferrer');
    else message.error(data.error || '未能获取到机器人信息');
  } catch (error) {
    if (isCurrentAccount(user)) message.error(error.response?.data?.error || '请求机器人信息失败');
  } finally { isFetchingBotLink.value = false; }
};

onMounted(loadAccount);
onBeforeUnmount(() => {
  if (authStore.isLoggedIn && authStore.user?.id === notificationDraftOwner) savePersonalNotificationDraft(notificationDraftOwner, telegramChatId.value, loadedChatId);
  pageActive = false;
});
</script>

<style scoped>
.notification-actions { display:flex; flex-wrap:wrap; gap:12px; }
.notification-channel { cursor:pointer; }
.notification-channel .app-card-summary__content { padding-inline-end:4rem; }
.notification-channel:focus-visible { outline:2px solid rgb(var(--v-theme-primary)); outline-offset:3px; }
.notification-scope-table { min-width:0; }
.scope-header, .scope-row { display:grid; grid-template-columns:130px minmax(0,1fr); gap:16px; align-items:center; padding:12px 0; border-bottom:1px solid var(--ui-border); }
.scope-header { font-size:14px; font-weight:600; }
.scope-row :deep(.v-selection-control-group) { flex-wrap:wrap; gap:4px 20px; }
.scope-row :deep(.v-label) { white-space:normal; }
.global-notification-link { display:flex; flex-wrap:wrap; align-items:center; gap:12px; }
@media (max-width:600px) {
  .scope-header { display:none; }
  .scope-row { grid-template-columns:minmax(0,1fr); gap:4px; }
  .notification-channel { max-width:none; }
  .notification-actions { gap:8px; }
}
</style>
