<template>
  <VDialog :model-value="show" max-width="1200" scrollable
    @update:model-value="$emit('update:show', $event)">
    <VCard class="fingerprint-report">
      <VCardTitle class="d-flex align-center justify-space-between ga-3">
        <span>最新指纹扫描报告</span>
        <VBtn icon="mdi-close" variant="text" size="small" aria-label="关闭指纹扫描报告" @click="$emit('update:show', false)" />
      </VCardTitle>
      <VCardText class="report-content">
        <div class="report-actions">
          <VBtn variant="tonal" prepend-icon="mdi-refresh" :loading="loading" @click="load">刷新报告</VBtn>
          <VBtn variant="tonal" prepend-icon="mdi-download" href="/api/tasks/fingerprint-scan/report?download=1" :disabled="!report">下载报告</VBtn>
          <VBtn color="primary" variant="tonal" prepend-icon="mdi-check" :loading="busy"
            :disabled="running || loading || !canRetry" @click="$emit('retry', report.run_id)">再次补齐未确认项</VBtn>
        </div>
        <VAlert v-if="error" type="error" variant="tonal">{{ error }}</VAlert>
        <div v-if="!loading && !report && !error" class="report-empty text-medium-emphasis">
          <VIcon icon="mdi-file-document-outline" size="48" aria-hidden="true" />
          <p>暂无扫描报告</p>
        </div>
        <template v-if="report">
          <p>{{ stateLabel }} · 已扫描 {{ report.completed }}/{{ report.total }} · {{ updatedAt }}</p>
          <div class="d-flex flex-wrap ga-2">
            <VChip color="success" variant="tonal">一致跳过 {{ report.counts.consistent }}</VChip>
            <VChip color="info" variant="tonal">已更新 {{ report.counts.repaired }}</VChip>
            <VChip :color="report.counts.unknown ? 'warning' : 'success'" variant="tonal">无法确认 {{ report.counts.unknown }}</VChip>
            <VChip v-if="report.remaining" variant="tonal">尚未扫描 {{ report.remaining }}</VChip>
          </div>
          <p class="text-body-2 text-medium-emphasis">未确认项尚未通过文件身份核验。再次补齐只处理这些项目，并重新查询核验；已确认项目不会重复处理。</p>
          <VAlert v-if="report.error" type="error" title="扫描中断" variant="tonal">{{ report.error }}</VAlert>
          <VAlert v-if="report.note" type="info" variant="tonal">{{ report.note }}</VAlert>
          <VAlert v-if="report.missing_details" type="warning" variant="tonal">有 {{ report.missing_details }} 项来自启用报告前的断点，历史明细未保存，请结合当时日志查看。</VAlert>
          <VAlert v-for="issue in report.analysis" :key="issue.reason" type="warning" :title="`${issue.reason}（${issue.count} 项）`" variant="tonal">
            {{ issue.advice }}
          </VAlert>
          <VAlert v-if="!report.counts.unknown && !report.remaining" type="success" variant="tonal">本轮所有项目均已核对或更新，未发现需要重试的项目。</VAlert>
          <p v-if="report.run_id !== currentRunId" class="text-body-2 text-medium-emphasis">这份报告与当前扫描断点不同，不能用它发起重试。</p>
          <div class="report-filters">
            <VSelect v-model="filter" :items="filterOptions" item-title="label" item-value="value" label="筛选报告结果" aria-label="筛选报告结果" variant="outlined" density="compact" hide-details class="report-result-filter" />
            <VTextField v-model="keyword" clearable label="搜索报告明细" placeholder="筛选路径、Emby ID 或原因" aria-label="搜索报告明细" variant="outlined" density="compact" hide-details class="report-search" />
          </div>
          <VDataTable :headers="columns" :items="rows" :items-per-page="10" :items-per-page-options="[10, 25, 50, 100]"
            :item-value="row => `${row.item_id}:${row.path}`" disable-sort fixed-header height="420" :loading="loading" density="compact" :mobile-breakpoint="0" items-per-page-text="每页条数" no-data-text="没有符合筛选条件的报告明细" loading-text="正在读取报告">
            <template #item.state="{ item }">{{ item.state === 'unknown' ? '未确认' : '已更新' }}</template>
            <template #item.error="{ item }">{{ item.error || (item.differences || []).join('、') || '旧日志未记录具体差异' }}</template>
            <template #item.size="{ item }">Emby：{{ value(item.emby_size) }}；云端：{{ value(item.new?.size) }}；原记录：{{ value(item.old?.size) }}</template>
            <template #item.pc="{ item }">{{ value(item.old?.pc) }} → {{ value(item.new?.pick_code) }}</template>
            <template #item.sha1="{ item }">{{ value(item.old?.sha1) }} → {{ value(item.new?.sha1) }}</template>
            <template #item.fid="{ item }">{{ value(item.new?.id) }}</template>
          </VDataTable>
          <p class="text-body-2 text-medium-emphasis">一致项仅汇总数量；表格展示未确认项和已更新项。报告保存在 /config/fingerprint-scan-latest.json，新扫描覆盖旧报告。</p>
        </template>
      </VCardText>
    </VCard>
  </VDialog>
</template>

<script setup>
import { computed, ref, watch } from 'vue';
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
  [row.item_id, row.path, row.error, ...(row.differences || [])].join(' ').toLowerCase().includes((keyword.value || '').trim().toLowerCase())
));
const value = input => input ?? '未记录';
const columns = [
  { title: '结果', key: 'state', width: 90 },
  { title: 'Emby ID', key: 'item_id', width: 90 },
  { title: '路径', key: 'path', width: 340 },
  { title: '原因 / 差异', key: 'error', width: 250 },
  { title: '大小（字节）', key: 'size', width: 210, sortable: false },
  { title: 'PC（旧 → 新）', key: 'pc', width: 260, sortable: false },
  { title: 'SHA1（旧 → 新）', key: 'sha1', width: 370, sortable: false },
  { title: '当前 FID', key: 'fid', width: 180, sortable: false },
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
.report-content { display:grid; gap:16px; }
.report-content p { margin:0; }
.report-actions { display:flex; flex-wrap:wrap; gap:10px; }
.report-empty { display:grid; justify-items:center; gap:12px; padding:32px 0; }
.report-filters { display:flex; flex-wrap:wrap; gap:12px; }
.report-result-filter { flex:0 1 170px; min-width:0; }
.report-search { flex:1 1 250px; min-width:0; }
:deep(.v-data-table__td) { overflow-wrap:anywhere; }
@container (max-width:520px) { .report-actions > .v-btn { flex:1 1 170px; } .report-result-filter, .report-search { flex-basis:100%; } }
</style>
