<!-- src/components/ReviewList.vue -->
<template>
  <n-layout class="mp-page review-page" content-style="padding: 0;">
  <n-page-header class="mp-page-header" subtitle="搜索媒体或检查待复核项目"><template #title><UiIcon name="edit" /> 手动处理</template></n-page-header>
  <n-card class="dashboard-card" :bordered="false" size="small">

    <n-alert title="操作提示" type="info" style="margin-bottom: 20px;">
          可在下方搜索框输入片名直接搜索处理，也可点击待复核列表媒体项进入手动编辑页面、可一键清空待复核列表转到已处理记录。
    </n-alert>
    <n-alert
      v-if="props.taskStatus?.is_running"
      title="后台任务运行中"
      type="warning"
      style="margin-bottom: 20px;"
      closable
    >
      后台任务正在运行，此时“手动处理”等操作可能会失败。
    </n-alert>

    <div>
      <!-- ★ 新增：搜索框和筛选框放在同一行 -->
      <div class="review-toolbar">
        <div class="review-search-field">
          <input
            v-model="searchQuery"
            class="review-search-input"
            type="text"
            placeholder="输入媒体名称搜索整个 Emby 库..."
            autocomplete="off"
            spellcheck="false"
            @keyup.enter="handleSearch"
            @input="handleSearchQueryChange($event.target.value)"
          />
          <button
            v-if="searchQuery"
            type="button"
            class="review-search-action review-search-clear"
            title="清空搜索"
            aria-label="清空搜索"
            @click="clearSearch"
          >
            <n-icon :component="ClearIcon" />
          </button>
          <button
            type="button"
            class="review-search-action review-search-submit"
            title="搜索"
            aria-label="搜索"
            @click="handleSearch"
          >
            <n-icon :component="SearchIcon" />
          </button>
        </div>

        <n-select
          v-if="isShowingSearchResults"
          v-model:value="searchSort"
          :options="searchSortOptions"
          aria-label="搜索结果排序"
          class="review-filter"
          @update:value="currentPage = 1"
        />

        <n-select
          v-model:value="selectedMediaType"
          :options="mediaTypeOptions"
          aria-label="按媒体类型筛选"
          placeholder="全部媒体类型"
          clearable
          class="review-filter"
          @update:value="handleMediaTypeChange"
        />

        <n-popconfirm
            @positive-click="clearAllReviewItems"
            :positive-button-props="{ type: 'error' }"
          >
            <template #trigger>
              <n-button type="error" ghost :disabled="tableData.length === 0 || loading || isShowingSearchResults">
                <template #icon><n-icon :component="TrashIcon" /></template>
                清空待复核
              </n-button>
            </template>
            确定要清空全部待复核记录吗？此操作不可恢复。
        </n-popconfirm>

        <n-popconfirm
            @positive-click="reprocessAllReviewItems"
        >
            <template #trigger>
                <n-button type="warning" ghost :disabled="tableData.length === 0 || loading || props.taskStatus?.is_running || isShowingSearchResults">
                    <template #icon><n-icon :component="ReprocessIcon" /></template>
                    <!-- ★ 动态按钮文字 -->
                    {{ selectedMediaType ? `重新处理 ${totalItems} 项` : '重新处理所有' }}
                </n-button>
            </template>
            确定要重新处理 {{ selectedMediaType ? '当前筛选出的' : '所有' }} {{ totalItems }} 条记录吗？
        </n-popconfirm>
      </div>

      <n-spin :show="loading">
        <div v-if="error" class="error-message">
          <n-alert title="加载错误" type="error">{{ error }}</n-alert>
        </div>
        <div v-else ref="resultsContainer" class="review-results">
          <div v-if="tableData.length > 0 && compactList" class="review-cards">
            <article v-for="row in cardRows" :key="row.item_id" class="review-item-card">
              <button type="button" class="review-item-title" @click="goToEditPage(row)"><UiIcon name="media" /> {{ row.item_name || '未知名称' }}</button>
              <p class="review-item-meta">{{ [row.series_name !== row.item_name ? row.series_name : '', reviewEpisodeLabel(row), row.production_year, `ID: ${row.item_id}`].filter(Boolean).join(' · ') }}</p>
              <p v-if="isShowingSearchResults" class="review-item-meta">{{ row.library_name || '未知媒体库' }}</p>
              <template v-else>
                <p class="review-item-reason">{{ row.reason || '未记录原因' }}</p>
                <p class="review-item-meta">记录时间：{{ formatDate(row.failed_at) }}</p>
              </template>
              <ReviewActions :row="row" />
            </article>
            <n-pagination :page="currentPage" :page-size="itemsPerPage" :item-count="totalItems" simple
              @update:page="paginationProps.onChange" />
          </div>
          <n-data-table
            v-else-if="tableData.length > 0"
            :columns="columns"
            :data="displayedTableData"
            :pagination="paginationProps"
            :bordered="false"
            :single-line="false"
            striped
            size="small"
            :row-key="row => row.item_id"
            :loading="loadingAction[currentRowId]"
            :remote="!isShowingSearchResults"
            :scroll-x="isShowingSearchResults ? 820 : 1100"
          />
          <n-empty
            v-else-if="!loading && tableData.length === 0"
            :description="isShowingSearchResults ? '在 Emby 库中未找到匹配项。' : '太棒了！没有需要手动处理的媒体项。'"
            style="margin-top: 50px; margin-bottom: 30px;"
          />
        </div>
      </n-spin>
    </div>
  </n-card>
  </n-layout>
