<template>
  <section class="history-log-view" aria-label="历史日志">
    <div class="history-search-toolbar">
      <VTextField v-model="searchQuery" label="在所有日志文件中搜索" placeholder="输入关键词" clearable hide-details density="compact" prepend-inner-icon="mdi-magnify" :disabled="isLoading" @keyup.enter="executeSearch" />
      <VBtn variant="tonal" prepend-icon="mdi-magnify" :loading="isSearching" :disabled="isLoading" @click="executeSearch">搜索</VBtn>
    </div>
    <VRadioGroup v-model="searchMode" inline hide-details :disabled="isLoadingFiles || isLoadingContent">
      <VRadio label="匹配日志行" value="filter" />
      <VRadio label="完整上下文" value="context" />
    </VRadioGroup>
    <div class="history-toolbar">
      <VBtn v-if="isSearchMode" variant="tonal" size="small" prepend-icon="mdi-arrow-left" @click="clearSearch">返回文件浏览</VBtn>
      <VSelect v-else v-model="selectedFile" :items="logFiles" label="日志文件" :loading="isLoadingFiles" :disabled="isLoading" hide-details density="compact" class="history-file-select" @update:model-value="filename => fetchLogContent(filename)" />
      <VSelect v-model="levelFilter" :items="levelOptions" label="日志级别" hide-details density="compact" class="history-level-select" />
      <VBtn icon="mdi-refresh" variant="text" size="small" aria-label="刷新历史日志" :loading="isLoading" @click="refreshLogs" />
      <VBtn variant="tonal" size="small" prepend-icon="mdi-download" :disabled="!displayedRecords.length" @click="downloadLogs">导出日志</VBtn>
    </div>
    <div v-if="!isSearchMode && logPages > 1" class="history-pagination">
      <VBtn variant="tonal" size="small" prepend-icon="mdi-chevron-left" :disabled="isLoading || logPage <= 1" @click="fetchLogContent(selectedFile, logPage - 1)">较新日志</VBtn>
      <span>第 {{ logPage }} / {{ logPages }} 页，共 {{ logTotalLines }} 行</span>
      <VBtn variant="tonal" size="small" append-icon="mdi-chevron-right" :disabled="isLoading || logPage >= logPages" @click="fetchLogContent(selectedFile, logPage + 1)">较早日志</VBtn>
    </div>
    <VAlert v-if="errorMessage" type="error" variant="tonal">{{ errorMessage }}</VAlert>
    <VProgressLinear v-if="isLoading" indeterminate color="primary" :aria-label="loadingText" />
    <div ref="scrollRef" class="history-log-list" role="region" aria-label="历史日志内容" tabindex="0">
      <LogRecord v-for="(entry, index) in displayedRecords" :key="index" :line="entry.line" :file="entry.file" :keyword="isSearchMode ? appliedQuery : ''" />
      <div v-if="!isLoading && !displayedRecords.length" class="history-empty"><VIcon icon="mdi-text-search" size="40" /><p>{{ isSearchMode ? '未找到匹配的日志记录' : levelFilter === 'all' ? '暂无日志内容' : '没有匹配的日志' }}</p></div>
    </div>
  </section>
</template>

<script setup>
import { ref, watch, computed, onUnmounted, nextTick } from 'vue';
import axios from 'axios';
import LogRecord from './LogRecord.vue';
import { matchesLogRecord } from '@/utils/logRecords';

const props = defineProps({ show: { type: Boolean, default: false } });
const isLoadingFiles = ref(false), isLoadingContent = ref(false), isSearching = ref(false);
const logFiles = ref([]), selectedFile = ref(null), logContent = ref([]), searchResults = ref([]);
const logPage = ref(1), logPages = ref(1), logTotalLines = ref(0);
const searchQuery = ref(''), appliedQuery = ref(''), searchMode = ref('context'), isSearchMode = ref(false);
const levelFilter = ref('all'), errorMessage = ref(''), scrollRef = ref(null);
const levelOptions = [{ title: '全部日志', value: 'all' }, { title: 'DEBUG', value: 'debug' }, { title: 'INFO', value: 'info' }, { title: '警告', value: 'warning' }, { title: '错误', value: 'error' }];
let readSequence = 0, fileSequence = 0, pageActive = true;
const isLoading = computed(() => isLoadingFiles.value || isLoadingContent.value || isSearching.value);
const loadingText = computed(() => isLoadingFiles.value ? '正在获取文件列表' : isSearching.value ? '正在搜索日志' : '正在读取日志');
const displayedRecords = computed(() => (isSearchMode.value ? searchResults.value : logContent.value)
  .filter(entry => matchesLogRecord(entry.line, '', levelFilter.value)));

const fetchLogFiles = async () => {
  const sequence = ++fileSequence;
  isLoadingFiles.value = true;
  try {
    const { data } = await axios.get('/api/logs/list');
    if (!pageActive || sequence !== fileSequence) return;
    if (!Array.isArray(data) || data.some(file => typeof file !== 'string')) throw new Error('日志文件列表格式无效');
    logFiles.value = data;
    if (!data.includes(selectedFile.value)) selectedFile.value = data[0] || null;
    if (!selectedFile.value) logContent.value = [];
  } catch (error) {
    if (pageActive && sequence === fileSequence) errorMessage.value = error.response?.data?.error || error.message || '获取日志文件列表失败';
  } finally {
    if (pageActive && sequence === fileSequence) isLoadingFiles.value = false;
  }
};

