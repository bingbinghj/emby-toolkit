<!-- src/components/LogViewer.vue -->
<template>
  <section class="history-log-view" aria-label="历史日志">
      <div class="drawer-inner-wrapper">

        <!-- 顶部控制区 -->
        <n-space vertical class="header-section">
          <n-input-group>
            <n-input
              v-model:value="searchQuery"
              placeholder="在所有日志文件中搜索..."
              clearable
              @keyup.enter="executeSearch"
              :disabled="isLoading"
            ><template #prefix><UiIcon name="search" /></template></n-input>
            <n-button type="primary" @click="executeSearch" :loading="isSearching">
              <template #icon><UiIcon name="search" /></template>
              搜索
            </n-button>
          </n-input-group>

          <n-radio-group v-model:value="searchMode" name="search-mode-radio">
            <n-radio-button value="filter" :disabled="isLoading">
              <UiIcon name="options" :size="16" />
              匹配日志行
            </n-radio-button>
            <n-radio-button value="context" :disabled="isLoading">
              <UiIcon name="time" :size="16" />
              完整上下文
            </n-radio-button>
          </n-radio-group>
        </n-space>

        <n-divider style="margin: 10px 0;" />

        <n-spin :show="isLoading" class="flex-spin">

          <!-- 场景 1: 定位模式 (HTML iframe) -->
          <div v-if="isSearchMode && searchMode === 'context'" class="view-container">
             <div class="toolbar">
               <n-button @click="clearSearch" size="tiny" secondary>
                 <template #icon><UiIcon name="back" /></template>
                 返回文件浏览
               </n-button>
               <span class="tip">已隐藏日期与模块名，仅显示核心流</span>
             </div>

            <div v-if="htmlContent" class="iframe-wrapper">
              <iframe :srcdoc="htmlContent" title="日志搜索上下文" frameborder="0" width="100%" height="100%" @load="applyFrameTheme"></iframe>
            </div>
            <n-empty v-else description="未找到匹配的完整处理流程。" style="margin-top: 50px;" />
          </div>

          <!-- 场景 2: 筛选模式 (JSON) - 保持不变，用于快速查看原始行 -->
          <div v-else-if="isSearchMode && searchMode === 'filter'" class="view-container">
            <n-button @click="clearSearch" size="tiny" secondary style="margin-bottom: 5px;">
              <template #icon><UiIcon name="back" /></template>
              返回文件浏览
            </n-button>

            <div v-if="hasSearchResults" class="log-text-area">
              <div
                v-for="(line, index) in parsedLogResults"
                :key="index"
                class="log-line"
                :class="line.type === 'log' ? line.level.toLowerCase() : 'raw'"
              >
                <template v-if="line.type === 'log'">
                  <span class="timestamp">{{ line.timestamp }}</span>
                  <span class="level">{{ line.level }}</span>
                  <span class="message">{{ line.message }}</span>
                </template>
                <template v-else>{{ line.content }}</template>
              </div>
            </div>
            <n-empty v-else description="未找到匹配的日志记录。" style="margin-top: 50px;" />
          </div>

          <!-- ★★★ 场景 3: 文件浏览 (默认) - 改为 HTML iframe ★★★ -->
          <div v-else class="view-container">
            <div class="toolbar">
              <n-select
                v-model:value="selectedFile"
                placeholder="请选择日志文件"
                :options="fileOptions"
                :loading="isLoadingFiles"
                @update:value="filename => fetchLogContent(filename)"
                size="small"
                class="history-file-select"
              />
              <span class="tip"><UiIcon name="time" :size="14" /> 最新日志在顶部</span>
              <n-button quaternary circle aria-label="刷新历史日志" title="刷新" :loading="isLoading" @click="refreshLogs"><template #icon><n-icon :component="RefreshOutline" /></template></n-button>
            </div>
            <div v-if="logPages > 1" class="history-pagination">
              <n-button size="tiny" :disabled="isLoading || logPage <= 1" @click="fetchLogContent(selectedFile, logPage - 1)"><template #icon><UiIcon name="time" :size="14" /></template>较新日志</n-button>
              <span>第 {{ logPage }} / {{ logPages }} 页，共 {{ logTotalLines }} 行</span>
              <n-button size="tiny" :disabled="isLoading || logPage >= logPages" @click="fetchLogContent(selectedFile, logPage + 1)"><template #icon><UiIcon name="time" :size="14" /></template>较早日志</n-button>
            </div>

            <!-- 这里复用 iframe-wrapper -->
            <div v-if="logContent" class="iframe-wrapper">
              <iframe :srcdoc="logContent" title="历史日志内容" frameborder="0" width="100%" height="100%" @load="applyFrameTheme"></iframe>
            </div>
            <n-empty v-else description="无数据" style="margin-top: 50px;" />
          </div>

          <template #description>{{ loadingText }}</template>
        </n-spin>
      </div>
  </section>
</template>

