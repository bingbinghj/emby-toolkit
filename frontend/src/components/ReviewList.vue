<!-- Media presentation follows MoviePilot v3 cards and Vuetify tables; actions use ETK APIs. -->
<template>
  <div class="mp-page review-page">
    <VCard class="dashboard-card">
      <VCardText>
        <VAlert v-if="props.taskStatus?.is_running" type="warning" variant="tonal" title="后台任务运行中" class="mb-4">
          请等待当前任务结束后再提交处理操作。
        </VAlert>
        <div class="review-toolbar">
          <VTextField v-model="searchQuery" label="搜索媒体" aria-label="搜索媒体" placeholder="输入媒体名称搜索整个 Emby 库..."
            prepend-inner-icon="mdi-magnify" clearable hide-details density="compact" variant="solo" flat rounded="pill" class="review-search-field"
            autocomplete="off" @keyup.enter="handleSearch" @update:model-value="handleSearchQueryChange" @click:clear="clearSearch" @compositionstart="searchComposing = true" @compositionend="finishSearchComposition">
            <template #append-inner><VBtn icon="mdi-magnify" variant="text" size="small" aria-label="搜索" title="搜索" @click.stop="handleSearch" /></template>
          </VTextField>
          <VMenu v-if="isShowingSearchResults" location="bottom end">
            <template #activator="{ props }"><VBtn v-bind="props" icon="mdi-sort" variant="text" aria-label="搜索结果排序" title="搜索结果排序" /></template>
            <VList min-width="200" class="px-2"><VListSubheader>搜索结果排序</VListSubheader><VListItem v-for="option in searchSortOptions" :key="option.value" :title="option.label" :active="searchSort === option.value" @click="searchSort = option.value; currentPage = 1"><template #append><VIcon v-if="searchSort === option.value" icon="mdi-check" color="primary" /></template></VListItem></VList>
          </VMenu>
          <VMenu location="bottom end">
            <template #activator="{ props }"><VBtn v-bind="props" icon="mdi-filter-multiple-outline" :variant="selectedMediaType ? 'tonal' : 'text'" :color="selectedMediaType ? 'primary' : 'default'" aria-label="按媒体类型筛选" title="按媒体类型筛选" /></template>
            <VList min-width="200" class="px-2"><VListSubheader>媒体类型</VListSubheader><VListItem v-for="option in mediaTypeOptions" :key="option.value || 'all'" :title="option.label" :active="selectedMediaType === option.value" @click="selectedMediaType = option.value; handleMediaTypeChange()"><template #append><VIcon v-if="selectedMediaType === option.value" icon="mdi-check" color="primary" /></template></VListItem></VList>
          </VMenu>
          <VBtnToggle v-if="!compactList" v-model="displayMode" mandatory divided variant="outlined" density="compact" aria-label="显示方式">
            <VBtn value="table" icon="mdi-format-list-bulleted" title="表格视图" aria-label="表格视图" />
            <VBtn value="cards" icon="mdi-view-grid-outline" title="卡片视图" aria-label="卡片视图" />
          </VBtnToggle>
          <VBtn color="error" variant="tonal" prepend-icon="mdi-delete-outline"
            :disabled="tableData.length === 0 || loading || isShowingSearchResults"
            @click="confirmReviewAction('清空待复核', '确定要清空全部待复核记录吗？此操作不可恢复。', clearAllReviewItems)">清空待复核</VBtn>
          <VBtn color="warning" variant="tonal" prepend-icon="mdi-refresh" :loading="reprocessingAll"
            :disabled="tableData.length === 0 || loading || props.taskStatus?.is_running || isShowingSearchResults"
            @click="confirmReviewAction('批量重新处理', `确定要重新处理 ${selectedMediaType ? '当前筛选出的' : '所有'} ${totalItems} 条记录吗？`, runReprocessAll)">
            {{ selectedMediaType ? `重新处理 ${totalItems} 项` : '重新处理所有' }}
          </VBtn>
        </div>
        <VAlert v-if="reprocessFeedback" :type="reprocessFeedback.type" variant="tonal" :title="reprocessFeedback.message"
          class="mb-4" closable @click:close="reprocessFeedback = null">
          <div v-for="detail in reprocessFeedback.details" :key="detail">{{ detail }}</div>
        </VAlert>
        <VProgressLinear v-if="loading" indeterminate class="mb-4" aria-label="正在加载媒体" />
        <VAlert v-if="error" title="加载错误" type="error" variant="tonal">{{ error }}</VAlert>
        <div v-else ref="resultsContainer" class="review-results">
          <div v-if="tableData.length > 0 && useCardView">
            <div class="review-cards">
              <VCard v-for="row in cardRows" :key="row.item_id" class="review-item-card app-hover-lift-card" variant="outlined">
                <div class="review-card-body">
                  <VCard tag="button" type="button" class="review-card-cover" :aria-label="`编辑 ${row.item_name || '媒体'}`" @click="goToEditPage(row)">
                    <VImg :src="reviewImage(row) || undefined" :aspect-ratio="2 / 3" cover>
                      <template #error><div class="review-poster-fallback"><VIcon icon="mdi-movie-open-outline" size="32" /></div></template>
                    </VImg>
                  </VCard>
                  <div class="review-card-detail">
                    <VBtn variant="text" color="default" class="review-item-title text-none pa-0" @click="goToEditPage(row)">{{ row.item_name || '未知名称' }}</VBtn>
                    <p class="review-item-meta">{{ [row.series_name !== row.item_name ? row.series_name : '', reviewEpisodeLabel(row), row.production_year, `ID: ${row.item_id}`].filter(Boolean).join(' · ') }}</p>
                    <VChip v-if="row.item_type" size="x-small" color="info" variant="tonal">{{ mediaTypeLabels[row.item_type] || row.item_type }}</VChip>
                    <p v-if="isShowingSearchResults" class="review-item-meta">{{ row.library_name || '未知媒体库' }}</p>
                    <template v-else>
                      <p class="review-item-reason">{{ row.reason || '未记录原因' }}</p>
                      <p class="review-item-meta">记录时间：{{ formatDate(row.failed_at) }}</p>
                    </template>
                    <ReviewActions :row="row" />
                  </div>
                </div>
              </VCard>
            </div>
            <div class="review-pagination mt-5">
              <VSelect :model-value="itemsPerPage" :items="paginationProps.pageSizes" label="每页数量" hide-details density="compact"
                class="review-page-size" @update:model-value="paginationProps.onUpdatePageSize" />
              <VPagination :model-value="currentPage" :length="Math.max(Math.ceil(totalItems / itemsPerPage), 1)"
                :total-visible="compactList ? 5 : 7" density="comfortable" @update:model-value="paginationProps.onChange" />
            </div>
          </div>
          <component :is="isShowingSearchResults ? VDataTable : VDataTableServer" v-else-if="tableData.length > 0"
            :headers="columns" :items="displayedTableData" item-value="item_id" :page="currentPage" :items-per-page="itemsPerPage"
            :items-per-page-options="paginationProps.pageSizes" :loading="loading || Boolean(loadingAction[currentRowId])"
            v-bind="isShowingSearchResults ? {} : { 'items-length': totalItems }" density="comfortable" hover
            @update:page="paginationProps.onChange" @update:items-per-page="paginationProps.onUpdatePageSize">
            <template #item.cover="{ item }">
              <VCard tag="button" type="button" class="review-result-cover" :aria-label="`编辑 ${item.item_name || '媒体'}`" @click="goToEditPage(item)">
                <VImg :src="reviewImage(item) || undefined" :aspect-ratio="2 / 3" cover>
                  <template #error><div class="review-poster-fallback"><VIcon icon="mdi-movie-open-outline" size="24" /></div></template>
                </VImg>
              </VCard>
            </template>
            <template #item.item_name="{ item }">
              <VBtn color="default" variant="text" class="review-item-title text-none px-0" @click="goToEditPage(item)">
                <span>{{ item.item_name || '未知名称' }}</span>
              </VBtn>
              <div class="review-item-meta">{{ [item.series_name !== item.item_name ? item.series_name : '', reviewEpisodeLabel(item), item.production_year, `ID: ${item.item_id || 'N/A'}`].filter(Boolean).join(' · ') }}</div>
            </template>
            <template #item.item_type="{ item }"><VChip size="small" color="info" variant="tonal">{{ mediaTypeLabels[item.item_type] || item.item_type }}</VChip></template>
            <template #item.library_name="{ item }">{{ item.library_name || '未知媒体库' }}</template>
            <template #item.failed_at="{ item }">{{ formatDate(item.failed_at) }}</template>
            <template #item.reason="{ item }"><span class="review-table-reason" :title="item.reason">{{ item.reason || '未记录原因' }}</span></template>
            <template #item.actions="{ item }"><ReviewActions :row="item" /></template>
          </component>
          <div v-else-if="!loading" class="text-center py-10">
            <VIcon :icon="isShowingSearchResults ? 'mdi-movie-search-outline' : 'mdi-check-circle-outline'" size="54" color="secondary" />
            <p class="text-body-1 text-medium-emphasis mt-4">{{ isShowingSearchResults ? '在 Emby 库中未找到匹配项。' : '没有需要手动处理的媒体项。' }}</p>
          </div>
        </div>
      </VCardText>
    </VCard>
  </div>
