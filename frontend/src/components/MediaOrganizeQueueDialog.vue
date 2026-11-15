<!-- 布局参考 MoviePilot-Frontend v3 TransferQueueDialog（MIT），许可证见 frontend/third-party/MoviePilot-LICENSE。 -->
<template>
  <VDialog :model-value="modelValue" :fullscreen="smAndDown" max-width="60rem" scrollable @update:model-value="$emit('update:modelValue', $event)">
    <VCard data-testid="organize-queue-dialog">
      <VCardItem class="queue-heading">
        <template #prepend><VIcon icon="mdi-menu" color="primary" size="28" class="me-2" /></template>
        <VCardTitle>整理队列</VCardTitle>
        <VCardSubtitle v-if="queue.items.length">本页 {{ groups.length }} 项媒体，共 {{ queue.items.length }} 项任务</VCardSubtitle>
        <template #append><VBtn icon="mdi-refresh" variant="text" aria-label="刷新整理队列" :loading="queue.loading" @click="queue.refresh" /><VBtn icon="mdi-close" variant="text" aria-label="关闭整理队列" @click="$emit('update:modelValue', false)" /></template>
      </VCardItem>
      <VDivider />
      <VCardText class="queue-content" :class="{ 'queue-content--active': queue.items.length }">
        <VAlert v-if="queue.error" type="warning" variant="tonal" class="mb-3" data-testid="organize-queue-error">{{ queue.error }}<VBtn variant="text" :loading="queue.loading" @click="queue.refresh">重新读取</VBtn></VAlert>
        <VAlert v-if="queue.actionError || queue.actionMessage" :type="queue.actionError ? 'error' : 'info'" variant="tonal" class="mb-3" data-testid="organize-queue-action-result">{{ queue.actionError || queue.actionMessage }}</VAlert>
        <VProgressLinear v-if="queue.loading && !queue.loaded" indeterminate />
        <div v-if="!queue.items.length && !queue.loading && !queue.error" class="queue-empty"><VIcon icon="mdi-timer-sand-paused" size="36" /><p>当前没有整理任务</p><div class="text-medium-emphasis">选择媒体后，可加入重新整理入库或重新刮削任务。</div></div>
        <template v-if="queue.items.length">
          <section class="queue-overall app-surface-shape" aria-label="整体进度">
            <span class="queue-overall-label">整体进度</span>
            <span class="queue-overall-count">本页已完成 {{ completedCount }} / {{ queue.items.length }}</span>
            <span class="queue-overall-value">{{ Math.round(overallProgress) }}%</span>
            <VProgressLinear class="queue-overall-progress" :model-value="overallProgress" color="primary" bg-color="secondary" height="6" rounded />
          </section>
          <div class="queue-pending-summary text-caption text-medium-emphasis">等待 {{ queue.queuedCount }} 项 · 正在处理 {{ queue.runningCount }} 项</div>
          <div class="queue-main">
            <nav class="queue-media-list" aria-label="队列媒体列表">
              <button v-for="group in groups" :key="group.key" type="button" class="queue-media-button" :class="{ selected: group.key === activeGroupKey }" :aria-current="group.key === activeGroupKey ? 'true' : undefined" @click="activeGroupKey = group.key">
                <VAvatar rounded width="68" height="102"><VImg v-if="group.poster" :src="group.poster" :alt="group.title" cover /><VIcon v-else icon="mdi-movie-open-outline" /></VAvatar>
                <div class="queue-media-copy"><div class="queue-media-title font-weight-medium" :title="group.title">{{ group.title }}</div><div class="queue-media-meta text-caption text-medium-emphasis"><span>已完成 {{ group.completed }} / {{ group.tasks.length }}</span><span class="text-primary">{{ Math.round(group.progress) }}%</span></div><VProgressLinear :model-value="group.progress" color="primary" bg-color="secondary" height="4" rounded /></div>
              </button>
            </nav>
            <section v-if="activeGroup" class="queue-detail">
              <div class="d-flex align-center ga-3 pb-4 queue-active-media"><VAvatar rounded width="64" height="96"><VImg v-if="activeGroup.poster" :src="activeGroup.poster" :alt="activeGroup.title" cover /><VIcon v-else icon="mdi-movie-open-outline" /></VAvatar><div class="queue-media-copy"><div class="text-h6">{{ activeGroup.title }}</div><div class="text-body-2 text-medium-emphasis mt-2">{{ activeGroup.tasks.length }} 项文件任务</div></div></div>
              <div class="queue-task-header" aria-hidden="true"><span></span><span>文件名</span><span>状态</span><span>进度</span><span>操作</span></div>
              <div class="queue-task-list">
              <article v-for="task in activeGroup.tasks" :key="task.id" class="queue-task" :data-testid="'organize-queue-task-' + task.id">
                <VIcon class="queue-task-state-icon" :icon="statusIcon(task)" :color="statusPresentation(task).color" size="22" />
                <div class="queue-task-copy"><div class="queue-task-name font-weight-medium" :title="taskTitle(task)">{{ taskTitle(task) }}</div><div class="text-caption text-medium-emphasis mt-1">{{ operationLabel(task.mode) }}</div><div v-if="task.message" class="text-body-2 text-wrap mt-2">{{ task.message }}</div><div v-for="(detail, index) in resultDetails(task)" :key="index" class="text-caption text-wrap mt-1">{{ detail }}</div></div>
                <div class="queue-task-status"><VChip size="small" variant="tonal" :color="statusPresentation(task).color">{{ statusPresentation(task).title }}</VChip></div>
                <div class="queue-task-progress"><template v-if="task.status === 'running'"><VProgressLinear v-if="task.progress_known || !progressHeld(task)" :model-value="task.progress_known ? task.progress : undefined" :indeterminate="!task.progress_known && !progressHeld(task)" color="primary" bg-color="secondary" height="5" rounded /><div class="text-caption mt-1">{{ task.progress_known ? Math.round(task.progress) + '%' : progressHeld(task) ? statusPresentation(task).title : '处理中' }}{{ task.current_action ? ' · ' + task.current_action : '' }}</div></template><span v-else-if="task.status === 'completed'" class="text-caption text-success">100%</span><span v-else class="text-caption text-medium-emphasis">—</span></div>
                <div class="queue-task-actions"><VBtn v-if="['queued', 'running'].includes(task.status)" icon="mdi-close-circle-outline" variant="text" size="small" :aria-label="'取消等待任务 ' + taskTitle(task)" :disabled="task.status !== 'queued' || Boolean(queue.actionId)" :loading="queue.actionId === task.id" @click="queue.cancel(task)" /><VBtn v-else-if="['failed', 'interrupted', 'cancelled'].includes(task.status)" icon="mdi-replay" variant="text" size="small" :aria-label="'重试任务 ' + taskTitle(task)" :disabled="Boolean(queue.actionId) || activeItemModes.has(task.item_id + ':' + task.mode)" :loading="queue.actionId === task.id" @click="queue.retry(task)" /></div>
              </article>
              </div>
            </section>
          </div>
        </template>
        <VPagination v-if="pages > 1" :model-value="queue.page" :length="pages" :total-visible="5" :disabled="queue.loading || Boolean(queue.actionId)" class="mt-4" @update:model-value="changePage" />
      </VCardText>
    </VCard>
  </VDialog>
