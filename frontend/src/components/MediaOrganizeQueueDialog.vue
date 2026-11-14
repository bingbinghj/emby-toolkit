<template>
  <VDialog :model-value="modelValue" :fullscreen="smAndDown" max-width="1180" scrollable @update:model-value="$emit('update:modelValue', $event)">
    <VCard data-testid="organize-queue-dialog">
      <VCardTitle class="d-flex align-center ga-2"><span class="flex-grow-1">整理队列</span><VBtn icon="mdi-refresh" variant="text" aria-label="刷新整理队列" :loading="queue.loading" @click="queue.refresh" /><VBtn icon="mdi-close" variant="text" aria-label="关闭整理队列" @click="$emit('update:modelValue', false)" /></VCardTitle>
      <VCardText class="queue-content">
        <VAlert v-if="queue.error" type="warning" variant="tonal" class="mb-3" data-testid="organize-queue-error">{{ queue.error }}<VBtn variant="text" :loading="queue.loading" @click="queue.refresh">重新读取</VBtn></VAlert>
        <VAlert v-if="queue.actionError || queue.actionMessage" :type="queue.actionError ? 'error' : 'info'" variant="tonal" class="mb-3" data-testid="organize-queue-action-result">{{ queue.actionError || queue.actionMessage }}</VAlert>
        <VProgressLinear v-if="queue.loading && !queue.loaded" indeterminate />
        <div v-if="!queue.items.length && !queue.loading && !queue.error" class="queue-empty"><VIcon icon="mdi-timer-sand-paused" size="36" /><p>当前没有整理任务</p><div class="text-medium-emphasis">选择媒体后，可加入重新整理入库或重新刮削任务。</div></div>
        <template v-if="queue.items.length">
          <div class="queue-overall mb-4">
            <span>等待 {{ queue.queuedCount }} 项 · 正在处理 {{ queue.runningCount }} 项</span>
            <span>本页已完成 {{ completedCount }} / {{ queue.items.length }}</span>
            <VProgressLinear :model-value="100 * completedCount / queue.items.length" color="primary" height="6" rounded />
          </div>
          <div class="queue-main">
            <nav class="queue-media-list" aria-label="队列媒体列表">
              <button v-for="group in groups" :key="group.key" type="button" class="queue-media-button" :class="{ selected: group.key === activeGroupKey }" :aria-current="group.key === activeGroupKey ? 'true' : undefined" @click="activeGroupKey = group.key">
                <VAvatar rounded width="48" height="72"><VImg v-if="group.poster" :src="group.poster" :alt="group.title" cover /><VIcon v-else icon="mdi-movie-open-outline" /></VAvatar>
                <div class="queue-media-copy"><div class="font-weight-medium">{{ group.title }}</div><div class="text-caption text-medium-emphasis">已完成 {{ group.tasks.filter(task => task.status === 'completed').length }} / {{ group.tasks.length }}</div></div>
              </button>
            </nav>
            <section v-if="activeGroup" class="queue-detail">
              <div class="d-flex align-center ga-3 pb-4 mb-2 queue-active-media"><VAvatar rounded width="52" height="78"><VImg v-if="activeGroup.poster" :src="activeGroup.poster" :alt="activeGroup.title" cover /><VIcon v-else icon="mdi-movie-open-outline" /></VAvatar><div class="queue-media-copy"><div class="text-h6">{{ activeGroup.title }}</div><div class="text-body-2 text-medium-emphasis">{{ activeGroup.tasks.length }} 项任务</div></div></div>
              <div class="queue-task-header"><span>媒体文件</span><span>状态与进度</span><span>操作</span></div>
              <article v-for="task in activeGroup.tasks" :key="task.id" class="queue-task" :data-testid="'organize-queue-task-' + task.id">
                <div class="queue-task-copy"><div class="text-wrap font-weight-medium">{{ taskTitle(task) }}</div><div class="text-caption text-medium-emphasis mt-1">{{ operationLabel(task.mode) }}</div><div v-if="task.message" class="text-body-2 text-wrap mt-2">{{ task.message }}</div><div v-for="(detail, index) in resultDetails(task)" :key="index" class="text-caption text-wrap mt-1">{{ detail }}</div></div>
                <div class="queue-task-status"><VChip size="small" variant="tonal" :color="statusPresentation(task).color">{{ statusPresentation(task).title }}</VChip><div v-if="task.status === 'running'" class="mt-2"><VProgressLinear v-if="task.progress_known || !progressHeld(task)" :model-value="task.progress_known ? task.progress : undefined" :indeterminate="!task.progress_known && !progressHeld(task)" color="primary" height="5" /><div class="text-caption mt-1">{{ task.progress_known ? Math.round(task.progress) + '%' : progressHeld(task) ? statusPresentation(task).title : '处理中' }}{{ task.current_action ? ' · ' + task.current_action : '' }}</div></div><div v-else-if="task.status === 'completed'" class="text-caption text-success mt-2">已完成</div></div>
                <div class="queue-task-actions"><VBtn v-if="['queued', 'running'].includes(task.status)" icon="mdi-close-circle-outline" variant="text" size="small" :aria-label="'取消等待任务 ' + taskTitle(task)" :disabled="task.status !== 'queued' || Boolean(queue.actionId)" :loading="queue.actionId === task.id" @click="queue.cancel(task)" /><VBtn v-else-if="['failed', 'interrupted', 'cancelled'].includes(task.status)" icon="mdi-replay" variant="text" size="small" :aria-label="'重试任务 ' + taskTitle(task)" :disabled="Boolean(queue.actionId) || activeItemModes.has(task.item_id + ':' + task.mode)" :loading="queue.actionId === task.id" @click="queue.retry(task)" /></div>
              </article>
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
  return [...values.values()];
});
const activeGroup = computed(() => groups.value.find(group => group.key === activeGroupKey.value));
const activeItemModes = computed(() => new Set(props.queue.items.filter(task => ['queued', 'running'].includes(task.status)).map(task => task.item_id + ':' + task.mode)));
const completedCount = computed(() => props.queue.items.filter(task => task.status === 'completed').length);
const pages = computed(() => Math.max(1, Math.ceil(props.queue.total / props.queue.pageSize)));
const changePage = page => { props.queue.page = page; void props.queue.refresh(); };
watch(groups, values => { if (!values.some(group => group.key === activeGroupKey.value)) activeGroupKey.value = values[0]?.key || ''; }, { immediate: true });
</script>

