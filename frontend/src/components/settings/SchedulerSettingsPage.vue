<template>
  <section class="mp-page scheduler-page">
    <div class="mp-page-header mb-6">
      <MpPageTitle title="任务中心" subtitle="查看任务说明并执行媒体维护" />
    </div>
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
      <VCard class="dashboard-card manual-tasks-card">
        <VCardItem title="临时任务" subtitle="立即执行媒体维护任务"><template #prepend><VIcon icon="mdi-play-circle-outline" color="primary" /></template></VCardItem>
        <VCardText>
          <div class="temp-task-groups">
            <section v-for="group in manualTaskGroups" :key="group.key" class="temp-task-group">
              <h2 class="text-subtitle-1 font-weight-medium mb-3">{{ group.name }}</h2>
              <div class="temp-task-grid">
                <VCard v-for="task in group.tasks" :key="task.key" variant="outlined" class="task-card">
                  <VCardText class="temp-task-item">
                    <div class="temp-task-copy">
                      <div class="temp-task-name">{{ task.name }}</div>
                      <p class="temp-task-description">{{ getTaskHelp(task) }}</p>
                    </div>
                    <VTooltip :text="getTaskHelp(task)" location="top">
                      <template #activator="{ props: activatorProps }">
                        <VBtn v-bind="activatorProps" color="primary" variant="tonal" size="small" prepend-icon="mdi-play" :loading="isTriggeringTask === task.key" :disabled="isBackgroundTaskRunning || Boolean(isTriggeringTask)" @click="triggerTaskNow(task.key)">立即执行</VBtn>
                      </template>
                    </VTooltip>
                  </VCardText>
                </VCard>
              </div>
            </section>
          </div>
        </VCardText>
      </VCard>
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
        <VCardActions><VSpacer /><VBtn variant="text" @click="showTgMenuConfigModal = false">取消</VBtn><VBtn color="primary" variant="tonal" prepend-icon="mdi-content-save-outline" @click="saveTgMenuConfig">保存</VBtn></VCardActions>
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
import MpPageTitle from '../MpPageTitle.vue';
import { ref, onMounted, onBeforeUnmount, watch, nextTick, computed } from 'vue';
import { useMessage } from '@/composables/useFeedback';
import { useConfig } from '../../composables/useConfig.js';
import { useTaskStatus } from '../../composables/useTaskStatus.js';
import axios from 'axios';
import FingerprintAuditPanel from './FingerprintAuditPanel.vue';
import Sortable from 'sortablejs';

const message = useMessage();

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

const { isBackgroundTaskRunning, backgroundTaskStatus } = useTaskStatus();

const availableTasksForManualRun = ref([]);
const availableTasksForTgMenu = ref([]);
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

const manualTaskGroups = computed(() => {
  const groups = new Map();
  availableTasksForManualRun.value.forEach((task) => {
    const key = task.category || 'system-maintenance';
    if (!groups.has(key)) {
      groups.set(key, {
        key,
        name: task.category_name || '其他任务',
        tasks: [],
      });
    }
    groups.get(key).tasks.push(task);
  });
  return Array.from(groups.values());
});


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

const isMobile = ref(false);
const checkMobile = () => {
  isMobile.value = window.innerWidth < 768;
};

onMounted(() => {
  checkMobile();
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
.temp-task-groups { display: grid; gap: 24px; }
.temp-task-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 320px), 1fr)); gap: 16px; }
.task-card { min-width: 0; }
.scheduler-page :deep(.v-card-title), .scheduler-page :deep(.v-card-subtitle) { white-space: normal; overflow-wrap: anywhere; }
.temp-task-item { display: flex; justify-content: space-between; align-items: center; gap: 12px; min-height: 120px; }
.temp-task-copy { min-width: 0; flex: 1; }
.temp-task-name { font-weight: 600; line-height: 1.6; overflow-wrap: anywhere; }
.temp-task-description { margin: 6px 0 0; font-size: 12px; line-height: 1.6; color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity)); }
.temp-task-item > *:last-child { flex-shrink: 0; }
.task-chain-list { display: grid; gap: 8px; }
.task-chain-item { display: flex; align-items: center; gap: 12px; padding: 8px 12px; background: rgba(var(--v-theme-on-surface), .04); border: 1px solid rgba(var(--v-theme-on-surface), .12); border-radius: 8px; }
.task-chain-item :deep(.v-label) { white-space: normal; overflow-wrap: anywhere; }
.task-chain-item.sortable-ghost { background: rgba(var(--v-theme-primary), .12); }
.drag-handle { cursor: grab; touch-action: none; }
.drag-handle:active { cursor: grabbing; }
.task-tooltip-content { max-width: 320px; line-height: 1.6; white-space: normal; }
.sync-mode-actions { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: 8px; padding: 16px; }
@media (max-width: 899px) { .scheduler-overview { grid-template-columns: minmax(0, 1fr); } }
@media (max-width: 540px) { .sync-mode-actions { flex-direction: column; align-items: stretch; } .temp-task-item { align-items: flex-start; flex-wrap: wrap; } .temp-task-copy { flex-basis: 100%; } }
</style>
