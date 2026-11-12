<template>
  <section class="notification-center" :class="{ 'is-dark-theme': isDarkTheme, 'is-standalone': standalone }" aria-label="实时日志">
    <header class="notification-heading">
      <VCardTitle tag="h2" class="pa-0">实时日志</VCardTitle>
      <VBtn variant="tonal" prepend-icon="mdi-download" color="default" class="logging-heading-action" size="small" aria-label="下载实时日志" title="下载日志" :loading="isDownloading" :disabled="isDownloading" @click="downloadLogs">下载</VBtn>
      <VBtn v-if="!standalone" variant="tonal" prepend-icon="mdi-open-in-new" color="default" class="logging-heading-action" size="small" aria-label="在新窗口中打开实时日志" title="在新窗口中打开" :href="logFileUrl" target="_blank" rel="noopener noreferrer">在新窗口中打开</VBtn>
      <VSpacer class="logging-heading-spacer" />
      <VBtn v-if="!standalone" variant="text" icon="mdi-close" color="default" class="logging-close-action" size="small" aria-label="关闭实时日志" title="关闭" @click="$emit('close')" />
    </header>
    <VTabs v-model="activeTab" color="primary" density="compact" class="notification-tabs">
      <VTab value="logs" prepend-icon="mdi-console-line">实时日志</VTab>
      <VTab value="history" prepend-icon="mdi-history">历史日志</VTab>
    </VTabs>
    <div v-show="activeTab === 'logs'" class="notification-live logging-view">
      <VProgressLinear v-if="following" indeterminate color="primary" :height="1" class="logging-live-progress" aria-label="实时日志正在更新" />
      <div class="notification-toolbar logging-toolbar px-3">
        <VBtnToggle v-if="display.mdAndUp.value" v-model="logFilter" mandatory divided density="compact" variant="text" class="logging-level-toggle" selected-class="logging-level-toggle__button--active" aria-label="日志级别">
          <VBtn v-for="option in filterOptions" :key="option.value" :value="option.value" class="logging-level-toggle__button" :class="`logging-level-toggle__button--${option.value}`">{{ option.value === 'all' ? '全部' : option.label }}</VBtn>
        </VBtnToggle>
        <VSelect v-else v-model="logFilter" :items="filterOptions" item-title="label" item-value="value" hide-details variant="plain" density="compact" aria-label="日志类型" class="notification-filter logging-level-select" :menu-props="{ width: 'auto' }">
          <template #selection="{ item }"><span class="font-weight-medium">{{ item.title }}</span></template>
        </VSelect>
        <VTextField v-model="keyword" clearable hide-details variant="plain" density="compact" prepend-inner-icon="mdi-magnify" placeholder="搜索日志" aria-label="搜索实时日志" class="notification-search logging-search" />
        <VBtn variant="flat" color="primary" icon class="logging-stream-action" :aria-label="following ? '暂停日志流' : '继续日志流'" :title="following ? '暂停日志流' : '继续日志流'" @click="following = !following"><VIcon :icon="following ? 'mdi-pause' : 'mdi-play'" /></VBtn>
      </div>
      <div ref="logRef" class="notification-log logging-shell" role="region" aria-label="实时日志内容" tabindex="0" @scroll="handleScroll">
        <LogRecord v-for="(group, index) in logGroups" :key="group.key" :lines="group.lines" :keyword="keyword" :stripe="index % 2 ? 'odd' : 'even'" />
        <div v-if="!filteredLogs.length" class="logging-empty"><VIcon :icon="displayedLogs.length ? 'mdi-filter-remove-outline' : 'mdi-console-line'" size="20" /><span>{{ displayedLogs.length ? '没有匹配的数据' : '等待日志输出' }}</span></div>
      </div>
      <footer class="notification-footer">
        <span>{{ filteredLogs.length }} 条日志{{ following ? '' : ' · 日志流已暂停' }}</span>
        <VBtn v-if="!following" size="small" color="primary" variant="elevated" prepend-icon="mdi-arrow-down" @click="following = true">{{ hasNewLogs ? '有新日志 · 回到最新' : '回到最新' }}</VBtn>
      </footer>
    </div>
    <LogViewer v-show="activeTab === 'history'" :show="activeTab === 'history'" />
  </section>
</template>

