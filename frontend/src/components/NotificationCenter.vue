<template>
  <section class="notification-center" aria-label="通知中心">
    <header class="notification-heading">
      <h2>通知中心</h2>
      <div class="notification-window-actions">
        <VBtn variant="text" icon="mdi-close" color="default" size="small" aria-label="关闭通知中心" title="关闭" @click="$emit('close')" />
      </div>
    </header>
    <VTabs v-model="activeTab" color="primary" density="compact" class="notification-tabs">
      <VTab value="logs" prepend-icon="mdi-text-box-outline">实时日志</VTab>
      <VTab value="history" prepend-icon="mdi-history">历史日志</VTab>
    </VTabs>
    <div v-show="activeTab === 'logs'" class="notification-live">
      <VCard variant="tonal" class="notification-task" aria-label="当前任务">
        <div class="notification-task-heading">
          <strong>{{ task.is_running ? task.current_action : (task.last_action || '暂无运行任务') }}</strong>
          <VChip size="small" :color="statusType" variant="tonal">{{ stateLabel }}</VChip>
        </div>
        <p class="notification-task-message">{{ taskMessage }}</p>
        <div v-if="task.is_running && task.progress_known" class="notification-progress">
          <VProgressLinear :model-value="Number(task.progress) || 0" color="primary" :height="5" rounded aria-label="任务进度" />
          <span>{{ Number(task.progress) || 0 }}%</span>
        </div>
        <div v-if="task.is_running" class="notification-task-controls">
          <span v-if="task.phase" class="notification-task-phase">{{ task.phase }}</span>
          <VBtn size="small" variant="tonal" color="primary" :prepend-icon="task.state === 'paused' ? 'mdi-play' : 'mdi-pause'" :loading="controlAction === 'pause' || controlAction === 'resume'" :disabled="controlDisabled" @click="togglePause">
            {{ task.state === 'pausing' ? '正在暂停…' : task.state === 'paused' ? '继续任务' : '暂停任务' }}
          </VBtn>
          <VBtn size="small" color="error" variant="tonal" prepend-icon="mdi-stop" :loading="controlAction === 'stop'" :disabled="!!controlAction || task.stop_requested || task.state === 'stopping'" @click="confirmStopTask">
            {{ task.stop_requested || task.state === 'stopping' ? '正在停止…' : '停止任务' }}
          </VBtn>
        </div>
      </VCard>
      <div class="notification-toolbar">
        <VTextField v-model="keyword" clearable hide-details variant="outlined" density="compact" prepend-inner-icon="mdi-magnify" placeholder="搜索实时日志" aria-label="搜索实时日志" class="notification-search" />
        <VSelect v-model="logFilter" :items="filterOptions" item-title="label" item-value="value" hide-details variant="outlined" density="compact" aria-label="日志类型" class="notification-filter" />
        <VSwitch v-model="following" hide-details inset color="primary" density="compact" label="自动跟随" aria-label="自动跟随日志" class="notification-follow" />
        <VBtn size="small" variant="tonal" prepend-icon="mdi-download" @click="downloadLogs" :disabled="!filteredLogs.length">导出日志</VBtn>
      </div>
      <div ref="logRef" class="notification-log" role="region" aria-label="实时日志内容" tabindex="0" @scroll="handleScroll">
        <LogRecord v-for="(line, index) in filteredLogs" :key="index" :line="line" :keyword="keyword" />
        <VAlert v-if="!filteredLogs.length" variant="tonal" color="secondary" icon="mdi-text-box-outline" class="notification-empty">{{ displayedLogs.length ? '没有匹配的日志' : '暂无实时日志' }}</VAlert>
      </div>
      <footer class="notification-footer">
        <span>{{ filteredLogs.length }} 条日志{{ following ? ' · 自动跟随最新日志' : ' · 已暂停跟随' }}</span>
        <VBtn v-if="!following" size="small" color="primary" variant="tonal" prepend-icon="mdi-arrow-down" @click="following = true">{{ hasNewLogs ? '有新日志 · 回到最新' : '回到最新' }}</VBtn>
      </footer>
    </div>
    <LogViewer v-show="activeTab === 'history'" :show="activeTab === 'history'" />
  </section>
</template>

<script setup>
import { computed, ref, watch, nextTick } from 'vue';
import { useMessage, useDialog } from '@/composables/useFeedback';
import LogViewer from './LogViewer.vue';
import LogRecord from './LogRecord.vue';
import { matchesLogRecord } from '@/utils/logRecords';
import { refreshTaskStatus } from '../composables/useTaskStatus.js';
import axios from 'axios';

