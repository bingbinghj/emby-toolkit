<template>
  <VCard variant="outlined" class="dashboard-card fingerprint-audit">
    <VCardTitle class="d-flex align-center ga-3">
      <VAvatar color="primary" variant="tonal" size="40"><VIcon icon="mdi-fingerprint" /></VAvatar>
      <span>全库指纹校验与补齐</span>
    </VCardTitle>
    <VCardText class="audit-content">
      <p class="text-body-2 text-medium-emphasis">逐项核对在库电影和分集。一致就跳过，不一致且能确认文件身份就自动更新；无法确认的保留。115 目录请求至少间隔 5 秒，适合夜间运行。</p>
      <div class="audit-actions">
        <VBtn color="primary" variant="tonal" prepend-icon="mdi-magnify" :disabled="running || busy" @click="run('start')">扫描全库并补齐</VBtn>
        <VBtn variant="tonal" prepend-icon="mdi-play" :disabled="running || busy || !progress?.remaining" @click="run('resume')">继续上次扫描</VBtn>
        <VBtn variant="tonal" prepend-icon="mdi-refresh" :disabled="running || busy || !progress?.failed || !!progress?.remaining" @click="run('retry')">重试无法确认项</VBtn>
        <VBtn variant="tonal" prepend-icon="mdi-refresh" :loading="loading" @click="load">刷新进度</VBtn>
        <VBtn variant="tonal" prepend-icon="mdi-file-search-outline" :disabled="!reportAvailable" @click="reportVisible = true">查看并分析报告</VBtn>
      </div>
      <div v-if="progress" class="audit-progress">
        <div class="audit-progress-heading"><strong>{{ stateLabel }}</strong><span>已扫描 {{ progress.completed }} / {{ progress.total }}</span></div>
        <VProgressLinear :model-value="progress.total ? Math.round(progress.completed / progress.total * 100) : 0" color="primary" rounded height="6" aria-label="全库指纹扫描进度" />
        <dl class="audit-summary">
          <div><dt>一致跳过</dt><dd>{{ progress.counts.consistent }}</dd></div>
          <div><dt>已更新</dt><dd>{{ progress.counts.repaired }}</dd></div>
          <div><dt>无法确认</dt><dd>{{ progress.counts.unknown }}</dd></div>
        </dl>
      </div>
      <VAlert v-if="runFeedback" :type="runFeedback.type" :title="runFeedback.message" variant="tonal">
        <div v-for="detail in runFeedback.details" :key="detail">{{ detail }}</div>
      </VAlert>
      <p v-if="progress?.report_error" class="text-error">{{ progress.report_error }}</p>
      <p class="text-body-2 text-medium-emphasis">报告保存在 /config/fingerprint-scan-latest.json，只保留最新一份；可在 Web 查看原因分析、更新明细并再次补齐未确认项。后台任务支持暂停、继续和停止；停止或重启后可从保存的位置继续。</p>
    </VCardText>
    <FingerprintReport v-model:show="reportVisible" :running="running" :busy="busy"
      :current-run-id="progress?.run_id" @retry="run('retry', $event)" />
  </VCard>
</template>

<script setup>
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { useMessage } from '@/composables/useFeedback';
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
.audit-content { display:grid; gap:16px; }
.audit-content p { margin:0; }
.audit-actions { display:flex; flex-wrap:wrap; gap:10px; }
.audit-progress { padding:16px; border:1px solid rgba(var(--v-border-color), var(--v-border-opacity)); border-radius:8px; background:rgba(var(--v-theme-primary), .04); }
.audit-progress-heading { display:flex; flex-wrap:wrap; justify-content:space-between; gap:8px; margin-bottom:12px; }
.audit-progress-heading span, .audit-summary dt { color:rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity)); }
.audit-summary { display:grid; grid-template-columns:repeat(3,minmax(0,1fr)); gap:12px; margin:16px 0 0; }
.audit-summary div { min-width:0; }
.audit-summary dt { font-size:12px; }
.audit-summary dd { margin:4px 0 0; font-size:22px; font-weight:600; }
@container (max-width:520px) { .audit-actions > .v-btn { flex:1 1 160px; } .audit-progress { padding:12px; } .audit-summary { gap:8px; } }
</style>
