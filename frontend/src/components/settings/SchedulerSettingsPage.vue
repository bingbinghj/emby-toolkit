<template>
  <section class="mp-page scheduler-page">
    <h1 class="sr-only">任务中心</h1>
    <VCard v-if="backgroundTaskStatus.task_id" class="dashboard-card current-task-card mb-6" data-testid="scheduler-current-task">
      <VCardItem :title="backgroundTaskStatus.last_action || backgroundTaskStatus.current_action || '当前任务'">
        <template #append><VChip :color="backgroundTaskStatus.state === 'failed' ? 'error' : backgroundTaskStatus.state === 'completed_with_warnings' ? 'warning' : 'primary'" variant="tonal">{{ currentTaskStateLabel }}</VChip></template>
      </VCardItem>
      <VCardText>
        <VProgressLinear v-if="backgroundTaskStatus.is_running" :model-value="backgroundTaskStatus.progress" :indeterminate="!backgroundTaskStatus.progress_known" color="primary" height="4" rounded />
        <div class="task-status-message mt-3">{{ backgroundTaskStatus.message }}</div>
        <div v-if="backgroundTaskStatus.progress_known && backgroundTaskStatus.is_running" class="text-caption mt-2">{{ backgroundTaskStatus.progress }}%</div>
      </VCardText>
      <VCardActions v-if="backgroundTaskStatus.is_running">
        <VSpacer />
        <VBtn variant="text" :prepend-icon="taskPaused ? 'mdi-play' : 'mdi-pause'" :loading="taskControlBusy" :disabled="taskStopping || backgroundTaskStatus.state === 'pausing'" @click="controlTask(taskPaused ? 'resume' : 'pause')">{{ taskPaused ? '继续' : backgroundTaskStatus.state === 'pausing' ? '暂停中' : '暂停' }}</VBtn>
        <VBtn color="error" variant="text" prepend-icon="mdi-stop" :disabled="taskStopping || taskControlBusy" @click="confirmStopTask">{{ taskStopping ? '停止中' : '停止' }}</VBtn>
      </VCardActions>
    </VCard>
    <div v-if="isLoading" class="center-container"><VProgressCircular indeterminate color="primary" size="48" /></div>
    <VAlert v-else-if="configError" type="error" variant="tonal">{{ configError }}</VAlert>
    <div v-else-if="configModel" class="scheduler-content">
      <div class="scheduler-overview">
        <VCard class="dashboard-card">
          <VCardItem title="Telegram 快捷菜单" subtitle="配置机器人 Menu 按钮中的快捷任务">
            <template #prepend><VIcon icon="mdi-telegram" color="primary" /></template>
          </VCardItem>
          <VCardText>
            <VBtn variant="tonal" color="primary" prepend-icon="mdi-cog-outline" @click="showTgMenuConfigModal = true">配置 TG 菜单任务</VBtn>
            <p class="text-body-2 font-weight-medium mt-6 mb-3">当前 TG 菜单列表</p>
            <div v-if="enabledTgMenuTasks.length" class="d-flex flex-wrap ga-2">
              <VTooltip v-for="task in enabledTgMenuTasks" :key="task.key" location="top">
                <template #activator="{ props: activatorProps }"><VChip v-bind="activatorProps" color="primary" variant="tonal">{{ task.name }}</VChip></template>
                <div class="task-tooltip-content">{{ getTaskHelp(task) }}</div>
              </VTooltip>
            </div>
            <p v-else class="text-body-2 text-medium-emphasis">暂未配置任何 TG 菜单任务</p>
          </VCardText>
        </VCard>
        <FingerprintAuditPanel class="fingerprint-overview-card" :running="isBackgroundTaskRunning" :task-state="backgroundTaskStatus.state" />
      </div>
      <VAlert v-if="taskRunFeedback" :type="taskRunFeedback.type" variant="tonal" :title="taskRunFeedback.message" role="status">
        <div v-for="detail in taskRunFeedback.details" :key="detail">{{ detail }}</div>
      </VAlert>
      <VCard class="dashboard-card manual-tasks-card d-none d-md-block" data-testid="scheduler-desktop-tasks">
        <VTable v-if="filteredManualTasks.length" class="scheduler-table">
          <thead><tr><th scope="col">分类</th><th scope="col">任务名称</th><th scope="col">任务状态</th><th scope="col" /></tr></thead>
          <tbody>
            <tr v-for="task in filteredManualTasks" :key="task.key" class="task-record" :data-task-key="task.key">
              <td class="scheduler-category-cell">{{ task.category_name || '其他任务' }}</td>
              <td class="scheduler-task-cell">
                <div class="temp-task-name">{{ task.name }}</div>
                <p class="temp-task-description">{{ getTaskHelp(task) }}</p>
                <div v-if="isCurrentTask(task) && isBackgroundTaskRunning" class="scheduler-progress">
                  <VProgressLinear :model-value="backgroundTaskStatus.progress" :indeterminate="!backgroundTaskStatus.progress_known" color="primary" height="4" rounded />
                  <div class="scheduler-progress-meta"><span>{{ backgroundTaskStatus.message }}</span><strong v-if="backgroundTaskStatus.progress_known">{{ backgroundTaskStatus.progress }}%</strong></div>
                </div>
              </td>
              <td><VChip :color="getTaskRowColor(task)">{{ getTaskRowState(task) }}</VChip></td>
              <td>
                <VBtn size="small" prepend-icon="mdi-play" :loading="isTriggeringTask === task.key" :disabled="isBackgroundTaskRunning || Boolean(isTriggeringTask)" @click="triggerTaskNow(task.key)">执行</VBtn>
              </td>
            </tr>
          </tbody>
        </VTable>
        <VEmptyState v-else :title="taskSearchQuery.trim() ? '未找到匹配任务' : '暂无可用任务'" icon="mdi-timer-off-outline" />
      </VCard>
      <div class="mobile-scheduler-view d-md-none" data-testid="scheduler-mobile-tasks">
        <div v-if="filteredManualTasks.length" class="mobile-scheduler-list">
          <article v-for="task in filteredManualTasks" :key="task.key" class="mobile-scheduler-card task-record" :data-task-key="task.key">
            <div class="mobile-scheduler-icon"><VIcon :icon="getTaskIcon(task)" size="30" /></div>
            <div class="mobile-scheduler-content">
              <h3 class="temp-task-name">{{ task.name }}</h3>
              <div class="mobile-scheduler-meta"><p>{{ task.category_name || '其他任务' }}</p><span class="mobile-scheduler-status" :class="{ 'mobile-scheduler-status--running': isCurrentTask(task) && isBackgroundTaskRunning }">{{ getTaskRowState(task) }}</span></div>
            </div>
            <div class="mobile-scheduler-actions"><VBtn icon class="mobile-scheduler-run-btn" aria-label="执行" :loading="isTriggeringTask === task.key" :disabled="isBackgroundTaskRunning || Boolean(isTriggeringTask)" @click="triggerTaskNow(task.key)"><VIcon icon="mdi-play" size="24" /></VBtn></div>
            <p class="temp-task-description mobile-scheduler-description">{{ getTaskHelp(task) }}</p>
            <div v-if="isCurrentTask(task) && isBackgroundTaskRunning" class="mobile-scheduler-progress">
              <VProgressLinear :model-value="backgroundTaskStatus.progress" :indeterminate="!backgroundTaskStatus.progress_known" color="primary" height="4" rounded />
              <div class="scheduler-progress-meta"><span>{{ backgroundTaskStatus.message }}</span><strong v-if="backgroundTaskStatus.progress_known">{{ backgroundTaskStatus.progress }}%</strong></div>
            </div>
          </article>
        </div>
        <VEmptyState v-else :title="taskSearchQuery.trim() ? '未找到匹配任务' : '暂无可用任务'" icon="mdi-timer-off-outline" />
      </div>
    </div>
    <VDialog v-model="showTgMenuConfigModal" max-width="600" persistent>
      <VCard title="配置 Telegram 快捷菜单">
        <VCardText>
          <VAlert type="info" variant="tonal" class="mb-4">勾选需要显示的任务，并拖动调整顺序。</VAlert>
          <div ref="draggableContainerTgMenu" class="task-chain-list">
            <div v-for="task in configuredTgMenuSequence" :key="task.key" class="task-chain-item" :data-key="task.key">
              <VIcon icon="mdi-drag" class="drag-handle" aria-label="拖动调整任务顺序" />
              <VCheckbox v-model="task.enabled" :label="task.name" hide-details density="compact" class="flex-grow-1" />
              <VTooltip :text="getTaskHelp(task)" location="bottom"><template #activator="{ props: activatorProps }"><VIcon v-bind="activatorProps" icon="mdi-information-outline" size="18" class="text-medium-emphasis" /></template></VTooltip>
            </div>
          </div>
        </VCardText>
        <VCardActions><VSpacer /><VBtn variant="text" @click="showTgMenuConfigModal = false">取消</VBtn><VBtn color="default" variant="text" prepend-icon="mdi-content-save-outline" @click="saveTgMenuConfig">保存</VBtn></VCardActions>
      </VCard>
    </VDialog>
    <VDialog v-model="showSyncModeModal" max-width="560" persistent>
      <VCard title="选择处理模式">
        <VCardText>您希望如何执行此任务？</VCardText>
        <VCardActions class="sync-mode-actions">
          <VBtn variant="text" @click="showSyncModeModal = false">取消</VBtn>
          <VBtn color="primary" variant="tonal" prepend-icon="mdi-play" @click="runTaskFromModal(false)">快速模式（增量）</VBtn>
          <VBtn color="warning" variant="tonal" prepend-icon="mdi-play" @click="runTaskFromModal(true)">深度模式（全量）</VBtn>
        </VCardActions>
      </VCard>
    </VDialog>
  </section>