<script setup>
import { ref, watch, computed, onMounted, onUnmounted } from 'vue';
import axios from 'axios';
import {
  useMessage, NSelect, NSpace, NSpin,
  NInput, NInputGroup, NButton, NDivider, NEmpty, NIcon,
  NRadioGroup, NRadioButton
} from 'naive-ui';
import { RefreshOutline } from '@vicons/ionicons5';

const props = defineProps({ show: { type: Boolean, default: false } });

const message = useMessage();
const isLoadingFiles = ref(false);
const isLoadingContent = ref(false);
const isSearching = ref(false);
const logFiles = ref([]);
const selectedFile = ref(null);
const logContent = ref(''); // 现在这里存储的是 HTML 字符串
const logPage = ref(1), logPages = ref(1), logTotalLines = ref(0);
const searchQuery = ref('');
const searchResults = ref([]);
const htmlContent = ref(''); // 搜索结果的 HTML
const isSearchMode = ref(false);
const searchMode = ref('context');
let themeObserver;
let readSequence = 0, searchSequence = 0;

function applyFrameTheme(event) {
  const frame = event?.target;
  if (!frame?.contentDocument) return;
  const vars = getComputedStyle(document.documentElement);
  let style = frame.contentDocument.querySelector('#etk-log-theme');
  if (!style) {
    style = frame.contentDocument.createElement('style');
    style.id = 'etk-log-theme';
    (frame.contentDocument.head || frame.contentDocument.documentElement).appendChild(style);
  }
  const color = key => vars.getPropertyValue(key).trim();
  const css = `:root{--bg-color:${color('--ui-input-bg')};--text-color:${color('--ui-text')};--time-color:${color('--ui-text-muted')};--debug-color:${color('--ui-text-muted')};--info-color:${color('--ui-text')};--warn-color:${color('--ui-warning-text')};--error-color:${color('--ui-error-text')};--arrow-color:${color('--ui-primary-text')};--highlight-bg:${color('--ui-primary-solid')}}
body{background:var(--bg-color)!important;color:var(--text-color)!important;margin:0!important;padding:14px!important;height:auto!important;line-height:1.75!important}h3,.block-header{color:var(--time-color)!important;border-color:${color('--ui-border')}!important}pre,.msg{overflow-wrap:anywhere!important;word-break:break-word!important}.log-block{border-color:${color('--ui-border')}!important}.line:hover{background:${color('--accent-glow-color')}!important}.ts{opacity:1!important}mark,.keyword{background:var(--highlight-bg)!important;color:${color('--ui-on-primary')}!important}`;
  if (style.textContent !== css) style.textContent = css;
}
onMounted(() => {
  themeObserver = new MutationObserver(() => {
    document.querySelectorAll('.history-log-view iframe').forEach(target => applyFrameTheme({ target }));
  });
  themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['style', 'class', 'data-theme'] });
});
onUnmounted(() => themeObserver?.disconnect());

const isLoading = computed(() => isLoadingFiles.value || isLoadingContent.value || isSearching.value);
const hasSearchResults = computed(() => searchResults.value.length > 0);
const fileOptions = computed(() => logFiles.value.map(file => ({ label: file, value: file })));
const loadingText = computed(() => {
  if (isLoadingFiles.value) return '正在获取文件列表...';
  if (isLoadingContent.value) return '正在渲染日志视图...';
  if (isSearching.value) return '正在搜索...';
  return '';
});

// 仅用于筛选模式的简单解析
const parseLogLine = (line) => {
  const match = line.match(/^(\d{4}-\d{2}-\d{2}\s(\d{2}:\d{2}:\d{2})),\d+\s-\s.+?\s-\s(DEBUG|INFO|WARNING|ERROR|CRITICAL)\s-\s(.*)$/);
  if (match) {
    return { type: 'log', timestamp: match[2], level: match[3], message: match[4].trim() };
  }
  return { type: 'raw', content: line };
};

const parsedLogResults = computed(() => {
  if (!hasSearchResults.value || searchMode.value === 'context') return [];
  const finalLines = [];
  finalLines.push(`以“筛选”模式找到 ${searchResults.value.length} 条结果:`);
  let lastFile = '';
  searchResults.value.forEach(result => {
    if (result.file !== lastFile) {
      if (finalLines.length > 1) finalLines.push('');
      finalLines.push(`--- [ 文件: ${result.file} ] ---`);
      lastFile = result.file;
    }
    finalLines.push(result.content);
  });
  return finalLines.map(parseLogLine);
});

const fetchLogFiles = async () => {
  isLoadingFiles.value = true;
  try {
    const response = await axios.get('/api/logs/list');
    logFiles.value = Array.isArray(response.data) ? response.data : [];
    if (!isSearchMode.value && logFiles.value.length > 0 && !selectedFile.value) {
      selectedFile.value = logFiles.value[0];
      await fetchLogContent(selectedFile.value);
    }
  } catch (error) {
    message.error('获取日志文件列表失败！');
  } finally {
    isLoadingFiles.value = false;
  }
};