const fetchLogContent = async (filename, page = 1) => {
  if (!filename) return;
  const sequence = ++readSequence;
  isLoadingContent.value = true;
  errorMessage.value = '';
  try {
    const { data } = await axios.get('/api/logs/view', { params: { filename, page, format: 'json' } });
    if (!pageActive || sequence !== readSequence) return;
    if (!Array.isArray(data?.lines) || data.lines.some(line => typeof line !== 'string')) throw new Error('日志内容格式无效');
    logContent.value = data.lines.map(line => ({ line, file: filename }));
    logPage.value = Number(data.page) || 1;
    logPages.value = Number(data.pages) || 1;
    logTotalLines.value = Number(data.total_lines) || 0;
    await nextTick();
    if (scrollRef.value) scrollRef.value.scrollTop = 0;
  } catch (error) {
    if (pageActive && sequence === readSequence) errorMessage.value = error.response?.data?.error || error.message || `加载日志 ${filename} 失败`;
  } finally {
    if (pageActive && sequence === readSequence) isLoadingContent.value = false;
  }
};

const executeSearch = async () => {
  if (isLoadingFiles.value || isLoadingContent.value) return;
  const query = String(searchQuery.value || '').trim();
  if (!query) { clearSearch(); return; }
  const sequence = ++readSequence;
  const mode = searchMode.value;
  isSearching.value = true;
  errorMessage.value = '';
  try {
    const { data } = await axios.get(mode === 'context' ? '/api/logs/search_context' : '/api/logs/search', { params: { q: query } });
    if (!pageActive || sequence !== readSequence || mode !== searchMode.value) return;
    if (!Array.isArray(data)) throw new Error('日志搜索结果格式无效');
    const entries = mode === 'context'
      ? data.flatMap(block => (block.lines || []).map(line => ({ line, file: block.file || '' })))
      : data.map(entry => ({ line: entry.content, file: entry.file || '' }));
    if (entries.some(entry => typeof entry.line !== 'string')) throw new Error('日志搜索内容格式无效');
    searchResults.value = entries;
    appliedQuery.value = query;
    isSearchMode.value = true;
    await nextTick();
    if (scrollRef.value) scrollRef.value.scrollTop = 0;
  } catch (error) {
    if (pageActive && sequence === readSequence) errorMessage.value = error.response?.data?.error || error.message || '搜索日志失败';
  } finally {
    if (pageActive && sequence === readSequence) isSearching.value = false;
  }
};

const clearSearch = () => {
  readSequence++;
  isSearching.value = false;
  isSearchMode.value = false;
  searchQuery.value = '';
  appliedQuery.value = '';
  searchResults.value = [];
  errorMessage.value = '';
};
async function refreshLogs() {
  if (isLoading.value) return;
  if (isSearchMode.value) return executeSearch();
  errorMessage.value = '';
  await fetchLogFiles();
  if (selectedFile.value) await fetchLogContent(selectedFile.value);
}
function downloadLogs() {
  const url = URL.createObjectURL(new Blob([displayedRecords.value.map(entry => entry.line).join('\n')], { type: 'text/plain;charset=utf-8' }));
  const link = document.createElement('a');
  link.href = url; link.download = selectedFile.value || 'etk-search.log'; link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
watch(() => props.show, async visible => { if (visible && !logFiles.value.length && !isLoading.value) await refreshLogs(); }, { immediate: true });
watch(searchMode, () => { if (isSearchMode.value || isSearching.value) executeSearch(); }, { flush: 'sync' });
onUnmounted(() => { pageActive = false; readSequence++; fileSequence++; });
</script>

<style scoped>
.history-log-view { display:flex; flex:1; flex-direction:column; gap:12px; min-height:0; min-width:0; overflow:auto; }
.history-search-toolbar, .history-toolbar { display:flex; flex-wrap:wrap; align-items:center; gap:10px; flex-shrink:0; }
.history-search-toolbar > .v-input { flex:1; min-width:160px; }
.history-file-select { flex:1 1 200px; min-width:0; }
.history-level-select { flex:0 1 140px; min-width:110px; }
.history-log-view > .v-input, .history-log-view > .v-alert, .history-log-view > .v-progress-linear { flex-shrink:0; }
.history-pagination { display:flex; flex-wrap:wrap; align-items:center; justify-content:space-between; gap:8px; flex-shrink:0; }
.history-pagination span { color:var(--ui-text-muted); font-size:12px; }
.history-log-list { display:flex; flex-direction:column; gap:12px; flex:1 0 160px; min-height:160px; min-width:0; overflow:auto; overscroll-behavior:contain; padding:8px 4px; }
.history-empty { display:flex; flex-direction:column; align-items:center; justify-content:center; gap:12px; padding:32px 8px; color:var(--ui-text-muted); }
@media (max-width:540px) { .history-search-toolbar > .v-input { flex-basis:100%; } .history-log-list { padding:4px; } }
</style>
