<template>
  <section class="notification-center" aria-label="通知中心">
    <header class="notification-heading">
      <div><h2>通知中心</h2><p>查看运行状态与日志</p></div>
      <div class="notification-window-actions">
        <n-button v-if="!mobile" quaternary circle :aria-label="expanded ? '收起面板' : '展开面板'" :title="expanded ? '收起面板' : '展开面板'" @click="$emit('expand')"><template #icon><n-icon :component="expanded ? ContractOutline : ExpandOutline" /></template></n-button>
        <n-button quaternary circle aria-label="关闭通知中心" title="关闭" @click="$emit('close')"><template #icon><n-icon :component="CloseOutline" /></template></n-button>
      </div>
    </header>
    <n-tabs v-model:value="activeTab" type="line" class="notification-tabs">
      <n-tab name="logs">实时日志</n-tab>
      <n-tab name="history">历史日志</n-tab>
    </n-tabs>
    <div v-show="activeTab === 'logs'" class="notification-live">
      <section class="notification-task" aria-label="当前任务">
        <div class="notification-task-heading">
          <strong>{{ task.is_running ? task.current_action : (task.last_action || '暂无运行任务') }}</strong>
          <n-tag size="small" :type="statusType">{{ stateLabel }}</n-tag>
        </div>
        <p class="notification-task-message">{{ taskMessage }}</p>
        <n-progress v-if="task.is_running && task.progress_known" type="line" :percentage="Number(task.progress) || 0" :show-indicator="true" :height="5" />
        <div v-if="task.is_running" class="notification-task-controls">
          <span v-if="task.phase" class="notification-task-phase">{{ task.phase }}</span>
          <n-button size="small" secondary :loading="controlAction === 'pause' || controlAction === 'resume'" :disabled="controlDisabled" @click="togglePause">
            <template #icon><n-icon :component="task.state === 'paused' ? PlayOutline : PauseOutline" /></template>
            {{ task.state === 'pausing' ? '正在暂停…' : task.state === 'paused' ? '继续任务' : '暂停任务' }}
          </n-button>
          <n-popconfirm @positive-click="stopTask" positive-text="停止任务" negative-text="取消" :positive-button-props="{ type: 'error' }">
            <template #trigger><n-button size="small" type="error" secondary :loading="controlAction === 'stop'" :disabled="!!controlAction || task.stop_requested || task.state === 'stopping'"><template #icon><n-icon :component="StopOutline" /></template>{{ task.stop_requested || task.state === 'stopping' ? '正在停止…' : '停止任务' }}</n-button></template>
            停止后将完成必要的收尾操作，当前任务不会继续处理新项目。
          </n-popconfirm>
        </div>
      </section>
      <div class="notification-toolbar">
        <n-input v-model:value="keyword" clearable placeholder="搜索实时日志" aria-label="搜索实时日志" size="small"><template #prefix><n-icon :component="SearchOutline" /></template></n-input>
        <n-select v-model:value="logFilter" :options="filterOptions" aria-label="日志类型" size="small" class="notification-filter" />
        <label class="notification-follow"><n-switch v-model:value="following" size="small" aria-label="自动跟随日志" />自动跟随</label>
      </div>
      <div ref="logRef" class="notification-log" role="region" aria-label="实时日志内容" tabindex="0" @scroll="handleScroll">
        <div v-for="(line, index) in filteredLogs" :key="index" class="notification-line" :class="lineClass(line)"><span class="notification-line-number" aria-hidden="true">{{ index + 1 }}</span><span>{{ line }}</span></div>
        <n-empty v-if="!filteredLogs.length" :description="displayedLogs.length ? '没有匹配的日志' : '暂无实时日志'" class="notification-empty" />
      </div>
      <footer class="notification-footer">
        <span>{{ filteredLogs.length }} 条日志{{ following ? ' · 自动跟随最新日志' : ' · 已暂停跟随' }}</span>
        <n-button v-if="!following" size="small" type="primary" secondary @click="following = true">{{ hasNewLogs ? '有新日志 · 回到最新' : '回到最新' }}</n-button>
      </footer>
    </div>
    <LogViewer v-show="activeTab === 'history'" :show="activeTab === 'history'" />
  </section>
</template>

<script setup>
import { computed, ref, watch, nextTick } from 'vue';
import { NTabs, NTab, NTag, NButton, NEmpty, NIcon, NInput, NSelect, NSwitch, NProgress, NPopconfirm, useMessage } from 'naive-ui';
import { CloseOutline, ExpandOutline, ContractOutline, SearchOutline, PauseOutline, PlayOutline, StopOutline } from '@vicons/ionicons5';
import LogViewer from './LogViewer.vue';
import { refreshTaskStatus } from '../composables/useTaskStatus.js';
import axios from 'axios';

