<!-- src/components/settings/SchedulerSettingsPage.vue -->
<template>
  <n-layout class="mp-page scheduler-page" content-style="padding: 0;">
    <n-page-header class="mp-page-header" subtitle="查看任务说明并执行媒体维护"><template #title>任务中心</template></n-page-header>
    <div v-if="isLoading" class="center-container">
      <n-spin size="large" />
    </div>

    <div v-else-if="configModel" class="scheduler-content">
      <div class="scheduler-overview">
      <n-card :bordered="false" class="dashboard-card">
        <template #header>
          <span class="card-title">Telegram 快捷菜单</span>
        </template>
        <template #header-extra>
          <n-text depth="3">配置显示在 TG 机器人输入框左侧 Menu 按钮中的快捷任务</n-text>
        </template>

        <n-grid cols="1 720:3" :x-gap="24" :y-gap="16">
          <n-gi span="1">
            <n-button type="default" @click="showTgMenuConfigModal = true" style="width: 100%;">
              <template #icon><n-icon :component="Settings24Regular" /></template>
              配置 TG 菜单任务
            </n-button>
          </n-gi>

          <n-gi span="1 720:2">
            <n-text strong>当前 TG 菜单列表</n-text>
            <div class="flowchart-wrapper">
              <div v-if="enabledTgMenuTasks.length > 0" class="flowchart-container">
                <n-tooltip v-for="task in enabledTgMenuTasks" :key="task.key" trigger="hover" placement="top">
                  <template #trigger>
                    <div class="flowchart-node">{{ task.name }}</div>
                  </template>
                  <div class="task-tooltip-content">
                    <div class="task-tooltip-title">{{ task.name }}</div>
                    <div>{{ getTaskHelp(task) }}</div>
                  </div>
                </n-tooltip>
              </div>
              <div v-else class="flowchart-container empty">
                <n-text depth="3">暂未配置任何 TG 菜单任务...</n-text>
              </div>
            </div>
          </n-gi>
        </n-grid>
      </n-card>

      <FingerprintAuditPanel class="fingerprint-overview-card" :running="isBackgroundTaskRunning" :task-state="backgroundTaskStatus.state" />
      </div>
      <n-card :bordered="false" class="dashboard-card manual-tasks-card">
        <template #header>
          <span class="card-title">临时任务</span>
        </template>
        <template #header-extra>
          <n-text depth="3">用于需要立即手动执行的场景</n-text>
        </template>

        <div class="temp-task-groups">
          <section v-for="group in manualTaskGroups" :key="group.key" class="temp-task-group">
            <div class="temp-task-group-title">{{ group.name }}</div>
            <div class="temp-task-grid">
              <div v-for="task in group.tasks" :key="task.key">
                <n-tooltip trigger="hover" placement="top">
                  <template #trigger>
                    <div class="temp-task-item">
                      <div class="temp-task-copy">
                        <n-text class="temp-task-name">{{ task.name }}</n-text>
                        <p class="temp-task-description">{{ getTaskHelp(task) }}</p>
                      </div>
                      <n-button
                        size="small"
                        type="primary"
                        ghost
                        @click="triggerTaskNow(task.key)"
                        :loading="isTriggeringTask === task.key"
                        :disabled="isBackgroundTaskRunning"
                      >
                        <template #icon><n-icon :component="Play24Regular" /></template>
                        立即执行
                      </n-button>
                    </div>
                  </template>
                  <div class="task-tooltip-content">
                    <div class="task-tooltip-title">{{ task.name }}</div>
                    <div>{{ getTaskHelp(task) }}</div>
                  </div>
                </n-tooltip>
              </div>
            </div>
          </section>
        </div>
      </n-card>
    </div>

    <n-modal
      v-model:show="showTgMenuConfigModal"
      class="custom-card custom-modal glass-modal"
      preset="card"
      style="width: 90%; max-width: 600px;"
      :mask-closable="false"
    ><template #header>配置 Telegram 快捷菜单</template>
      <n-alert type="info" :show-icon="false" style="margin-bottom: 16px;">
        请勾选需要在 Telegram 机器人菜单中显示的任务，并拖动调整顺序。
      </n-alert>
      <div class="task-chain-list" ref="draggableContainerTgMenu">
        <n-tooltip v-for="task in configuredTgMenuSequence" :key="task.key" to="body" trigger="hover" placement="bottom">
          <template #trigger>
            <div class="task-chain-item" :data-key="task.key">
              <n-icon :component="Drag24Regular" class="drag-handle" />
              <n-checkbox v-model:checked="task.enabled" style="flex-grow: 1;">{{ task.name }}</n-checkbox>
            </div>
          </template>
          <div class="task-tooltip-content">
            <div class="task-tooltip-title">{{ task.name }}</div>
            <div>{{ getTaskHelp(task) }}</div>
          </div>
        </n-tooltip>
      </div>
      <template #footer>
        <n-space justify="end">
          <n-button @click="showTgMenuConfigModal = false"><template #icon><UiIcon name="close" /></template>取消</n-button>
          <n-button type="primary" @click="saveTgMenuConfig"><template #icon><UiIcon name="save" /></template>保存</n-button>
        </n-space>
      </template>
    </n-modal>

    <n-modal v-model:show="showSyncModeModal" preset="dialog" :show-icon="false" :mask-closable="false" class="glass-modal"><template #header>选择处理模式</template>
      <n-text>您希望如何执行此任务？</n-text>
      <template #action>
        <n-button @click="showSyncModeModal = false"><template #icon><UiIcon name="close" /></template>取消</n-button>
        <n-button @click="runTaskFromModal(false)"><template #icon><UiIcon name="play" /></template>快速模式（增量）</n-button>
        <n-button type="warning" @click="runTaskFromModal(true)"><template #icon><UiIcon name="play" /></template>深度模式 (全量)</n-button>
      </template>
    </n-modal>
  </n-layout>