</template>

<script setup>
import { computed, ref, watch } from 'vue';
import { useDisplay } from 'vuetify';

const props = defineProps({ modelValue: Boolean, queue: { type: Object, required: true } });
defineEmits(['update:modelValue']);
const { smAndDown } = useDisplay();
const activeGroupKey = ref('');
const operationLabel = mode => mode === 'rescrape' ? '重新刮削' : '重新整理入库';
const executionState = task => task.status !== 'running' ? task.status
  : task.stop_requested || ['stopping', 'cancelled'].includes(task.job_state) ? 'stopping'
    : task.job_state === 'paused' ? 'paused'
      : task.pause_requested || task.job_state === 'pausing' ? 'pausing' : 'running';
const statusPresentation = task => ({ queued: { title: '等待处理', color: 'secondary' }, running: { title: '正在处理', color: 'primary' }, paused: { title: '已暂停', color: 'warning' }, pausing: { title: '暂停中', color: 'warning' }, stopping: { title: '停止中', color: 'warning' }, completed: { title: '已完成', color: 'success' }, failed: { title: '失败', color: 'error' }, cancelled: { title: '已取消', color: 'warning' }, interrupted: { title: '已中断', color: 'warning' } }[executionState(task)] || { title: '状态未确认', color: 'warning' });
const progressHeld = task => ['paused', 'pausing', 'stopping'].includes(executionState(task));
const statusIcon = task => ({ queued: 'mdi-clock-outline', running: 'mdi-progress-clock', paused: 'mdi-pause-circle-outline', pausing: 'mdi-pause-circle-outline', stopping: 'mdi-stop-circle-outline', completed: 'mdi-check-circle-outline', failed: 'mdi-alert-outline', cancelled: 'mdi-cancel', interrupted: 'mdi-alert-outline' }[executionState(task)] || 'mdi-help-circle-outline');
const taskTitle = task => {
  const item = task.item_json;
  const episode = item.item_type === 'Episode' ? [Number.isInteger(item.season_number) ? 'S' + String(item.season_number).padStart(2, '0') : '', Number.isInteger(item.episode_number) ? 'E' + String(item.episode_number).padStart(2, '0') : ''].join('') : '';
  return item.file_name || [episode, item.name || task.item_id].filter(Boolean).join(' · ');
};
const resultDetails = task => [...(Array.isArray(task.result?.failures) ? task.result.failures : []), ...(Array.isArray(task.result?.warning_details) ? task.result.warning_details : [])].map(detail => typeof detail === 'string' ? detail : JSON.stringify(detail));
const groups = computed(() => {
  const values = new Map();
  for (const task of props.queue.items) {
    const item = task.item_json;
    const series = item.item_type === 'Episode' && item.series_id;
    const key = series ? 'series:' + item.series_id + ':season:' + (item.season_number ?? '') : 'item:' + task.item_id;
    if (!values.has(key)) {
      const title = series ? (item.series_name || item.name || task.item_id) + (Number.isInteger(item.season_number) ? ' S' + String(item.season_number).padStart(2, '0') : '') : item.name || task.item_id;
      const posterId = series && item.series_image_tag ? item.series_id : task.item_id;
      const tag = series && item.series_image_tag ? item.series_image_tag : item.image_tag;
      values.set(key, { key, title, poster: tag ? '/image_proxy/Items/' + encodeURIComponent(posterId) + '/Images/Primary?tag=' + encodeURIComponent(tag) + '&quality=90' : '', tasks: [] });
    }
    values.get(key).tasks.push(task);
  }
  return [...values.values()].map(group => {
    const completed = group.tasks.filter(task => task.status === 'completed').length;
    return { ...group, completed, progress: 100 * completed / group.tasks.length };
  });
});
const activeGroup = computed(() => groups.value.find(group => group.key === activeGroupKey.value));
const activeItemModes = computed(() => new Set(props.queue.items.filter(task => ['queued', 'running'].includes(task.status)).map(task => task.item_id + ':' + task.mode)));
const completedCount = computed(() => props.queue.items.filter(task => task.status === 'completed').length);
const overallProgress = computed(() => props.queue.items.length ? 100 * completedCount.value / props.queue.items.length : 0);
const pages = computed(() => Math.max(1, Math.ceil(props.queue.total / props.queue.pageSize)));
const changePage = page => { props.queue.page = page; void props.queue.refresh(); };
watch(groups, values => { if (!values.some(group => group.key === activeGroupKey.value)) activeGroupKey.value = values[0]?.key || ''; }, { immediate: true });
</script>

