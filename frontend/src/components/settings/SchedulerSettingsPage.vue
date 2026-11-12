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
    <VAlert v-else-if="configError && !configModel" type="error" variant="tonal">{{ configError }}</VAlert>
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
      <div class="scheduler-category-list" :data-testid="mdAndUp ? 'scheduler-desktop-tasks' : 'scheduler-mobile-tasks'">
        <VCard v-for="group in manualTaskGroups" :key="group.name" class="dashboard-card manual-tasks-card scheduler-category-card" :data-task-category="group.name">
          <VCardItem :title="group.name"><template #prepend><VIcon :icon="getTaskIcon(group.tasks[0])" color="primary" /></template></VCardItem>
          <VCardText>
            <Draggable :list="group.tasks" item-key="key" tag="div" class="scheduler-task-grid"
              handle=".task-drag-handle" :group="manualTaskDragGroup"
              :animation="150" :delay="150" :delay-on-touch-only="true" :force-fallback="true" :fallback-on-body="true"
              @change="reorderManualTaskGroup(group)">
              <template #item="{ element: task }">
                <VCard variant="tonal" class="app-card-shell app-card-colorful scheduler-task-card task-record"
                  :class="{ 'mobile-scheduler-card': !mdAndUp }" :data-task-key="task.key">
                  <VBtn icon="mdi-drag" variant="text" size="small" class="app-card-top-action task-drag-handle"
                    :aria-label="`拖动${task.name}调整顺序`" @click.stop />
                  <VCardText class="app-card-summary scheduler-task-summary">
                    <div class="app-card-summary__content">
                      <h3 class="temp-task-name text-h6">{{ task.name }}</h3>
                      <VChip :color="getTaskRowColor(task)" variant="tonal" size="small" class="align-self-start mt-2">{{ getTaskRowState(task) }}</VChip>
                    </div>
                    <div class="app-card-summary__media" aria-hidden="true"><VIcon :icon="getTaskIcon(task)" color="primary" size="48" /></div>
                  </VCardText>
                  <VCardText class="scheduler-task-details">
                    <p class="temp-task-description">{{ getTaskHelp(task) }}</p>
                    <div v-if="isCurrentTask(task) && isBackgroundTaskRunning" class="scheduler-progress">
                      <VProgressLinear :model-value="backgroundTaskStatus.progress" :indeterminate="!backgroundTaskStatus.progress_known" color="primary" height="4" rounded />
                      <div class="scheduler-progress-meta"><span>{{ backgroundTaskStatus.message }}</span><strong v-if="backgroundTaskStatus.progress_known">{{ backgroundTaskStatus.progress }}%</strong></div>
                    </div>
                  </VCardText>
                  <VCardActions><VBtn color="primary" variant="flat" size="small" prepend-icon="mdi-play"
                    :loading="isTriggeringTask === task.key" :disabled="isBackgroundTaskRunning || Boolean(isTriggeringTask)"
                    @click="triggerTaskNow(task.key)">执行</VBtn></VCardActions>
                </VCard>
              </template>
            </Draggable>
          </VCardText>
        </VCard>
        <VEmptyState v-if="!manualTaskGroups.length" :title="taskSearchQuery.trim() ? '未找到匹配任务' : '暂无可用任务'" icon="mdi-timer-off-outline" />
      </div>
      <VAlert v-if="configError" type="error" variant="tonal">{{ configError }}</VAlert>
      <div v-if="availableTasksForManualRun.length" class="scheduler-order-actions d-flex align-center flex-wrap justify-end ga-3">
        <span v-if="manualOrderDirty" class="text-body-2 text-medium-emphasis">顺序已调整，保存后生效</span>
        <VBtn color="primary" variant="flat" prepend-icon="mdi-content-save-outline" aria-label="保存任务顺序"
          :loading="manualOrderSaving" :disabled="manualOrderSaving" @click="saveManualTaskOrder">保存任务顺序</VBtn>
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
        <VCardActions><VSpacer /><VBtn variant="text" @click="showTgMenuConfigModal = false">取消</VBtn><VBtn color="primary" variant="flat" prepend-icon="mdi-content-save-outline" @click="saveTgMenuConfig">保存</VBtn></VCardActions>
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
import { useDisplay } from 'vuetify';
import { useMessage, useDialog } from '@/composables/useFeedback';
import { useConfig } from '../../composables/useConfig.js';
import { useTaskStatus } from '../../composables/useTaskStatus.js';
import axios from 'axios';
import FingerprintAuditPanel from './FingerprintAuditPanel.vue';
import Sortable from 'sortablejs';
import Draggable from 'vuedraggable';
import { useContextSearch } from '../../composables/useContextSearch.js';