</template>
<script setup>
import { ref, onMounted, onBeforeUnmount, watch, nextTick, computed } from 'vue';
import { useMessage, useDialog } from '@/composables/useFeedback';
import { useConfig } from '../../composables/useConfig.js';
import { useTaskStatus } from '../../composables/useTaskStatus.js';
import axios from 'axios';
import FingerprintAuditPanel from './FingerprintAuditPanel.vue';
import Sortable from 'sortablejs';
import { useContextSearch } from '../../composables/useContextSearch.js';

const message = useMessage();
const dialog = useDialog();

const DUAL_MODE_TASKS = [
  'role-translation',
  'populate-metadata',
  'enrich-aliases',
  'restore_mediainfo'
];

const DEFAULT_TG_TASKS = [
  'populate-metadata',
  'enrich-aliases',
  'role-translation',
  'actor-translation',
  'clear-mediainfo',
  'clear-emby-activity-log',
  'backup-mediainfo',
  'repair-p115-fingerprints',
  'restore_mediainfo',
  'restore-truehd-core-backups',
  'clear-truehd-core-backups',
  'restore-nfo-and-images',
  'purge-ghost-actors'
];

const {
  configModel,
  loadingConfig: isLoading,
  handleSaveConfig,
  configError
} = useConfig();

const { isBackgroundTaskRunning, backgroundTaskStatus, refreshTaskStatus } = useTaskStatus();
const taskControlBusy = ref(false);
const taskPaused = computed(() => backgroundTaskStatus.value.state === 'paused');
const taskStopping = computed(() => backgroundTaskStatus.value.stop_requested || backgroundTaskStatus.value.state === 'stopping');
const currentTaskStateLabel = computed(() => ({
  running: '运行中', pausing: '暂停中', paused: '已暂停', stopping: '停止中', cancelled: '已停止',
  completed: '已完成', completed_with_warnings: '已完成，有提醒', failed: '失败',
}[backgroundTaskStatus.value.state] || backgroundTaskStatus.value.state || '等待任务'));
const controlTask = async (action) => {
  const taskId = backgroundTaskStatus.value.task_id;
  if (!taskId || !backgroundTaskStatus.value.is_running || taskControlBusy.value || taskStopping.value) return;
  taskControlBusy.value = true;
  try {
    const endpoint = action === 'stop' ? '/api/trigger_stop_task' : `/api/tasks/${action}`;
    const { data } = await axios.post(endpoint, { task_id: taskId });
    message.info(data.message || '任务控制请求已发送');
    await refreshTaskStatus(true);
  } catch (error) {
    message.error(error.response?.data?.error || error.message || '任务控制失败');
  } finally {
    taskControlBusy.value = false;
  }
};
const confirmStopTask = () => {
  const taskId = backgroundTaskStatus.value.task_id;
  if (!taskId || !backgroundTaskStatus.value.is_running || taskStopping.value) return;
  dialog.warning({ title: '停止当前任务',
    content: `确定停止“${backgroundTaskStatus.value.current_action || '当前任务'}”吗？任务将在安全停止点结束，并保留恢复和清理结果。`,
    positiveText: '停止任务', negativeText: '取消', onPositiveClick: () => {
      if (backgroundTaskStatus.value.task_id !== taskId || !backgroundTaskStatus.value.is_running) {
        message.warning('当前任务已变化，未执行停止操作。');
        return;
      }
      return controlTask('stop');
    },
  });
};