</template>

<script setup>
import { useRouter, useRoute } from 'vue-router';
import { useContextSearch } from '@/composables/useContextSearch';
import { ref, onMounted, onUnmounted, computed, watch, h } from 'vue';
import axios from 'axios';
import { sortReviewSearchResults, reviewEpisodeLabel } from '../utils/reviewSearch';
import { VBtn, VDataTable, VDataTableServer } from 'vuetify/components';
import { useMessage, useDialog } from '@/composables/useFeedback';

const props = defineProps({
  taskStatus: {
    type: Object,
    required: true,
    default: () => ({ is_running: false })
  }
});

const router = useRouter(), route = useRoute();
const message = useMessage();
const dialog = useDialog();

const tableData = ref([]);
const loading = ref(true);
const error = ref(null);
const reprocessFeedback = ref(null);
const totalItems = ref(0);
const currentPage = ref(1);
const itemsPerPage = ref(15);
const resultsContainer = ref(null);
const compactList = ref(false);
const displayMode = ref('table');
const useCardView = computed(() => displayMode.value === 'cards' || compactList.value);
const reprocessingAll = ref(false);
let listObserver, searchTimer, listSequence = 0, pageActive = true;
const searchComposing = ref(false);
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
  { label: '全部媒体类型', value: null },
  { label: '电视剧', value: 'Series' },
  { label: '电影', value: 'Movie' }
];
const handleMediaTypeChange = () => {
  clearTimeout(searchTimer);
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

const confirmReviewAction = (title, content, action) => dialog.warning({
  title,
  content,
  positiveText: '确认',
  negativeText: '取消',
  onPositiveClick: action,
});

const runReprocessAll = async () => {
  if (reprocessingAll.value) return;
  reprocessingAll.value = true;
  try { await reprocessAllReviewItems(); }
  finally { reprocessingAll.value = false; }
};

const ReviewActions = ({ row }) => {
  const busy = Boolean(loadingAction.value[row.item_id]);
  const actions = [
    h(VBtn, {
      size: 'small', color: 'warning', variant: 'tonal', prependIcon: 'mdi-refresh',
      loading: busy && currentRowId.value === row.item_id,
      disabled: busy || props.taskStatus?.is_running,
      onClick: () => confirmReviewAction('重新处理媒体', `确定要重新处理 "${row.item_name}" 吗？`, () => handleReprocessItem(row)),
    }, () => '重新处理'),
    h(VBtn, {
      size: 'small', color: 'primary', variant: 'flat', prependIcon: 'mdi-pencil-outline',
      onClick: () => goToEditPage(row),
    }, () => '手动编辑'),
  ];
  if (!isShowingSearchResults.value) {
    actions.push(h(VBtn, {
      size: 'small', color: 'success', variant: 'tonal', prependIcon: 'mdi-check-circle-outline',
      'aria-label': '标记为已处理', title: '标记为已处理',
      loading: busy && currentRowId.value === row.item_id,
      disabled: busy || props.taskStatus?.is_running,
      onClick: () => confirmReviewAction('标记为已处理', `确定要将 "${row.item_name}" 标记为已处理吗？`, () => handleMarkAsProcessed(row)),
    }, () => '已处理'));
  }
  return h('div', { class: ['review-actions', useCardView.value ? '' : 'justify-center'] }, actions);
};

const mediaTypeLabels = { Movie: '电影', Series: '电视剧', Episode: '剧集' };
const reviewImage = row => {
  const id = row.image_id || row.item_id;
  if (!id) return '';
  const tag = row.image_tag ? `&tag=${encodeURIComponent(row.image_tag)}` : '';
  return `/image_proxy/Items/${encodeURIComponent(id)}/Images/Primary?maxWidth=240&quality=85${tag}`;
};

const columns = computed(() => [
  ...(isShowingSearchResults.value ? [{ title: '封面', key: 'cover', width: 84, sortable: false }] : []),
  { title: '媒体名称 (ID)', key: 'item_name', minWidth: 220, sortable: false },
  ...(isShowingSearchResults.value ? [
    { title: '媒体库', key: 'library_name', minWidth: 140, sortable: false },
  ] : [
    { title: '类型', key: 'item_type', width: 100, sortable: false },
    { title: '记录时间', key: 'failed_at', minWidth: 170, sortable: false },
    { title: '原因', key: 'reason', minWidth: 220, sortable: false },
  ]),
  { title: '操作', key: 'actions', minWidth: 340, align: 'center', sortable: false },
]);

const paginationProps = computed(() => ({
    page: currentPage.value,
    pageSize: itemsPerPage.value,
    itemCount: totalItems.value,
    showSizePicker: true,
    pageSizes: [10, 15, 20, 30, 50, 100],
    onChange: (page) => {
        if (page === currentPage.value) return;
        currentPage.value = page;
        if (!isShowingSearchResults.value) {
            fetchReviewItems();
        }
    },
    onUpdatePageSize: (pageSize) => {
        if (pageSize === itemsPerPage.value) return;
        itemsPerPage.value = pageSize;
        currentPage.value = 1;
        if (!isShowingSearchResults.value) {
            fetchReviewItems();
        }
    }
}));

const fetchReviewItems = async () => {
  const sequence = ++listSequence, requestedPage = currentPage.value;
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
    if (!pageActive || sequence !== listSequence || currentPage.value !== requestedPage || (selectedMediaType.value || '') !== mediaType) return;
    tableData.value = response.data.items;
    totalItems.value = response.data.total_items;
  } catch (err) {
    if (pageActive && sequence === listSequence) handleFetchError(err, "加载待处理列表失败。");
  } finally {
    if (pageActive && sequence === listSequence) loading.value = false;
  }
};