<style scoped>
.queue-content { display: flex; flex-direction: column; min-inline-size: 0; min-block-size: 0; overflow: hidden; padding: 1.5rem !important; }
.queue-content--active { block-size: 34rem; }
.queue-content > .v-alert, .queue-overall, .queue-pending-summary { flex: 0 0 auto; }
.queue-empty { min-block-size: 13rem; padding: 2.5rem 1rem; text-align: center; }
.queue-empty > p { font-size: 1.15rem; font-weight: 600; margin-block: 0.75rem; }
.queue-overall { display: grid; align-items: center; border: 1px solid rgba(var(--v-border-color), var(--v-border-opacity)); grid-template-columns: auto auto auto minmax(8rem, 1fr); gap: 1.25rem; padding: 1.15rem 1.25rem; }
.queue-overall-label { font-size: 0.95rem; font-weight: 600; }
.queue-overall-count { color: rgba(var(--v-theme-on-surface), 0.65); font-size: 0.875rem; }
.queue-overall-value { color: rgb(var(--v-theme-primary)); font-size: 1.05rem; font-weight: 700; }
.queue-overall-progress { min-inline-size: 0; }
.queue-pending-summary { margin-block: 0.65rem 1rem; }
.queue-main { display: grid; flex: 1 1 auto; overflow: hidden; min-block-size: 0; grid-template-columns: minmax(14rem, 19rem) minmax(0, 1fr); }
.queue-media-list { display: flex; flex-direction: column; min-inline-size: 0; gap: 0.75rem; border-inline-end: 1px solid rgba(var(--v-border-color), var(--v-border-opacity)); overflow-y: auto; padding: 0.25rem 1rem 0.25rem 0.25rem; scrollbar-width: none; }
.queue-media-button { display: grid; grid-template-columns: 4.25rem minmax(0, 1fr); flex: 0 0 auto; align-items: center; gap: 0.9rem; inline-size: 100%; padding: 0.9rem; border-radius: var(--app-surface-radius); text-align: start; }
.queue-media-button:hover { background: rgba(var(--v-theme-on-surface), var(--v-hover-opacity)); }
.queue-media-button.selected { background: rgba(var(--v-theme-primary), 0.08); }
.queue-media-copy, .queue-task-copy, .queue-task-status, .queue-task-progress { min-inline-size: 0; overflow-wrap: anywhere; }
.queue-media-title, .queue-task-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.queue-media-title { font-size: 0.92rem; line-height: 1.45; }
.queue-media-meta { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; margin-block: 0.65rem 0.4rem; }
.queue-detail { display: flex; flex-direction: column; min-inline-size: 0; min-block-size: 0; overflow: hidden; padding-inline-start: 1.5rem; }
.queue-active-media { flex: 0 0 auto; }
.queue-active-media, .queue-task, .queue-task-header { border-block-end: 1px solid rgba(var(--v-border-color), var(--v-border-opacity)); }
.queue-task-header, .queue-task { display: grid; grid-template-columns: 1.5rem minmax(0, 1fr) 5.5rem 7.5rem 2.5rem; gap: 0.5rem; align-items: center; }
.queue-task-header { flex: 0 0 auto; min-block-size: 3rem; color: rgba(var(--v-theme-on-surface), 0.5); font-size: 0.75rem; }
.queue-task-list { flex: 1 1 auto; min-block-size: 0; overflow-y: auto; scrollbar-width: none; }
.queue-task-list::-webkit-scrollbar, .queue-media-list::-webkit-scrollbar { display: none; }
.queue-task { min-block-size: 4.15rem; padding-block: 1rem; }
.queue-task-name { font-size: 0.8rem; }
.queue-task-status :deep(.v-chip) { max-inline-size: 100%; }
.queue-task-actions { justify-self: end; }
@media (max-width: 959px) {
  .queue-content { block-size: auto; overflow-y: auto; padding: 0.75rem 1rem calc(0.75rem + env(safe-area-inset-bottom)) !important; }
  .queue-overall { grid-template-columns: auto 1fr auto; gap: 0.5rem 0.75rem; padding: 0.75rem 1rem; }
  .queue-overall-count { justify-self: center; }
  .queue-overall-progress { grid-column: 1 / -1; margin-block-start: 0.35rem; }
  .queue-main { display: block; overflow: visible; flex: 0 0 auto; }
  .queue-media-list { flex-direction: row; overflow: auto hidden; border: 0; gap: 0.75rem; margin-inline: -1rem; margin-block-end: 0.75rem; padding: 0.5rem 1rem 1rem; scroll-snap-type: inline mandatory; }
  .queue-media-button { flex: 0 0 13rem; grid-template-columns: 3.25rem minmax(0, 1fr); gap: 0.5rem; padding: 0.5rem; scroll-snap-align: start; }
  .queue-media-button :deep(.v-avatar) { inline-size: 3.25rem !important; block-size: 4.875rem !important; }
  .queue-media-title { display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; white-space: normal; }
  .queue-media-meta { font-size: 0.68rem; }
  .queue-detail { overflow: visible; padding-inline-start: 0; }
  .queue-task-header { display: none; }
  .queue-task-list { overflow: visible; flex: 0 0 auto; }
  .queue-task { grid-template-columns: 1.5rem minmax(0, 1fr) 2.5rem; align-items: start; gap: 0.75rem; }
  .queue-task-state-icon { grid-column: 1; grid-row: 1 / 4; margin-block-start: 0.15rem; }
  .queue-task-copy, .queue-task-status, .queue-task-progress { grid-column: 2; }
  .queue-task-name { font-size: 0.9rem; }
  .queue-task-actions { grid-column: 3; grid-row: 1 / 4; }
}
</style>