</template>

<script setup>
import { useRouter } from 'vue-router';
import { ref, onMounted, onUnmounted, computed, watch, h } from 'vue';
import axios from 'axios';
import { sortReviewSearchResults, reviewEpisodeLabel } from '../utils/reviewSearch';
import {
    NCard, NSpin, NAlert, NText, NDataTable, NButton, NSpace, NPopconfirm, NEmpty, NIcon, NSelect, NPageHeader, NPagination,
    useMessage
} from 'naive-ui';
import { SearchOutline as SearchIcon, CloseCircleOutline as ClearIcon, PlayForwardOutline as ReprocessIcon, CheckmarkCircleOutline as MarkDoneIcon, CreateOutline as EditIcon, TrashOutline as TrashIcon } from '@vicons/ionicons5';

const props = defineProps({
  taskStatus: {
    type: Object,
    required: true,
    default: () => ({ is_running: false })
  }
});

const router = useRouter();
const message = useMessage();

const tableData = ref([]);
const loading = ref(true);
const error = ref(null);
const totalItems = ref(0);
const currentPage = ref(1);
const itemsPerPage = ref(15);
const resultsContainer = ref(null);
const compactList = ref(false);
let listObserver;
const searchQuery = ref('');
const loadingAction = ref({});
const currentRowId = ref(null);
const isShowingSearchResults = ref(false);
const searchSort = ref('episode_asc');
const searchSortOptions = [
  { label: '季 / 集数：从小到大', value: 'episode_asc' },
  { label: '季 / 集数：从大到小', value: 'episode_desc' },
  { label: 'ID：从小到大', value: 'id_asc' },
  { label: 'ID：从大到小', value: 'id_desc' }
];
const displayedTableData = computed(() => isShowingSearchResults.value
  ? sortReviewSearchResults(tableData.value, searchSort.value)
  : tableData.value);
const cardRows = computed(() => isShowingSearchResults.value
  ? displayedTableData.value.slice((currentPage.value - 1) * itemsPerPage.value, currentPage.value * itemsPerPage.value)
  : displayedTableData.value);

const selectedMediaType = ref(null);
const mediaTypeOptions = [
  { label: '电视剧', value: 'Series' },
  { label: '电影', value: 'Movie' }
];
const handleMediaTypeChange = () => {
  currentPage.value = 1;
  if (isShowingSearchResults.value) searchEmbyLibrary();
  else fetchReviewItems();
};

const formatDate = (timestamp) => {
  if (!timestamp) return 'N/A';
  try {
    const date = new Date(timestamp);
    if (isNaN(date.getTime())) return '无效日期';
    return date.toLocaleString('zh-CN', {
      year: 'numeric', month: '2-digit', day: '2-digit',
      hour: '2-digit', minute: '2-digit', second: '2-digit',
      hour12: false
    });
  } catch (e) {
    return '日期格式化错误';
  }
};

const clearAllReviewItems = async () => {
  loading.value = true;
  try {
    const response = await axios.post('/api/actions/clear_review_items');
    message.success(response.data.message);
    await fetchReviewItems();
  } catch (err) {
    console.error("清空待复核列表失败:", err);
    message.error(`操作失败: ${err.response?.data?.error || err.message}`);
  } finally {
    loading.value = false;
  }
};