// ★★★ 修改：请求 HTML 格式 ★★★
const fetchLogContent = async (filename, page = 1) => {
  if (!filename) return;
  const sequence = ++readSequence;
  isLoadingContent.value = true;
  logContent.value = ''; // 清空旧内容
  try {
    const response = await axios.get('/api/logs/view', {
      params: {
        filename,
        page,
        format: 'html' // 告诉后端我要 HTML
      },
      responseType: 'text'
    });
    if (sequence === readSequence) {
      logContent.value = response.data;
      logPage.value = Number(response.headers?.['x-log-page']) || 1;
      logPages.value = Number(response.headers?.['x-log-pages']) || 1;
      logTotalLines.value = Number(response.headers?.['x-log-total-lines']) || 0;
    }
  } catch (error) {
    if (sequence === readSequence) message.error(`加载日志 ${filename} 失败！`);
  } finally {
    if (sequence === readSequence) isLoadingContent.value = false;
  }
};

const executeSearch = async () => {
  if (isLoading.value) return;
  if (!searchQuery.value.trim()) {
    message.warning('请输入搜索关键词。');
    return;
  }
  isSearching.value = true;
  const sequence = ++searchSequence;
  isSearchMode.value = true;
  searchResults.value = [];
  htmlContent.value = '';

  try {
    if (searchMode.value === 'context') {
      const response = await axios.get('/api/logs/search_context', {
        params: { q: searchQuery.value, format: 'html' },
        responseType: 'text'
      });
      if (sequence !== searchSequence) return;
      if (typeof response.data === 'string' && response.data.trim()) {
        htmlContent.value = response.data;
      } else {
        htmlContent.value = '';
      }
    } else {
      const response = await axios.get('/api/logs/search', { params: { q: searchQuery.value } });
      if (sequence === searchSequence) searchResults.value = Array.isArray(response.data) ? response.data : [];
    }
  } catch (error) {
    console.error(error);
    message.error('搜索失败，请检查后台日志。');
  } finally {
    if (sequence === searchSequence) isSearching.value = false;
  }
};

const clearSearch = () => {
  searchSequence++;
  isSearching.value = false;
  isSearchMode.value = false;
  searchQuery.value = '';
  searchResults.value = [];
  htmlContent.value = '';
  if (selectedFile.value && !logContent.value) {
    fetchLogContent(selectedFile.value);
  }
};

watch(() => props.show, visible => { if (visible && !logFiles.value.length) fetchLogFiles(); }, { immediate: true });
watch(searchMode, () => { if (isSearchMode.value && searchQuery.value.trim()) executeSearch(); });
async function refreshLogs() {
  if (isLoading.value) return;
  await fetchLogFiles();
  if (selectedFile.value) await fetchLogContent(selectedFile.value);
}
</script>

<style scoped>
/* 保持之前的 CSS 样式不变，这里是完美的 */
.history-log-view { display:flex; flex:1; min-height:0; min-width:0; }
.drawer-inner-wrapper {
  height: 100%;
  display: flex;
  flex-direction: column;
  width:100%;
  min-width:0;
  padding:0;
  box-sizing: border-box;
}

.header-section { flex-shrink: 0; }
.history-pagination { display:flex; align-items:center; justify-content:space-between; gap:8px; margin-bottom:8px; flex-shrink:0; }
.history-pagination span { color:var(--ui-text-muted); font-size:12px; }

.flex-spin {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  min-height: 0;
}

:deep(.n-spin-content) {
  height: 100%;
  display: flex;
  flex-direction: column;
}

.view-container {
  flex: 1;
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
}

.toolbar {
  flex-shrink: 0;
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 8px;
  gap:12px;
  flex-wrap:wrap;
}
.history-file-select { width:min(260px,100%); flex:1; min-width:0; }

.tip { font-size:12px; color:var(--ui-text-muted); }

.iframe-wrapper {
  flex: 1;
  border:1px solid var(--ui-border);
  border-radius:10px;
  overflow: hidden;
  background-color:var(--ui-surface-muted);
  height: 100%;
  display: flex;
}

iframe { flex: 1; display: block; }

.log-text-area {
  flex: 1;
  background-color:var(--ui-surface-muted);
  font-family: 'Courier New', Courier, monospace;
  font-size: 13px;
  padding: 10px 15px;
  border:1px solid var(--ui-border);
  border-radius:10px;
  overflow-y: auto;
  white-space: pre-wrap;
  word-break: break-all;
}

.log-line { line-height:1.8; padding:3px 0; color:var(--ui-text); }
.log-line.warning { color:var(--ui-warning-text); }
.log-line.error, .log-line.critical { color:var(--ui-error-text); }
.log-line.debug, .log-line.raw { color:var(--ui-text-muted); }
.timestamp { color:var(--ui-text-muted); margin-right:1em; }
.level { font-weight: bold; margin-right: 1em; text-transform: uppercase; }
@media (max-width:540px) { .toolbar { gap:8px; } .tip { display:none; } }
</style>