const availableTasksForManualRun = ref([]);
const availableTasksForTgMenu = ref([]);
const taskSearchQuery = ref('');
useContextSearch({
  label: '搜索任务', query: taskSearchQuery,
  clear: () => { taskSearchQuery.value = ''; },
});
const isTriggeringTask = ref(null);
const taskRunFeedback = ref(null);
const showTgMenuConfigModal = ref(false);
const configuredTgMenuSequence = ref([]);
const draggableContainerTgMenu = ref(null);
let sortableInstanceTgMenu = null;
const showSyncModeModal = ref(false);
const taskToRunInModal = ref(null);

const enabledTgMenuTasks = computed(() => {
  if (!configuredTgMenuSequence.value) return [];
  return configuredTgMenuSequence.value.filter(t => t.enabled);
});

const filteredManualTasks = computed(() => {
  const query = taskSearchQuery.value.trim().toLocaleLowerCase();
  return availableTasksForManualRun.value.filter(task => !query || [task.name, task.help, task.description, task.category_name, task.key]
    .some(value => String(value || '').toLocaleLowerCase().includes(query)));
});
const isCurrentTask = (task) => Boolean(backgroundTaskStatus.value.task_id && backgroundTaskStatus.value.last_action === task.name);
const getTaskRowState = (task) => {
  if (isTriggeringTask.value === task.key && !backgroundTaskStatus.value.is_running) return '提交中';
  if (isCurrentTask(task)) return currentTaskStateLabel.value;
  return isTriggeringTask.value === task.key ? '提交中' : '待执行';
};
const getTaskRowColor = (task) => {
  if (!isCurrentTask(task) || (isTriggeringTask.value === task.key && !backgroundTaskStatus.value.is_running)) return '';
  return { running: 'success', failed: 'error', completed_with_warnings: 'warning', paused: 'warning', pausing: 'warning' }[backgroundTaskStatus.value.state] || '';
};
const getTaskIcon = (task) => ({
  'media-data': 'mdi-database-sync-outline', 'media-info': 'mdi-file-document-outline',
  'p115-cache': 'mdi-cloud-outline', 'audio-maintenance': 'mdi-volume-high',
}[task.category] || 'mdi-cog-outline');