const searchEmbyLibrary = async () => {
  const sequence = ++listSequence;
  const query = searchQuery.value.trim();
  loading.value = true;
  error.value = null;
  isShowingSearchResults.value = true;
  const mediaType = selectedMediaType.value || '';
  try {
    const response = await axios.get(`/api/search_emby_library`, {
        params: { query, media_type: mediaType }
    });
    if (!pageActive || sequence !== listSequence || searchQuery.value.trim() !== query || (selectedMediaType.value || '') !== mediaType) return;
    tableData.value = response.data.items.map(item => ({
        ...item,
        failed_at: null,
        reason: 'N/A',
    }));
    totalItems.value = response.data.total_items;
  } catch (err) {
    if (pageActive && sequence === listSequence) handleFetchError(err, "搜索 Emby 媒体库失败。");
  } finally {
    if (pageActive && sequence === listSequence) loading.value = false;
  }
};

const showReprocessResult = (response, fallback, failed = false) => {
  const result = response?.data || {};
  const details = [...new Set([
    ...(Array.isArray(result.failures) ? result.failures : []),
    ...(Array.isArray(result.warning_details) ? result.warning_details : []),
  ].map(detail => typeof detail === 'string' ? detail : [detail?.item_id, detail?.reason || detail?.error].filter(Boolean).join('：')).filter(Boolean))];
  const summary = result.error || result.message || fallback;
  const hasFailures = failed || Number(result.failed) > 0 || (Array.isArray(result.failures) && result.failures.length > 0);
  const type = hasFailures ? 'error' : response?.status === 207 || Number(result.warnings) > 0 || details.length ? 'warning' : 'success';
  reprocessFeedback.value = type === 'success' ? null : { type, message: summary, details };
  message[type]([summary, ...details].join('；'), { duration: type === 'success' ? 3000 : 8000 });
};