</template>

<script setup>
import { ref, onMounted, watch, nextTick, computed } from 'vue';
import {
  NCheckbox, NGrid, NGi, NAlert, NButton, NCard, NSpace, NIcon,
  NText, useMessage, NLayout, NSpin, NModal, NTooltip, NPageHeader
} from 'naive-ui';
import { Play24Regular, Settings24Regular, Drag24Regular } from '@vicons/fluent';
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

const runTaskFromModal = async (isDeepMode) => {
  showSyncModeModal.value = false;
  const taskIdentifier = taskToRunInModal.value;
  if (!taskIdentifier) return;

  isTriggeringTask.value = taskIdentifier;
  try {
    const payload = { task_name: taskIdentifier, force_full_update: isDeepMode };
    const response = await axios.post('/api/tasks/run', payload);
    message.success(response.data.message || '任务已成功提交。');
  } catch (error) {
    message.error(error.response?.data?.error || '请求后端接口失败。');
  } finally {
    isTriggeringTask.value = null;
    taskToRunInModal.value = null;
  }
};

const triggerTaskNow = async (taskIdentifier) => {
  if (isBackgroundTaskRunning.value) {
    message.warning('已有后台任务正在运行，请稍后再试。');
    return;
  }

  if (DUAL_MODE_TASKS.includes(taskIdentifier)) {
    taskToRunInModal.value = taskIdentifier;
    showSyncModeModal.value = true;
    return;
  }

  isTriggeringTask.value = taskIdentifier;
  try {
    const response = await axios.post('/api/tasks/run', { task_name: taskIdentifier });
    message.success(response.data.message || '任务已成功提交。');
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

watch(showTgMenuConfigModal, (newValue) => {
  if (newValue) {
    nextTick(() => {
      sortableInstanceTgMenu = initializeSortable(draggableContainerTgMenu.value, configuredTgMenuSequence);
    });
  } else if (sortableInstanceTgMenu) {
    sortableInstanceTgMenu.destroy();
    sortableInstanceTgMenu = null;
  }
});

watch([configModel, availableTasksForTgMenu], ([newConfig, tgMenuTasks]) => {
  if (newConfig && tgMenuTasks.length > 0) {
    initializeSequence(newConfig.tg_menu_tasks || DEFAULT_TG_TASKS, configuredTgMenuSequence, tgMenuTasks);
  }
}, { immediate: true, deep: true });
</script>

<style scoped>
.scheduler-content { display:grid; gap:24px; }
.scheduler-overview { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:24px; align-items:stretch; }
.scheduler-overview > * { min-width:0; height:100%; }
@container (max-width:899px) { .scheduler-overview { grid-template-columns:minmax(0,1fr); } }

.center-container {
  display: flex;
  justify-content: center;
  align-items: center;
  height: calc(100vh - 200px);
}
.temp-task-item {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 12px;
  min-height: 128px;
  height:100%;
  padding:16px;
  border:1px solid var(--ui-border);
  border-radius:10px;
  background:var(--ui-surface-muted);
}
.temp-task-grid { display:grid; grid-template-columns:repeat(auto-fill, minmax(min(100%, 300px), 1fr)); gap:14px; }
.temp-task-grid > div { min-width:0; }
.temp-task-copy { min-width:0; flex:1; }
.temp-task-name { color:var(--ui-text); font-weight:600; line-height:1.6; display:-webkit-box; -webkit-line-clamp:2; -webkit-box-orient:vertical; overflow:hidden; }
.temp-task-description { margin:6px 0 0; font-size:12px; line-height:1.6; color:var(--ui-text-muted); display:-webkit-box; -webkit-line-clamp:2; -webkit-box-orient:vertical; overflow:hidden; }
.temp-task-item > .n-button { flex-shrink:0; align-self:center; }
.temp-task-groups {
  display: flex;
  flex-direction: column;
  gap: 22px;
}
.temp-task-group {
  min-width: 0;
}
.temp-task-group-title {
  margin-bottom: 10px;
  color: var(--n-text-color-2);
  font-size: 14px;
  font-weight: 600;
}
.task-chain-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.task-chain-item {
  display: flex;
  align-items: center;
  padding: 10px;
  background-color: var(--n-action-color);
  border-radius: 4px;
  border:1px solid var(--ui-border);
  cursor: help;
}
.task-chain-item :deep(.n-checkbox) { min-width:0; }
.task-chain-item :deep(.n-checkbox__label) { white-space:normal; overflow-wrap:anywhere; }
.task-chain-item.sortable-ghost {
  background-color: var(--n-color-target-suppl);
}
.drag-handle {
  cursor: grab;
  margin-right: 12px;
  color: var(--n-text-color-disabled);
}
.drag-handle:active {
  cursor: grabbing;
}
.task-tooltip-content {
  max-width: 320px;
  line-height: 1.6;
  white-space: normal;
}
.task-tooltip-title {
  font-weight: 600;
  margin-bottom: 4px;
}
.flowchart-wrapper {
  margin-top: 12px;
  min-height: 64px;
  display: flex;
  align-items: center;
  overflow-x: auto;
  padding: 8px;
  background-color:var(--ui-surface-muted);
  border-radius:8px;
}
.flowchart-container {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}
.flowchart-container.empty {
  width: 100%;
  justify-content: center;
}
.flowchart-node {
  padding: 6px 12px;
  border-radius: 4px;
  background-color:var(--ui-surface);
  border:1px solid var(--ui-border);
  white-space: nowrap;
  cursor: help;
}
</style>