const goToEditPage = (row) => {
  if (row && row.item_id) {
    router.push({ name: 'MediaEditPage', params: { itemId: row.item_id } });
  } else {
    message.error("无效的媒体项，无法跳转到编辑页面！");
  }
};

const handleMarkAsProcessed = async (row) => {
  currentRowId.value = row.item_id;
  loadingAction.value[row.item_id] = true;
  try {
    await axios.post(`/api/actions/mark_item_processed/${row.item_id}`);
    message.success(`项目 "${row.item_name}" 已标记为已处理。`);
    await fetchReviewItems();
  } catch (err) {
    console.error("标记为已处理失败:", err);
    message.error(`标记项目 "${row.item_name}" 为已处理失败: ${err.response?.data?.error || err.message}`);
  } finally {
    loadingAction.value[row.item_id] = false;
    currentRowId.value = null;
  }
};

const ReviewActions = ({ row }) => {
      const actionButtons = [];

      actionButtons.push(
        h(NPopconfirm, { onPositiveClick: () => handleReprocessItem(row) }, {
            trigger: () => h(NButton, {
                size: 'small',
                type: 'warning',
                ghost: true,
                loading: loadingAction.value[row.item_id] && currentRowId.value === row.item_id,
                disabled: loadingAction.value[row.item_id] || props.taskStatus?.is_running
            }, {
                icon: () => h(NIcon, { component: ReprocessIcon }),
                default: () => '重新处理'
            }),
            default: () => `确定要重新处理 "${row.item_name}" 吗？`
        })
      );

      actionButtons.push(
        h(NButton, {
          size: 'small',
          type: 'primary',
          onClick: () => goToEditPage(row)
        }, { icon: () => h(NIcon, { component: EditIcon }), default: () => '手动编辑' })
      );

      if (!isShowingSearchResults.value) {
        actionButtons.push(
          h(NPopconfirm, { onPositiveClick: () => handleMarkAsProcessed(row) }, {
            trigger: () => h(NButton, {
              size: 'small',
              type: 'success',
              ghost: true,
              'aria-label': '标记为已处理',
              title: '标记为已处理',
              loading: loadingAction.value[row.item_id] && currentRowId.value === row.item_id,
              disabled: loadingAction.value[row.item_id] || props.taskStatus?.is_running
            }, {
              icon: () => h(NIcon, { component: MarkDoneIcon }),
              default: () => compactList.value ? '已处理' : null
            }),
            default: () => `确定要将 "${row.item_name}" 标记为已处理吗？`
          })
        );
      }

      return h(NSpace, { justify: compactList.value ? 'start' : 'center', size: 8 }, { default: () => actionButtons });
};

const columns = computed(() => [
  ...(isShowingSearchResults.value ? [{
    title: '封面',
    key: 'cover',
    width: 88,
    render(row) {
      const src = row.image_id && row.image_tag
        ? `/image_proxy/Items/${encodeURIComponent(row.image_id)}/Images/Primary?tag=${encodeURIComponent(row.image_tag)}&maxWidth=160&quality=85`
        : '';
      return h('button', {
        class: 'review-result-cover',
        type: 'button',
        'aria-label': `编辑 ${row.item_name || '媒体'}`,
        onClick: () => goToEditPage(row)
      }, [
        h('span', '暂无封面'),
        src ? h('img', {
          src,
          alt: row.item_name || '媒体封面',
          loading: 'lazy',
          onError: event => { event.target.style.display = 'none'; }
        }) : null
      ]);
    }
  }] : []),
  {
    title: '媒体名称 (ID)',
    key: 'item_name',
    resizable: true,
    render(row) {
      return h('div', {
        style: 'cursor: pointer; padding: 5px 0;',
        onClick: () => goToEditPage(row)
      }, [
        h(NText, { strong: true }, { default: () => row.item_name || '未知名称' }),
        h(NText, { depth: 3, style: 'font-size: 0.8em; display: block; margin-top: 2px;' }, {
          default: () => [
            row.series_name !== row.item_name ? row.series_name : '',
            reviewEpisodeLabel(row), row.production_year, `ID: ${row.item_id || 'N/A'}`
          ].filter(Boolean).join(' · ')
        })
      ]);
    }
  },
  ...(isShowingSearchResults.value ? [{
    title: '媒体库',
    key: 'library_name',
    width: 160,
    resizable: true,
    ellipsis: { tooltip: true },
    render: row => row.library_name || '未知媒体库'
  }] : [
  {
    title: '类型',
    key: 'item_type',
    width: 80,
    resizable: true,
    render(row) {
      const typeMap = { 'Movie': '电影', 'Series': '电视剧', 'Episode': '剧集' };
      return typeMap[row.item_type] || row.item_type;
    }
  },
  {
    title: '记录时间',
    key: 'failed_at',
    width: 170,
    resizable: true,
    render(row) { return isShowingSearchResults.value ? 'N/A' : formatDate(row.failed_at); }
  },
  { title: '原因', key: 'reason', resizable: true, ellipsis: { tooltip: true } },
  ]),
  ...(isShowingSearchResults.value ? [] : [{
    title: '评分',
    key: 'score',
    width: 80,
    resizable: true,
    render(row) {
      return row.score !== null && row.score !== undefined ? row.score.toFixed(1) : 'N/A';
    }
  }]),
  {
    title: '操作',
    key: 'actions',
    width: 280,
    align: 'center',
    fixed: 'right',
    render: row => h(ReviewActions, { row })
  }
]);