const handleReprocessItem = async (row) => {
  currentRowId.value = row.item_id;
  loadingAction.value[row.item_id] = true;
  try {
    const response = await axios.post(`/api/actions/reprocess_item/${row.item_id}`, {
        reason: row.reason
    });
    showReprocessResult(response, `项目 "${row.item_name}" 的重新处理任务已提交。`);
    if (!isShowingSearchResults.value) {
        await fetchReviewItems();
    }
  } catch (err) {
    console.error("重新处理失败:", err);
    showReprocessResult(err.response || { data: { error: err.message } }, '重新处理失败', true);
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
    showReprocessResult(response, '重新处理任务已成功提交！');
  } catch (err) {
    console.error("提交重新处理任务失败:", err);
    showReprocessResult(err.response || { data: { error: err.message } }, '批量重新处理失败', true);
  }
};

const handleFetchError = (err, defaultMessage) => {
    console.error(defaultMessage, err);
    error.value = defaultMessage + (err.response?.data?.error || err.message);
    message.error(error.value);
};

const handleSearchQueryChange = (value) => {
  clearTimeout(searchTimer);
  searchQuery.value = String(value ?? '');
  if (!searchQuery.value.trim() && isShowingSearchResults.value) {
    tableData.value = [];
    totalItems.value = 0;
    currentPage.value = 1;
    fetchReviewItems();
  } else if (searchQuery.value.trim() && !searchComposing.value) {
    searchTimer = setTimeout(handleSearch, compactList.value ? 600 : 1000);
  }
};

