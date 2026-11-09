<template>
  <n-card :bordered="false" class="dashboard-card fingerprint-audit"><template #header>全库指纹校验与补齐</template>
    <n-space vertical :size="16">
      <n-text depth="3">逐项核对在库电影和分集。一致就跳过，不一致且能确认文件身份就自动更新；无法确认的保留。115 目录请求至少间隔 5 秒，适合夜间运行。</n-text>
      <div class="audit-actions">
        <n-button type="primary" secondary :disabled="running || busy" @click="run('start')"><template #icon><UiIcon name="search" /></template>扫描全库并补齐</n-button>
        <n-button :disabled="running || busy || !progress?.remaining" @click="run('resume')"><template #icon><UiIcon name="play" /></template>继续上次扫描</n-button>
        <n-button :disabled="running || busy || !progress?.failed || !!progress?.remaining" @click="run('retry')"><template #icon><UiIcon name="refresh" /></template>重试无法确认项</n-button>
        <n-button :loading="loading" @click="load"><template #icon><UiIcon name="refresh" /></template>刷新进度</n-button>
        <n-button :disabled="!reportAvailable" @click="reportVisible = true"><template #icon><UiIcon name="search" /></template>查看并分析报告</n-button>
      </div>
      <div v-if="progress" class="audit-progress">
        <div class="audit-progress-heading"><strong>{{ stateLabel }}</strong><span>已扫描 {{ progress.completed }} / {{ progress.total }}</span></div>
        <n-progress type="line" :percentage="progress.total ? Math.round(progress.completed / progress.total * 100) : 0" :show-indicator="false" />
        <dl class="audit-summary">
          <div><dt>一致跳过</dt><dd>{{ progress.counts.consistent }}</dd></div>
          <div><dt>已更新</dt><dd>{{ progress.counts.repaired }}</dd></div>
          <div><dt>无法确认</dt><dd>{{ progress.counts.unknown }}</dd></div>
        </dl>
      </div>
      <n-alert v-if="runFeedback" :type="runFeedback.type" :title="runFeedback.message">
        <div v-for="detail in runFeedback.details" :key="detail">{{ detail }}</div>
      </n-alert>
      <n-text v-if="progress?.report_error" type="error">{{ progress.report_error }}</n-text>
      <n-text depth="3">报告保存在 /config/fingerprint-scan-latest.json，只保留最新一份；可在 Web 查看原因分析、更新明细并再次补齐未确认项。后台任务支持暂停、继续和停止；停止或重启后可从保存的位置继续。</n-text>
    </n-space>
    <FingerprintReport v-model:show="reportVisible" :running="running" :busy="busy"
      :current-run-id="progress?.run_id" @retry="run('retry', $event)" />
  </n-card>
</template>

<script setup>
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { NAlert, NButton, NCard, NSpace, NText, NProgress, useMessage } from 'naive-ui';
import axios from 'axios';
import FingerprintReport from './FingerprintReport.vue';

const props = defineProps({ running: Boolean, taskState: String });
const message = useMessage();
const progress = ref(null), loading = ref(false), busy = ref(false);
const reportAvailable = ref(false), reportVisible = ref(false);
const runFeedback = ref(null);
let timer, sequence = 0;
const stateLabel = computed(() => {
  if (progress.value?.state === 'running') {
    if (props.taskState === 'paused') return '已暂停';
    if (props.taskState === 'pausing') return '等待安全暂停点';
    return props.running ? '执行中' : '可继续';
  }
  return { interrupted: '已中断', complete: '已完成', attention: '已完成，有未确认项' }[progress.value?.state] || '';
});
async function load() {
  const current = ++sequence;
  loading.value = true;
  try {
    const response = await axios.get('/api/tasks/fingerprint-scan');
    if (current === sequence) {
      progress.value = response.data.progress;
      reportAvailable.value = response.data.report_available;
    }
  } catch (error) {
    if (current === sequence) message.error(error.response?.data?.error || '读取扫描进度失败');
  } finally { if (current === sequence) loading.value = false; }
}
async function run(mode, runId = progress.value?.run_id) {
  busy.value = true;
  try {
    const response = await axios.post('/api/tasks/fingerprint-scan', { mode, run_id: runId });
    const result = response.data || {};
    const details = [...new Set([
      ...(Array.isArray(result.failures) ? result.failures : []),
      ...(Array.isArray(result.warning_details) ? result.warning_details : []),
    ].map(detail => typeof detail === 'string' ? detail : [detail?.item_id, detail?.reason || detail?.error].filter(Boolean).join('：')).filter(Boolean))];
    const unknown = Number(result.counts?.unknown || 0);
    const failed = Number(result.failed) > 0 || (Array.isArray(result.failures) && result.failures.length > 0);
    if (failed || response.status === 207 || Number(result.warnings) > 0 || unknown > 0 || details.length) {
      if (!details.length) details.push(unknown > 0 ? `有 ${unknown} 项未确认，请查看扫描报告中的原因。` : '处理结果有提醒，请查看扫描报告。');
      runFeedback.value = { type: failed ? 'error' : 'warning', message: result.error || result.message || '指纹扫描有未确认项', details };
      message[runFeedback.value.type]([runFeedback.value.message, ...details].join('；'), { duration: 8000 });
    } else {
      runFeedback.value = null;
      message.success(result.message || '指纹扫描已完成');
    }
    await load();
  } catch (error) {
    const result = error.response?.data || {};
    const reason = result.error || '提交扫描任务失败';
    const details = (Array.isArray(result.failures) ? result.failures : []).map(detail => typeof detail === 'string' ? detail : [detail?.item_id, detail?.reason || detail?.error].filter(Boolean).join('：')).filter(Boolean);
    runFeedback.value = { type: 'error', message: reason, details };
    message.error([reason, ...details].join('；'));
  }
  finally { busy.value = false; }
}
watch(() => props.running, load);
onMounted(() => { load(); timer = setInterval(() => { if (props.running) load(); }, 3000); });
onUnmounted(() => clearInterval(timer));
</script>

<style scoped>
.fingerprint-audit { container-type:inline-size; }
.audit-actions { display:flex; flex-wrap:wrap; gap:10px; }
.audit-progress { padding:16px; border:1px solid var(--ui-border); border-radius:10px; background:var(--ui-surface-muted); }
.audit-progress-heading { display:flex; flex-wrap:wrap; justify-content:space-between; gap:8px; margin-bottom:12px; }
.audit-progress-heading span { color:var(--ui-text-muted); }
.audit-summary { display:grid; grid-template-columns:repeat(3,minmax(0,1fr)); gap:12px; margin:16px 0 0; }
.audit-summary div { min-width:0; }
.audit-summary dt { color:var(--ui-text-muted); font-size:12px; }
.audit-summary dd { margin:4px 0 0; color:var(--ui-text); font-size:22px; font-weight:600; }
@container (max-width:520px) { .audit-actions > .n-button { flex:1 1 160px; } .audit-progress { padding:12px; } .audit-summary { gap:8px; } }
</style>