const getTaskHelp = (task) => {
  if (!task) return '暂无任务说明。';
  return task.help || task.description || task.name || '暂无任务说明。';
};

const fetchAvailableTasks = async () => {
  try {
    const manualResponse = await axios.get('/api/tasks/available?context=manual');
    availableTasksForManualRun.value = manualResponse.data;

    const allResponse = await axios.get('/api/tasks/available?context=all');
    availableTasksForTgMenu.value = allResponse.data;
  } catch (error) {
    message.error('获取可用任务列表失败。');
  }
};

const showTaskRunResult = (response) => {
  const result = response.data || {};
  const details = [...new Set([
    ...(Array.isArray(result.failures) ? result.failures : []),
    ...(Array.isArray(result.warning_details) ? result.warning_details : []),
    ...(Array.isArray(result.items) ? result.items.filter(item => item.status === 'skipped' && !item.chapter && item.error) : []),
  ].map(value => typeof value === 'string' ? value : [value?.item_id || value?.path, value?.reason || value?.error || value?.message].filter(Boolean).join('：')).filter(Boolean))];
  const hasWarnings = response.status === 207 || Number(result.warnings) > 0 || Number(result.failed) > 0 || details.length > 0;
  const summary = result.message || (hasWarnings ? '任务处理有提醒，请检查结果。' : '任务已完成。');
  taskRunFeedback.value = hasWarnings ? { type: 'warning', message: summary, details } : null;
  if (hasWarnings) message.warning(summary);
  else message.success(summary);
};

const runTaskFromModal = async (isDeepMode) => {
  showSyncModeModal.value = false;
  const taskIdentifier = taskToRunInModal.value;
  if (!taskIdentifier) return;

  isTriggeringTask.value = taskIdentifier;
  taskRunFeedback.value = null;
  try {
    const payload = { task_name: taskIdentifier, force_full_update: isDeepMode };
    const response = await axios.post('/api/tasks/run', payload);
    showTaskRunResult(response);
  } catch (error) {
    message.error(error.response?.data?.error || '请求后端接口失败。');
  } finally {
    isTriggeringTask.value = null;
    taskToRunInModal.value = null;
  }
};