const finishSearchComposition = () => { searchComposing.value = false; handleSearchQueryChange(searchQuery.value); };

const clearSearch = () => {
  searchQuery.value = '';
  handleSearchQueryChange('');
};

const handleSearch = () => {
  clearTimeout(searchTimer);
  if (searchComposing.value) return;
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
useContextSearch({ label: '搜索全部媒体', query: searchQuery, submit: handleSearch, onInput: handleSearchQueryChange, clear: clearSearch, composing: searchComposing, compositionEnd: finishSearchComposition, loading });
watch(() => route.query.q, value => {
  const query = typeof value === 'string' ? value : '';
  if (query !== searchQuery.value) { searchQuery.value = query; handleSearch(); }
});
onMounted(() => {
  const query = typeof route.query.q === 'string' ? route.query.q : '';
  if (query.trim()) { searchQuery.value = query; handleSearch(); } else fetchReviewItems();
});
onUnmounted(() => { pageActive = false; ++listSequence; clearTimeout(searchTimer); listObserver?.disconnect(); });
</script>

<style scoped>
.review-toolbar { display:flex; flex-wrap:wrap; align-items:center; gap:12px; margin-bottom:24px; }
.review-search-field { flex:1 1 300px; min-width:0; }
.review-results { min-width:0; }
.review-cards { display:grid; grid-template-columns:repeat(auto-fill,minmax(min(100%,440px),1fr)); gap:16px; }
.review-item-card { min-width:0; }
.review-card-body { display:flex; gap:16px; padding:16px; }
.review-card-cover { flex:0 0 96px; width:96px; align-self:flex-start; padding:0; cursor:pointer; }
.review-card-detail { flex:1; min-width:0; }
.review-item-title { max-width:100%; height:auto; min-height:24px; padding-block:4px !important; border-radius:0; justify-content:flex-start; font-weight:600; text-align:left; }
.review-item-title :deep(.v-btn__content) { white-space:normal; text-align:left; overflow-wrap:anywhere; }
.review-item-meta { margin:6px 0; color:rgba(var(--v-theme-on-surface),var(--v-medium-emphasis-opacity)); font-size:12px; overflow-wrap:anywhere; }
.review-item-reason { margin:12px 0; overflow-wrap:anywhere; }
:deep(.review-actions) { display:flex; flex-wrap:wrap; align-items:center; gap:8px; margin-top:12px; }
.review-results :deep(.v-data-table .review-actions) { margin:8px 0; }
.review-result-cover { width:56px; padding:0; margin:8px 0; cursor:pointer; }
.review-poster-fallback { display:flex; justify-content:center; align-items:center; width:100%; height:100%; color:rgba(var(--v-theme-on-surface),var(--v-medium-emphasis-opacity)); background:rgba(var(--v-theme-on-surface),.06); }
.review-table-reason { display:block; max-width:24rem; white-space:pre-wrap; overflow-wrap:anywhere; line-height:1.5; }
.review-page :deep(.v-alert-title) { white-space:normal; overflow-wrap:anywhere; }
.review-page :deep(.v-data-table__td) { padding-block:12px; }
.review-page :deep(.v-list-item-title) { white-space:normal; overflow-wrap:anywhere; }
.review-pagination { display:flex; align-items:center; justify-content:flex-end; flex-wrap:wrap; gap:16px; }
.review-page-size { flex:0 0 120px; }
@media (max-width:600px) { .review-search-field { flex-basis:100%; } .review-toolbar > .v-btn:not(.v-btn--icon) { flex:1 1 auto; } .review-card-body { gap:12px; padding:12px; } .review-card-cover { flex-basis:72px; width:72px; } .review-pagination { justify-content:center; } }
</style>