const props = defineProps({ task: { type: Object, default: () => ({}) }, expanded: Boolean, mobile: Boolean });
defineEmits(['expand', 'close']);
const message = useMessage();
const activeTab = ref('logs'), logRef = ref(null), controlAction = ref('');
const keyword = ref(''), logFilter = ref('all'), following = ref(true), displayedLogs = ref([]);
const filterOptions = [{ label: '全部日志', value: 'all' }, { label: '任务进度', value: 'progress' }, { label: '警告与错误', value: 'warning' }];
const labels = { idle: '空闲', queued: '等待执行', completed: '已完成', completed_with_warnings: '已完成，有提醒', failed: '失败', cancelled: '已停止', paused: '已暂停', pausing: '正在暂停', stopping: '正在停止', running: '执行中' };
const stateLabel = computed(() => props.task.stop_requested ? '正在停止' : (labels[props.task.state] || (props.task.is_running ? '执行中' : '空闲')));
const statusType = computed(() => props.task.state === 'failed' ? 'error' : ['pausing', 'paused', 'stopping', 'completed_with_warnings'].includes(props.task.state) ? 'warning' : props.task.is_running ? 'info' : props.task.state === 'completed' ? 'success' : 'default');
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
const latestLogs = computed(() => props.task.logs || []);
const hasNewLogs = computed(() => latestLogs.value.join('\n') !== displayedLogs.value.join('\n'));
const filteredLogs = computed(() => displayedLogs.value.filter(line =>
  (!keyword.value.trim() || line.toLowerCase().includes(keyword.value.trim().toLowerCase())) &&
  (logFilter.value === 'all' || (logFilter.value === 'progress' ? /\[任务(?:进度|开始|结束|暂停|继续)\]/.test(line) : /\b(WARNING|ERROR|CRITICAL)\b/.test(line)))
));
const lineClass = line => /\b(ERROR|CRITICAL)\b/.test(line) ? 'log-error' : /\bWARNING\b/.test(line) ? 'log-warning' : /\[任务(?:进度|开始|结束|暂停|继续)\]/.test(line) ? 'log-progress' : '';
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
.notification-center { display:flex; flex-direction:column; height:100%; min-height:0; gap:16px; }
.notification-heading { display:flex; align-items:center; gap:12px; flex-shrink:0; }
h2 { margin:0; font-size:20px; font-weight:600; }
.notification-heading p { margin:4px 0 0; font-size:12px; color:var(--ui-text-muted); }
.notification-window-actions { display:flex; margin-left:auto; }
.notification-tabs { flex-shrink:0; }
.notification-live { display:flex; flex:1; flex-direction:column; min-height:0; gap:16px; }
.notification-task { flex-shrink:0; padding:16px; border:1px solid var(--ui-border); border-radius:10px; background:var(--ui-surface-muted); }
.notification-task-heading { display:flex; align-items:flex-start; justify-content:space-between; gap:16px; }
.notification-task-heading strong { min-width:0; overflow-wrap:anywhere; font-size:14px; }
.notification-task-message { margin:8px 0 12px; color:var(--ui-text-muted); font-size:12px; line-height:1.7; }
.notification-task-controls { display:flex; flex-wrap:wrap; align-items:center; justify-content:flex-end; gap:10px; margin-top:14px; }
.notification-task-phase { flex:1; color:var(--ui-text-muted); font-size:12px; }
.notification-toolbar { display:flex; align-items:center; gap:12px; flex-shrink:0; }
.notification-toolbar > .n-input { min-width:0; }
.notification-filter { width:145px; flex-shrink:0; }
.notification-follow { display:flex; align-items:center; gap:8px; white-space:nowrap; font-size:12px; }
.notification-log { flex:1; min-height:100px; overflow:auto; overscroll-behavior:contain; padding:12px; border:1px solid var(--ui-border); border-radius:10px; background:var(--ui-surface-muted); font:12px/1.8 ui-monospace, SFMono-Regular, Consolas, monospace; }
.notification-line { display:flex; gap:12px; padding:3px 0; }
.notification-line > span:last-child { white-space:pre-wrap; overflow-wrap:anywhere; min-width:0; }
.notification-line-number { flex:0 0 3ch; text-align:right; color:var(--ui-text-muted); user-select:none; }
.log-error { color:var(--ui-error-color); }
.log-warning { color:var(--ui-warning-color); }
.log-progress { color:var(--accent-color); }
.notification-footer { display:flex; align-items:center; flex-wrap:wrap; gap:8px; flex-shrink:0; }
.notification-footer > span { margin-right:auto; font-size:12px; color:var(--ui-text-muted); }
.notification-empty { margin:40px 0; }
@media (max-width:600px) {
  .notification-toolbar { flex-wrap:wrap; gap:10px; }
  .notification-toolbar > .n-input { flex-basis:100%; }
  .notification-filter { flex:1; width:auto; }
  .notification-center, .notification-live { gap:12px; }
  .notification-log { padding:8px; }
  .notification-task { padding:12px; }
  .notification-footer > span { flex-basis:100%; }
}
</style>