<script setup>
import { computed, ref, watch, nextTick } from 'vue';
import { useDisplay, useTheme } from 'vuetify';
import axios from 'axios';
import { useMessage } from '@/composables/useFeedback';
import LogViewer from './LogViewer.vue';
import LogRecord from './LogRecord.vue';
import { groupLogRecords, matchesLogRecord } from '@/utils/logRecords';

const props = defineProps({ task: { type: Object, default: () => ({}) }, standalone: Boolean });
defineEmits(['close']);
const display = useDisplay();
const theme = useTheme();
const message = useMessage();
const logFileUrl = '/api/logs/view?filename=app.log';
const isDarkTheme = computed(() => theme.global.current.value.dark);
const activeTab = ref('logs'), logRef = ref(null);
const keyword = ref(''), logFilter = ref('all'), following = ref(true), displayedLogs = ref([]), isDownloading = ref(false);
const filterOptions = [{ label: '全部级别', value: 'all' }, { label: 'DEBUG', value: 'debug' }, { label: 'INFO', value: 'info' }, { label: 'WARNING', value: 'warning' }, { label: 'ERROR', value: 'error' }];
const latestLogs = computed(() => props.task.logs || []);
const hasNewLogs = computed(() => latestLogs.value.join('\n') !== displayedLogs.value.join('\n'));
async function downloadLogs() {
  isDownloading.value = true;
  try {
    const { data } = await axios.get(logFileUrl, { responseType: 'blob' });
    const url = URL.createObjectURL(data), link = document.createElement('a');
    link.href = url;
    link.download = 'app.log';
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  } catch (error) {
    message.error(error.response?.data?.error || '下载日志失败，请重试。');
  } finally {
    isDownloading.value = false;
  }
}
const filteredLogs = computed(() => displayedLogs.value.filter(line => matchesLogRecord(line, keyword.value, logFilter.value)));
const logGroups = computed(() => groupLogRecords(displayedLogs.value)
  .map(group => ({ ...group, lines: group.lines.filter(line => matchesLogRecord(line, keyword.value, logFilter.value)) }))
  .filter(group => group.lines.length));
watch([latestLogs, following], () => { if (following.value) displayedLogs.value = [...latestLogs.value]; }, { immediate: true });
watch([filteredLogs, activeTab, following], async () => {
  if (activeTab.value === 'logs' && following.value) {
    await nextTick();
    if (logRef.value) logRef.value.scrollTop = logRef.value.scrollHeight;
  }
}, { immediate: true });
function handleScroll() {
  const el = logRef.value;
  if (el && el.scrollHeight - el.scrollTop - el.clientHeight > 32) following.value = false;
}
</script>