const message = useMessage();
const dialog = useDialog();
const { mdAndUp } = useDisplay();
const manualTaskDragGroup = { pull: false, put: false };

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
const manualTaskOrder = ref([]);
const manualOrderDirty = ref(false);
const manualOrderSaving = ref(false);
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
const normalizeManualTaskOrder = (keys, tasks) => {
  const allowed = new Set(tasks.map(task => task.key));
  const saved = Array.isArray(keys) ? keys.filter(key => typeof key === 'string' && allowed.has(key)) : [];
  return [...new Set([...saved, ...tasks.map(task => task.key)])];
};
const manualTaskGroups = computed(() => {
  const groups = new Map();
  const positions = new Map(manualTaskOrder.value.map((key, index) => [key, index]));
  for (const task of filteredManualTasks.value) {
    const name = task.category_name || '其他任务';
    if (!groups.has(name)) groups.set(name, { name, tasks: [] });
    groups.get(name).tasks.push(task);
  }
  return [...groups.values()].map(group => ({ ...group, tasks: group.tasks.sort((a, b) =>
    (positions.get(a.key) ?? Infinity) - (positions.get(b.key) ?? Infinity)) }));
});
const reorderManualTaskGroup = (group) => {
  const visibleKeys = group.tasks.map(task => task.key);
  const visible = new Set(visibleKeys);
  let index = 0;
  manualTaskOrder.value = normalizeManualTaskOrder(manualTaskOrder.value, availableTasksForManualRun.value)
    .map(key => visible.has(key) ? visibleKeys[index++] : key);
  manualOrderDirty.value = true;
};
const saveManualTaskOrder = async () => {
  if (manualOrderSaving.value || !configModel.value || !availableTasksForManualRun.value.length) return;
  const submitted = normalizeManualTaskOrder(manualTaskOrder.value, availableTasksForManualRun.value);
  manualOrderSaving.value = true;
  try {
    const success = await handleSaveConfig({ manual_task_order: submitted });
    if (!success || JSON.stringify(configModel.value.manual_task_order) !== JSON.stringify(submitted)) {
      message.error(configError.value || '任务顺序保存未确认，请重试。');
      return;
    }
    if (JSON.stringify(manualTaskOrder.value) === JSON.stringify(submitted)) manualOrderDirty.value = false;
    message.success('任务顺序已保存。');
  } finally {
    manualOrderSaving.value = false;
  }
};
watch([() => configModel.value?.manual_task_order, availableTasksForManualRun], ([saved, tasks]) => {
  if (!manualOrderDirty.value && !manualOrderSaving.value) manualTaskOrder.value = normalizeManualTaskOrder(saved, tasks);
}, { immediate: true });
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
.scheduler-category-list > .scheduler-category-card + .scheduler-category-card { margin-block-start: 24px; }
.scheduler-task-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 16rem), 1fr)); gap: 12px; }
.scheduler-task-card { display: flex; flex-direction: column; min-inline-size: 0; }
.scheduler-task-summary { block-size: auto; min-block-size: 7.5rem; }
.scheduler-task-summary .app-card-summary__content { padding-inline-end: 4.5rem; }
.scheduler-task-details { padding-block-start: 0; }
.scheduler-task-card :deep(.v-card-actions) { margin-block-start: auto; padding: 16px; }
.task-drag-handle { position: absolute; inset-block-start: 4px; inset-inline-end: 4px; cursor: grab; touch-action: none; }
.task-drag-handle:active { cursor: grabbing; }
.scheduler-task-grid :deep(.sortable-ghost) { opacity: .4; }
.temp-task-name { margin: 0; line-height: 1.5; overflow-wrap: anywhere; }
.temp-task-description { margin: 6px 0 0; font-size: 12px; line-height: 1.6; color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity)); overflow-wrap: anywhere; white-space: normal; }
.task-status-message { white-space: pre-wrap; overflow-wrap: anywhere; }
.scheduler-progress { margin-block-start: 8px; max-inline-size: 320px; }
.scheduler-progress-meta { display: flex; justify-content: space-between; color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity)); font-size: 11px; gap: 12px; margin-block-start: 4px; }
.scheduler-progress-meta span { min-inline-size: 0; overflow-wrap: anywhere; white-space: normal; }
.scheduler-progress-meta strong { flex: 0 0 auto; font-variant-numeric: tabular-nums; }
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
</style>