const props = defineProps({ task: { type: Object, default: () => ({}) } });
defineEmits(['close']);
const message = useMessage();
const dialog = useDialog();
const activeTab = ref('logs'), logRef = ref(null), controlAction = ref('');
const keyword = ref(''), logFilter = ref('all'), following = ref(true), displayedLogs = ref([]);
const filterOptions = [{ label: '全部日志', value: 'all' }, { label: 'INFO', value: 'info' }, { label: '任务进度', value: 'progress' }, { label: '警告', value: 'warning' }, { label: '错误', value: 'error' }];
const labels = { idle: '空闲', queued: '等待执行', completed: '已完成', completed_with_warnings: '已完成，有提醒', failed: '失败', cancelled: '已停止', paused: '已暂停', pausing: '正在暂停', stopping: '正在停止', running: '执行中' };
const stateLabel = computed(() => props.task.stop_requested ? '正在停止' : (labels[props.task.state] || (props.task.is_running ? '执行中' : '空闲')));
const statusType = computed(() => props.task.state === 'failed' ? 'error' : ['pausing', 'paused', 'stopping', 'completed_with_warnings'].includes(props.task.state) ? 'warning' : props.task.is_running ? 'info' : props.task.state === 'completed' ? 'success' : 'secondary');
const controlDisabled = computed(() => !!controlAction.value || !props.task.task_id || props.task.stop_requested || !['running', 'paused'].includes(props.task.state));
const taskMessage = computed(() => props.task.stop_requested || props.task.state === 'stopping' ? '正在完成收尾操作，请稍候。' : props.task.state === 'pausing' ? '已请求暂停，当前操作完成后会在安全检查点暂停。' : props.task.state === 'paused' ? '任务已暂停，点击继续任务将从当前进度恢复。' : (props.task.message || '开始任务后，可在这里查看进度。'));

async function control(action) {
  if (controlAction.value || !props.task.is_running || !props.task.task_id || props.task.stop_requested) return;
  const taskId = props.task.task_id;
  controlAction.value = action;
  try {
    const endpoint = action === 'stop' ? '/api/trigger_stop_task' : '/api/tasks/' + action;
    const { data } = await axios.post(endpoint, { task_id: taskId });
    message.info(data.message || '操作请求已提交');
  } catch (error) {
    message.error(error.response?.data?.error || '任务控制失败');
  } finally {
    await refreshTaskStatus(true);
    controlAction.value = '';
  }
}
function togglePause() {
  if (!controlDisabled.value) return control(props.task.state === 'paused' ? 'resume' : 'pause');
}
function stopTask() { return control('stop'); }
function confirmStopTask() {
  const taskId = props.task.task_id;
  dialog.warning({
    title: '停止任务',
    content: '停止后将完成必要的收尾操作，当前任务不会继续处理新项目。',
    positiveText: '停止任务',
    negativeText: '取消',
    onPositiveClick: () => {
      if (props.task.task_id === taskId) return stopTask();
    },
  });
}
const latestLogs = computed(() => props.task.logs || []);
const hasNewLogs = computed(() => latestLogs.value.join('\n') !== displayedLogs.value.join('\n'));
function downloadLogs() {
  const url = URL.createObjectURL(new Blob([filteredLogs.value.join('\n')], { type: 'text/plain;charset=utf-8' }));
  const link = document.createElement('a');
  link.href = url;
  link.download = `etk-${new Date().toISOString().replace(/[:.]/g, '-')}.log`;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
const filteredLogs = computed(() => displayedLogs.value.filter(line => matchesLogRecord(line, keyword.value, logFilter.value)));
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
.notification-center { display:flex; flex-direction:column; height:100%; min-height:0; min-width:0; gap:16px; overflow:auto; }
.notification-heading { display:flex; align-items:center; gap:12px; flex-shrink:0; }
h2 { margin:0; font-size:20px; font-weight:600; }
.notification-window-actions { display:flex; margin-left:auto; }
.notification-tabs { flex-shrink:0; }
.notification-live { display:flex; flex:1; flex-direction:column; min-height:0; min-width:0; gap:16px; overflow:auto; }
.notification-task { flex-shrink:0; padding:16px; border:1px solid var(--ui-border); border-radius:10px; background:var(--ui-surface-muted); }
.notification-task-heading { display:flex; align-items:flex-start; justify-content:space-between; gap:16px; }
.notification-task-heading strong { min-width:0; overflow-wrap:anywhere; font-size:14px; }
.notification-task-message { margin:8px 0 12px; max-height:5.1em; overflow:auto; color:var(--ui-text-muted); font-size:12px; line-height:1.7; overflow-wrap:anywhere; }
.notification-progress { display:flex; align-items:center; gap:10px; }
.notification-progress > span { flex-shrink:0; font-size:12px; color:var(--ui-text-muted); }
.notification-task-controls { display:flex; flex-wrap:wrap; align-items:center; justify-content:flex-end; gap:10px; margin-top:14px; }
.notification-task-phase { flex:1; min-width:0; max-height:3.2em; overflow:auto; overflow-wrap:anywhere; color:var(--ui-text-muted); font-size:12px; }
.notification-toolbar { display:flex; align-items:center; gap:12px; flex-shrink:0; }
.notification-search { flex:1; min-width:0; }
.notification-filter { width:145px; flex-shrink:0; }
.notification-follow { flex:0 0 auto; white-space:nowrap; }
.notification-log { display:flex; flex-direction:column; gap:12px; flex:1 0 160px; min-height:160px; min-width:0; overflow:auto; overscroll-behavior:contain; padding:8px 4px; }
.notification-footer { display:flex; align-items:center; flex-wrap:wrap; gap:8px; flex-shrink:0; }
.notification-footer > span { margin-right:auto; font-size:12px; color:var(--ui-text-muted); }
.notification-empty { margin:40px 0; }
@media (max-width:600px) {
  .notification-toolbar { flex-wrap:wrap; gap:10px; }
  .notification-search { flex-basis:100%; }
  .notification-filter { flex:1; width:auto; }
  .notification-center, .notification-live { gap:12px; }
  .notification-log { padding:8px; }
  .notification-task { padding:12px; }
  .notification-task-phase { flex-basis:100%; }
  .notification-footer > span { flex-basis:100%; }
}
</style>