const triggerTaskNow = async (taskIdentifier) => {
  if (isBackgroundTaskRunning.value || isTriggeringTask.value) {
    message.warning('已有后台任务正在运行，请稍后再试。');
    return;
  }

  if (DUAL_MODE_TASKS.includes(taskIdentifier)) {
    taskToRunInModal.value = taskIdentifier;
    showSyncModeModal.value = true;
    return;
  }

  isTriggeringTask.value = taskIdentifier;
  taskRunFeedback.value = null;
  try {
    const response = await axios.post('/api/tasks/run', { task_name: taskIdentifier });
    showTaskRunResult(response);
  } catch (error) {
    message.error(error.response?.data?.error || '请求后端接口失败。');
  } finally {
    isTriggeringTask.value = null;
  }
};

const saveTgMenuConfig = async () => {
  const payload = { tg_menu_tasks: enabledTgMenuTasks.value.map(t => t.key) };
  if (configModel.value) {
    configModel.value.tg_menu_tasks = payload.tg_menu_tasks;
  }
  const success = await handleSaveConfig(payload);
  showTgMenuConfigModal.value = false;
  if (success) {
    message.success('TG 菜单配置已保存。');
  } else {
    message.error(configError.value || 'TG 菜单配置保存失败。');
  }
};

const initializeSequence = (savedSequenceKeys, targetConfiguredSequence, sourceTasks) => {
  if (!sourceTasks || sourceTasks.length === 0) return;

  const savedSequenceSet = new Set(savedSequenceKeys);
  const enabledTasks = savedSequenceKeys
    .map(key => {
      const task = sourceTasks.find(t => t.key === key);
      return task ? { ...task, enabled: true } : null;
    })
    .filter(Boolean);

  const disabledTasks = sourceTasks
    .filter(task => !savedSequenceSet.has(task.key))
    .map(task => ({ ...task, enabled: false }));

  targetConfiguredSequence.value = [...enabledTasks, ...disabledTasks];
};

const initializeSortable = (container, sequenceRef) => {
  if (!container) return null;
  return Sortable.create(container, {
    animation: 150,
    handle: '.drag-handle',
    onEnd: (evt) => {
      const { oldIndex, newIndex } = evt;
      const item = sequenceRef.value.splice(oldIndex, 1)[0];
      sequenceRef.value.splice(newIndex, 0, item);
    },
  });
};

onMounted(() => {
  fetchAvailableTasks();
});

watch([showTgMenuConfigModal, draggableContainerTgMenu], ([visible, container]) => {
  if (visible && container && !sortableInstanceTgMenu) {
    nextTick(() => {
      if (showTgMenuConfigModal.value && draggableContainerTgMenu.value && !sortableInstanceTgMenu) {
        sortableInstanceTgMenu = initializeSortable(draggableContainerTgMenu.value, configuredTgMenuSequence);
      }
    });
  } else if (!visible && sortableInstanceTgMenu) {
    sortableInstanceTgMenu.destroy();
    sortableInstanceTgMenu = null;
  }
});
onBeforeUnmount(() => sortableInstanceTgMenu?.destroy());

watch([configModel, availableTasksForTgMenu], ([newConfig, tgMenuTasks]) => {
  if (newConfig && tgMenuTasks.length > 0) {
    initializeSequence(newConfig.tg_menu_tasks || DEFAULT_TG_TASKS, configuredTgMenuSequence, tgMenuTasks);
  }
}, { immediate: true, deep: true });
</script>

