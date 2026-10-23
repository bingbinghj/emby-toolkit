<template>
  <n-modal :show="show" preset="card" title="最新指纹扫描报告" class="fingerprint-report"
    :style="{ width: 'min(1200px, 96vw)' }" @update:show="$emit('update:show', $event)">
    <n-space vertical :size="16">
      <div class="report-actions">
        <n-button :loading="loading" @click="load">刷新报告</n-button>
        <n-button tag="a" href="/api/tasks/fingerprint-scan/report?download=1" :disabled="!report">下载报告</n-button>
        <n-button type="primary" secondary :loading="busy"
          :disabled="running || loading || !canRetry" @click="$emit('retry', report.run_id)">再次补齐未确认项</n-button>
      </div>
      <n-alert v-if="error" type="error">{{ error }}</n-alert>
      <n-empty v-if="!loading && !report && !error" description="暂无扫描报告" />
      <template v-if="report">
        <n-text>{{ stateLabel }} · 已扫描 {{ report.completed }}/{{ report.total }} · {{ updatedAt }}</n-text>
        <n-space>
          <n-tag type="success">一致跳过 {{ report.counts.consistent }}</n-tag>
          <n-tag type="info">已更新 {{ report.counts.repaired }}</n-tag>
          <n-tag :type="report.counts.unknown ? 'warning' : 'success'">无法确认 {{ report.counts.unknown }}</n-tag>
          <n-tag v-if="report.remaining">尚未扫描 {{ report.remaining }}</n-tag>
        </n-space>
        <n-text depth="3">未确认项尚未通过文件身份核验。再次补齐只处理这些项目，并重新查询核验；已确认项目不会重复处理。</n-text>
        <n-alert v-if="report.error" type="error" title="扫描中断">{{ report.error }}</n-alert>
        <n-alert v-if="report.note" type="info">{{ report.note }}</n-alert>
        <n-alert v-if="report.missing_details" type="warning">有 {{ report.missing_details }} 项来自启用报告前的断点，历史明细未保存，请结合当时日志查看。</n-alert>
        <n-alert v-for="issue in report.analysis" :key="issue.reason" type="warning" :title="`${issue.reason}（${issue.count} 项）`">
          {{ issue.advice }}
        </n-alert>
        <n-alert v-if="!report.counts.unknown && !report.remaining" type="success">本轮所有项目均已核对或更新，未发现需要重试的项目。</n-alert>
        <n-text v-if="report.run_id !== currentRunId" depth="3">这份报告与当前扫描断点不同，不能用它发起重试。</n-text>
        <div class="report-filters">
          <n-select v-model:value="filter" :options="filterOptions" aria-label="筛选报告结果" class="report-result-filter" />
          <n-input v-model:value="keyword" clearable placeholder="筛选路径、Emby ID 或原因" :input-props="{ 'aria-label': '搜索报告明细' }" class="report-search" />
        </div>
        <n-data-table :columns="columns" :data="rows" :pagination="{ pageSize: 10 }" :scroll-x="1250"
          :row-key="row => `${row.item_id}:${row.path}`" :max-height="420" :loading="loading" size="small" />
        <n-text depth="3">一致项仅汇总数量；表格展示未确认项和已更新项。报告保存在 /config/fingerprint-scan-latest.json，新扫描覆盖旧报告。</n-text>
      </template>
    </n-space>
  </n-modal>
</template>

<script setup>
import { computed, ref, watch } from 'vue';
import { NAlert, NButton, NDataTable, NEmpty, NInput, NModal, NSelect, NSpace, NTag, NText } from 'naive-ui';
import axios from 'axios';

const props = defineProps({ show: Boolean, running: Boolean, busy: Boolean, currentRunId: String });
defineEmits(['update:show', 'retry']);
const report = ref(null), loading = ref(false), error = ref(''), filter = ref('unknown'), keyword = ref('');
let sequence = 0;
const filterOptions = [{ label: '未确认项', value: 'unknown' }, { label: '已更新项', value: 'repaired' }, { label: '全部明细', value: 'all' }];
const stateLabel = computed(() => ({ running: '执行中（已保存进度）', interrupted: '已中断，可继续', complete: '已完成', attention: '已完成，有未确认项' }[report.value?.state] || ''));
const updatedAt = computed(() => report.value?.updated_at ? new Date(report.value.updated_at * 1000).toLocaleString('zh-CN') : '');
const canRetry = computed(() => report.value?.run_id === props.currentRunId && report.value?.counts.unknown > 0 && !report.value.remaining);
const rows = computed(() => (report.value?.entries || []).filter(row =>
  (filter.value === 'all' || row.state === filter.value) &&
  [row.item_id, row.path, row.error, ...(row.differences || [])].join(' ').toLowerCase().includes(keyword.value.trim().toLowerCase())
));
const value = input => input ?? '未记录';
const columns = [
  { title: '结果', key: 'state', width: 90, render: row => row.state === 'unknown' ? '未确认' : '已更新' },
  { title: 'Emby ID', key: 'item_id', width: 90 },
  { title: '路径', key: 'path', width: 340, resizable: true },
  { title: '原因 / 差异', key: 'error', width: 250, render: row => row.error || (row.differences || []).join('、') || '旧日志未记录具体差异' },
  { title: '大小（字节）', key: 'size', width: 210, render: row => `Emby：${value(row.emby_size)}；云端：${value(row.new?.size)}；原记录：${value(row.old?.size)}` },
  { title: 'PC（旧 → 新）', key: 'pc', width: 260, render: row => `${value(row.old?.pc)} → ${value(row.new?.pick_code)}` },
  { title: 'SHA1（旧 → 新）', key: 'sha1', width: 370, render: row => `${value(row.old?.sha1)} → ${value(row.new?.sha1)}` },
  { title: '当前 FID', key: 'fid', width: 180, render: row => value(row.new?.id) },
];
async function load() {
  const current = ++sequence;
  loading.value = true;
  error.value = '';
  try {
    const { data } = await axios.get('/api/tasks/fingerprint-scan/report');
    if (current === sequence) report.value = data;
  } catch (err) {
    if (current === sequence) { report.value = null; error.value = err.response?.data?.error || '读取报告失败'; }
  } finally { if (current === sequence) loading.value = false; }
}
watch(() => props.show, visible => { if (visible) load(); });
watch(() => props.running, () => { if (props.show) load(); });
</script>

<style scoped>
.fingerprint-report { container-type:inline-size; }
.report-actions { display:flex; flex-wrap:wrap; gap:10px; }
.report-filters { display:flex; flex-wrap:wrap; gap:12px; }
.report-result-filter { flex:0 1 170px; min-width:0; }
.report-search { flex:1 1 250px; min-width:0; }
:deep(.n-data-table-td) { overflow-wrap:anywhere; }
@container (max-width:520px) { .report-actions > .n-button { flex:1 1 170px; } .report-result-filter, .report-search { flex-basis:100%; } }
</style>