<style scoped>
.notification-center { --logging-shell-background:linear-gradient(180deg,var(--logging-shell-bg),rgba(var(--v-theme-surface),0.9)); --logging-shell-bg:rgba(var(--v-theme-surface),0.96); --logging-record-bg-even:rgba(var(--v-theme-surface-variant),0.01); --logging-record-bg-odd:rgba(var(--v-theme-surface-variant),0.005); --logging-border:rgba(var(--v-theme-on-surface),0.12); --logging-text:rgba(var(--v-theme-on-surface),0.88); display:flex; flex-direction:column; box-sizing:border-box; height:calc(100dvh - 48px); min-height:0; min-width:0; gap:16px; padding:24px; overflow:hidden; }
.notification-center.is-dark-theme { --logging-shell-bg:rgba(var(--v-theme-surface),0.72); --logging-record-bg-even:rgba(var(--v-theme-on-surface),0.02); --logging-record-bg-odd:rgba(var(--v-theme-on-surface),0.008); --logging-shadow:inset 0 1px 0 rgba(255,255,255,0.04); }
.notification-center.is-standalone { height:100dvh; }
.notification-heading { display:flex; align-items:center; gap:8px; flex-shrink:0; }
.notification-heading :deep(.v-card-title) { white-space:normal; overflow-wrap:anywhere; }
.logging-heading-action { color:color-mix(in srgb,rgb(var(--v-theme-on-surface)) 80%,rgb(var(--v-theme-primary))); }
.notification-tabs { flex-shrink:0; }
.notification-tabs :deep(.v-tab--selected) { color:color-mix(in srgb,rgb(var(--v-theme-on-surface)) 80%,rgb(var(--v-theme-primary))) !important; }
.notification-live { display:flex; flex:1; flex-direction:column; min-height:0; min-width:0; gap:12px; overflow:hidden; }
.logging-toolbar { display:flex; flex-wrap:wrap; align-items:center; gap:8px; flex-shrink:0; }
.logging-search { flex:1 1 9rem; min-inline-size:5rem; }
.logging-level-select { flex:0 0 7rem; min-inline-size:7rem; }
.logging-search :deep(.v-field), .logging-level-select :deep(.v-field) { border-radius:var(--app-field-radius); background:transparent !important; box-shadow:none !important; }
.logging-search :deep(.v-field__outline), .logging-search :deep(.v-field__overlay), .logging-level-select :deep(.v-field__outline), .logging-level-select :deep(.v-field__overlay) { display:none; }
.logging-search :deep(.v-field__input) { padding-inline-start:0; }
.logging-level-select :deep(.v-field__input) { padding-inline:0; }
.logging-level-toggle { flex:0 0 auto; overflow:hidden; border:1px solid rgba(var(--v-theme-on-surface),0.14); border-radius:var(--app-control-radius); background:transparent; block-size:2rem; box-shadow:none !important; }
.logging-level-toggle :deep(.logging-level-toggle__button) { border-color:rgba(var(--v-theme-on-surface),0.1); border-radius:0; block-size:100%; color:var(--ui-text-muted); font-size:0.75rem; font-weight:600; letter-spacing:0; min-inline-size:auto; padding-inline:1.125rem; text-transform:none; }
.logging-level-toggle :deep(.logging-level-toggle__button--debug) { color:color-mix(in srgb,rgb(var(--v-theme-on-surface)) 80%,rgb(var(--v-theme-secondary))); }
.logging-level-toggle :deep(.logging-level-toggle__button--info) { color:color-mix(in srgb,rgb(var(--v-theme-on-surface)) 80%,rgb(var(--v-theme-success))); }
.logging-level-toggle :deep(.logging-level-toggle__button--warning) { color:color-mix(in srgb,rgb(var(--v-theme-on-surface)) 80%,rgb(var(--v-theme-warning))); }
.logging-level-toggle :deep(.logging-level-toggle__button--error) { color:color-mix(in srgb,rgb(var(--v-theme-on-surface)) 80%,rgb(var(--v-theme-error))); }
.logging-level-toggle :deep(.logging-level-toggle__button--active) { background:rgba(var(--v-theme-primary),0.16); color:color-mix(in srgb,rgb(var(--v-theme-on-surface)) 80%,rgb(var(--v-theme-primary))); }
.notification-log { position:relative; display:flex; flex-direction:column; gap:6px; flex:1 1 0; min-height:0; min-width:0; overflow:auto; overscroll-behavior:contain; padding:14px; border:1px solid var(--logging-border); background:var(--logging-shell-background); box-shadow:var(--logging-shadow); }
.logging-empty { display:flex; align-items:center; justify-content:center; flex:1; gap:8px; color:var(--ui-text-muted); }
.notification-footer { display:flex; align-items:center; flex-wrap:wrap; gap:8px; flex-shrink:0; }
.notification-footer > span { margin-right:auto; font-size:12px; color:var(--ui-text-muted); }
@media (min-width:960px) {
  .logging-toolbar { padding-block-start:4px; }
  .logging-search :deep(.v-field) { display:flex; align-items:center; block-size:2rem; min-block-size:2rem; }
  .logging-search :deep(.v-field__field), .logging-search :deep(.v-field__input), .logging-search :deep(.v-field__prepend-inner), .logging-search :deep(.v-field__clearable) { align-items:center; min-block-size:2rem; padding-block:0; }
  .logging-stream-action { block-size:2rem; inline-size:2rem; }
}
@media (max-width:959px) {
  .notification-center { height:100dvh; padding:16px; }
  .notification-center, .notification-live { gap:12px; }
  .notification-heading { display:grid; grid-template-columns:auto 1fr auto; }
  .notification-heading :deep(.v-card-title) { grid-column:1 / 3; }
  .logging-close-action { grid-column:3; grid-row:1; }
  .logging-heading-action { grid-row:2; justify-self:start; }
  .logging-heading-spacer { display:none; }
  .notification-log { padding:8px; }
}
</style>