<style scoped>
.scheduler-content { display: grid; gap: 24px; }
.scheduler-overview { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 24px; align-items: stretch; }
.scheduler-overview > * { min-width: 0; }
.center-container { display: flex; justify-content: center; align-items: center; min-height: 320px; }
.scheduler-page :deep(.v-card-title), .scheduler-page :deep(.v-card-subtitle) { white-space: normal; overflow-wrap: anywhere; }
.scheduler-category-cell { white-space: nowrap; }
.scheduler-task-cell { min-inline-size: 220px; padding-block: 16px !important; }
.temp-task-name { line-height: 1.5; overflow-wrap: anywhere; }
.temp-task-description { margin: 6px 0 0; font-size: 12px; line-height: 1.6; color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity)); overflow-wrap: anywhere; white-space: normal; }
.task-status-message { white-space: pre-wrap; overflow-wrap: anywhere; }
.scheduler-progress { margin-block-start: 8px; max-inline-size: 320px; }
.scheduler-progress-meta { display: flex; justify-content: space-between; color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity)); font-size: 11px; gap: 12px; margin-block-start: 4px; }
.scheduler-progress-meta span { min-inline-size: 0; overflow-wrap: anywhere; white-space: normal; }
.scheduler-progress-meta strong { flex: 0 0 auto; font-variant-numeric: tabular-nums; }
.mobile-scheduler-view { --mobile-scheduler-card-backdrop-filter: none; --mobile-scheduler-card-background: rgb(var(--v-theme-surface)); --mobile-scheduler-card-border: 0; --mobile-scheduler-card-shadow: none; min-block-size: 100%; padding-block: 12px calc(22px + env(safe-area-inset-bottom)); }
.mobile-scheduler-list { display: flex; flex-direction: column; gap: 18px; }
.mobile-scheduler-card { display: grid; align-items: center; padding: 18px; border: var(--mobile-scheduler-card-border); border-radius: var(--app-surface-radius); -webkit-backdrop-filter: var(--mobile-scheduler-card-backdrop-filter); backdrop-filter: var(--mobile-scheduler-card-backdrop-filter); background: var(--mobile-scheduler-card-background); box-shadow: var(--mobile-scheduler-card-shadow); column-gap: 14px; grid-template-columns: 62px minmax(0, 1fr) auto; }
.mobile-scheduler-icon { display: inline-flex; align-items: center; justify-content: center; border-radius: 50%; background: rgba(var(--v-theme-primary), .14); block-size: 58px; color: rgb(var(--v-theme-primary)); inline-size: 58px; }
.mobile-scheduler-content { min-inline-size: 0; }
.mobile-scheduler-content h3 { margin: 0; color: rgba(var(--v-theme-on-surface), .92); font-size: 18px; font-weight: 700; letter-spacing: .01em; line-height: 1.35; white-space: normal; }
.mobile-scheduler-meta { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 8px; margin-block-start: 6px; min-inline-size: 0; }
.mobile-scheduler-meta p { margin: 0; color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity)); font-size: 14px; font-weight: 500; line-height: 1.4; white-space: normal; }
.mobile-scheduler-status { display: inline-flex; align-items: center; justify-content: center; border-radius: 999px; background: rgba(var(--v-theme-on-surface), .06); color: rgba(var(--v-theme-on-surface), .92); font-size: 12px; font-weight: 600; line-height: 1.3; padding-block: 5px; padding-inline: 10px; white-space: nowrap; }
.mobile-scheduler-status--running { background: rgba(var(--v-theme-success), .14); }
.mobile-scheduler-actions { display: inline-flex; align-items: center; }
.mobile-scheduler-run-btn { border-radius: 50%; background: linear-gradient(135deg, #ff4f87, #e91e63) !important; block-size: 46px; box-shadow: 0 10px 22px rgba(233, 30, 99, .28); color: #fff !important; inline-size: 46px; }
.mobile-scheduler-run-btn.v-btn--disabled { background: rgba(var(--v-theme-on-surface), .12) !important; box-shadow: none; color: rgba(var(--v-theme-on-surface), .42) !important; }
.mobile-scheduler-description { grid-column: 2 / -1; margin-block-start: 10px; }
.mobile-scheduler-progress { grid-column: 2 / -1; margin-block-start: 8px; min-inline-size: 0; }
.task-chain-list { display: grid; gap: 8px; }
.task-chain-item { display: flex; align-items: center; gap: 12px; padding: 8px 12px; background: rgba(var(--v-theme-on-surface), .04); border: 1px solid rgba(var(--v-theme-on-surface), .12); border-radius: 8px; }
.task-chain-item :deep(.v-label) { white-space: normal; overflow-wrap: anywhere; }
.task-chain-item.sortable-ghost { background: rgba(var(--v-theme-primary), .12); }
.drag-handle { cursor: grab; touch-action: none; }
.drag-handle:active { cursor: grabbing; }
.task-tooltip-content { max-width: 320px; line-height: 1.6; white-space: normal; }
.sync-mode-actions { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: 8px; padding: 16px; }
@media (max-width: 899px) { .scheduler-overview { grid-template-columns: minmax(0, 1fr); } }
@media (max-width: 540px) { .sync-mode-actions { flex-direction: column; align-items: stretch; } }
@media (max-width: 480px) {
  .mobile-scheduler-card { padding: 16px; column-gap: 12px; grid-template-columns: 54px minmax(0, 1fr) auto; }
  .mobile-scheduler-icon { block-size: 50px; inline-size: 50px; }
  .mobile-scheduler-content h3 { font-size: 16px; }
  .mobile-scheduler-meta p { font-size: 13px; }
  .mobile-scheduler-status { padding-inline: 9px; }
  .mobile-scheduler-run-btn { block-size: 42px; inline-size: 42px; }
}
</style>