<style scoped>
.queue-content { min-width: 0; }
.queue-empty { padding: 48px 16px; text-align: center; }
.queue-overall { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 12px; }
.queue-main { display: grid; grid-template-columns: minmax(200px, 28%) minmax(0, 1fr); gap: 20px; }
.queue-media-list { min-width: 0; border-right: 1px solid rgba(var(--v-border-color), var(--v-border-opacity)); padding-right: 12px; }
.queue-media-button { display: flex; align-items: center; gap: 12px; width: 100%; padding: 12px 8px; border-radius: 8px; text-align: left; }
.queue-media-button.selected { background: rgba(var(--v-theme-primary), 0.1); }
.queue-media-copy, .queue-detail, .queue-task-copy, .queue-task-status { min-width: 0; overflow-wrap: anywhere; }
.queue-active-media, .queue-task { border-bottom: 1px solid rgba(var(--v-border-color), var(--v-border-opacity)); }
.queue-task-header, .queue-task { display: grid; grid-template-columns: minmax(0, 1fr) minmax(125px, 32%) 40px; gap: 12px; align-items: start; }
.queue-task-header { padding: 8px 0; color: rgba(var(--v-theme-on-surface), 0.6); font-size: 12px; }
.queue-task { padding: 16px 0; }
@media (max-width: 959px) {
  .queue-main { display: block; }
  .queue-media-list { display: flex; overflow-x: auto; border: 0; padding-right: 0; padding-bottom: 12px; margin-bottom: 12px; }
  .queue-media-button { flex: 0 0 220px; }
}
@media (max-width: 540px) {
  .queue-task-header { display: none; }
  .queue-task { grid-template-columns: minmax(0, 1fr) 40px; }
  .queue-task-copy { grid-column: 1; }
  .queue-task-status { grid-column: 1; }
  .queue-task-actions { grid-column: 2; grid-row: 1 / 3; }
}
</style>