const paginationProps = computed(() => ({
    page: currentPage.value,
    pageSize: itemsPerPage.value,
    itemCount: totalItems.value,
    showSizePicker: true,
    pageSizes: [10, 15, 20, 30, 50, 100],
    onChange: (page) => {
        currentPage.value = page;
        if (!isShowingSearchResults.value) {
            fetchReviewItems();
        }
    },
    onUpdatePageSize: (pageSize) => {
        itemsPerPage.value = pageSize;
        currentPage.value = 1;
        if (!isShowingSearchResults.value) {
            fetchReviewItems();
        }
    }
}));

const fetchReviewItems = async () => {
  loading.value = true;
  error.value = null;
  isShowingSearchResults.value = false;
  const mediaType = selectedMediaType.value || '';
  try {
    const response = await axios.get(`/api/review_items`, {
        params: {
            page: currentPage.value,
            per_page: itemsPerPage.value,
            media_type: mediaType
        }
    });
    if ((selectedMediaType.value || '') !== mediaType) return;
    tableData.value = response.data.items;
    totalItems.value = response.data.total_items;
  } catch (err) {
    handleFetchError(err, "加载待处理列表失败。");
  } finally {
    loading.value = false;
  }
};

const searchEmbyLibrary = async () => {
  const query = searchQuery.value.trim();
  loading.value = true;
  error.value = null;
  isShowingSearchResults.value = true;
  const mediaType = selectedMediaType.value || '';
  try {
    const response = await axios.get(`/api/search_emby_library`, {
        params: { query, media_type: mediaType }
    });
    if (searchQuery.value.trim() !== query || (selectedMediaType.value || '') !== mediaType) return;
    tableData.value = response.data.items.map(item => ({
        ...item,
        failed_at: null,
        reason: 'N/A',
    }));
    totalItems.value = response.data.total_items;
  } catch (err) {
    handleFetchError(err, "搜索 Emby 媒体库失败。");
  } finally {
    loading.value = false;
  }
};

const handleReprocessItem = async (row) => {
  currentRowId.value = row.item_id;
  loadingAction.value[row.item_id] = true;
  try {
    const response = await axios.post(`/api/actions/reprocess_item/${row.item_id}`, {
        reason: row.reason
    });
    message.success(response.data.message || `项目 "${row.item_name}" 的重新处理任务已提交。`);
    if (!isShowingSearchResults.value) {
        await fetchReviewItems();
    }
  } catch (err) {
    console.error("重新处理失败:", err);
    message.error(`操作失败: ${err.response?.data?.error || err.message}`);
  } finally {
    loadingAction.value[row.item_id] = false;
    currentRowId.value = null;
  }
};

const reprocessAllReviewItems = async () => {
  try {
    // 与当前列表使用同一媒体类型筛选条件
    const response = await axios.post('/api/actions/reprocess_all_review_items', {
      media_type: selectedMediaType.value || ''
    });
    message.success(response.data.message || '重新处理任务已成功提交！');
  } catch (err) {
    console.error("提交重新处理任务失败:", err);
    message.error(`操作失败: ${err.response?.data?.error || err.message}`);
  }
};

const handleFetchError = (err, defaultMessage) => {
    console.error(defaultMessage, err);
    error.value = defaultMessage + (err.response?.data?.error || err.message);
    message.error(error.value);
};

const handleSearchQueryChange = (value) => {
  if (!value.trim() && isShowingSearchResults.value) {
    tableData.value = [];
    totalItems.value = 0;
    currentPage.value = 1;
    fetchReviewItems();
  }
};

const clearSearch = () => {
  searchQuery.value = '';
  handleSearchQueryChange('');
};

const handleSearch = () => {
  if (searchQuery.value.trim()) {
    currentPage.value = 1;
    searchEmbyLibrary();
  } else {
    currentPage.value = 1;
    fetchReviewItems();
  }
};

watch(resultsContainer, element => {
  listObserver?.disconnect();
  if (!element) return;
  compactList.value = element.clientWidth < 760;
  listObserver = new ResizeObserver(([entry]) => { compactList.value = entry.contentRect.width < 760; });
  listObserver.observe(element);
});
onMounted(fetchReviewItems);
onUnmounted(() => listObserver?.disconnect());
</script>

<style scoped>
:deep(.review-result-cover) {
  position: relative;
  display: grid;
  place-items: center;
  width: 56px;
  height: 84px;
  padding: 0;
  overflow: hidden;
  border: 1px solid var(--n-border-color);
  border-radius: 4px;
  color: var(--n-text-color-3);
  background: var(--n-color);
  font-size: 11px;
  cursor: pointer;
}

:deep(.review-result-cover img) {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.review-search-field {
  position: relative;
  flex:1 1 280px;
  min-width:0;
  height:36px;
}

.review-search-input {
  width: 100%;
  height: 100%;
  box-sizing: border-box;
  padding: 0 66px 0 10px;
  color:var(--ui-text);
  background-color:var(--ui-input-bg);
  border:1px solid var(--ui-input-border);
  border-radius:8px;
  outline: none;
  font: inherit;
  line-height: 1.5;
  caret-color:var(--accent-color);
  transition: border-color 0.3s, box-shadow 0.3s;
}

.review-search-input:hover,
.review-search-input:focus {
  border-color:var(--accent-color);
}

.review-search-input:focus {
  box-shadow:0 0 0 3px var(--accent-glow-color);
}

.review-search-input::placeholder {
  color:var(--ui-text-muted);
}

.review-search-action {
  position: absolute;
  top:4px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  padding: 0;
  color:var(--ui-text-muted);
  background: transparent;
  border: 0;
  border-radius: 3px;
  cursor: pointer;
}

.review-search-action:hover {
  color:var(--accent-color);
  background-color:var(--accent-glow-color);
}

.review-search-clear {
  right: 31px;
}

.review-search-submit {
  right: 3px;
}

.review-toolbar { display:flex; flex-wrap:wrap; align-items:center; gap:12px; margin-bottom:24px; }
.review-filter { width:180px; }
.review-results { min-width:0; }
.review-cards { display:grid; gap:12px; }
.review-item-card { min-width:0; padding:16px; border:1px solid var(--ui-border); border-radius:12px; background:var(--ui-surface); }
.review-item-title { display:block; width:100%; padding:0; border:0; color:var(--ui-text); background:transparent; text-align:left; font:inherit; font-weight:600; line-height:1.5; overflow-wrap:anywhere; cursor:pointer; }
.review-item-title:hover { color:var(--accent-color); }
.review-item-title:focus-visible { outline:2px solid var(--accent-color); outline-offset:4px; border-radius:2px; }
.review-item-meta { margin:6px 0; color:var(--ui-text-muted); font-size:12px; overflow-wrap:anywhere; }
.review-item-reason { margin:12px 0; color:var(--ui-text); overflow-wrap:anywhere; }
.review-item-card :deep(.n-space) { margin-top:12px; }
.review-cards > .n-pagination { justify-self:end; }
@media (max-width:540px) {
  .review-search-field { flex-basis:100%; }
  .review-filter { flex:1 1 150px; min-width:0; }
  .review-toolbar > .n-button { flex:1; }
}
</style>
