<template>
  <div class="mp-page media-organize-page">
    <VCard class="mb-4">
      <VCardText>
        <div class="organize-toolbar">
          <VTextField v-model="search" label="搜索入库记录" aria-label="搜索入库记录" placeholder="按名称查找" prepend-inner-icon="mdi-magnify" clearable variant="solo" flat rounded="pill" hide-details density="compact" class="organize-search" :disabled="processing" @keyup.enter="searchItems" @click:clear="clearSearch" @compositionstart="searchComposing = true" @compositionend="finishSearchComposition">
            <template #append-inner><VBtn icon="mdi-magnify" variant="text" size="small" aria-label="搜索" :loading="loadingItems" :disabled="processing" @click.stop="searchItems" /></template>
          </VTextField>
          <VMenu location="bottom end">
            <template #activator="{ props }"><VBtn v-bind="props" icon="mdi-filter-multiple-outline" :variant="statusFilter === 'all' ? 'text' : 'tonal'" :color="statusFilter === 'all' ? 'default' : 'primary'" aria-label="处理状态筛选" title="处理状态筛选" :disabled="processing" data-testid="organize-status-filter" /></template>
            <VList min-width="200" class="px-2">
              <VListSubheader>处理状态</VListSubheader>
              <VListItem v-for="option in statusOptions" :key="option.value" :title="option.title" :active="statusFilter === option.value" @click="statusFilter = option.value">
                <template #append><VIcon v-if="statusFilter === option.value" icon="mdi-check" color="primary" /></template>
              </VListItem>
            </VList>
          </VMenu>
          <VBtn icon="mdi-refresh" variant="text" color="default" aria-label="刷新入库记录" :loading="loadingItems" :disabled="processing" @click="loadItems" />
          <VBtn color="primary" variant="tonal" prepend-icon="mdi-refresh" :disabled="!canReorganize" :loading="processing" data-testid="organize-selected" @click="openConfirmation">重新整理 {{ selectedMediaCount || '' }}</VBtn>
        </div>
      </VCardText>
    </VCard>

    <VAlert v-if="selectedItems.length" type="info" variant="tonal" class="mb-4" data-testid="organize-selection">
      <div class="d-flex flex-wrap align-center ga-3"><span>已选择 {{ selectedItems.length }} 条记录，对应 {{ selectedMediaCount }} 项媒体，跨分页保留选择。</span><VBtn size="small" variant="text" :disabled="processing" @click="selectedItems = []">清空选择</VBtn></div>
      <div class="d-flex flex-wrap ga-2 mt-2"><VChip v-for="item in selectedItems" :key="item.id" size="small" closable :disabled="processing" @click:close="removeSelection(item.id)">{{ itemTitle(item) }}{{ episodeLabel(item) ? ` · ${episodeLabel(item)}` : '' }} · {{ statusPresentation(item).title }}</VChip></div>
    </VAlert>

    <VAlert v-if="itemsError" type="error" variant="tonal" class="mb-4" data-testid="organize-load-error">{{ itemsError }}<VBtn variant="text" :loading="loadingItems" @click="loadItems">重新读取</VBtn></VAlert>
    <VAlert v-if="feedback" :type="feedback.type" :title="feedback.message" variant="tonal" class="mb-4" data-testid="organize-result">
      <div v-for="(detail, index) in feedback.details" :key="index" class="text-wrap">{{ detail }}</div>
    </VAlert>

    <VCard>
      <div v-if="smAndDown" class="history-cards pa-3" data-testid="organize-items-mobile">
        <VProgressLinear v-if="loadingItems" indeterminate />
        <VCard v-for="item in items" :key="item.id" variant="outlined" class="history-card">
          <VCardText class="history-card-body">
            <VCheckboxBtn :model-value="selectedIds.includes(item.id)" :disabled="!canSelectRecord(item)" :aria-label="`选择 ${itemTitle(item)}`" @update:model-value="toggleSelection(item, $event)" />
            <VAvatar rounded width="48" height="72"><VImg v-if="poster(item)" :src="poster(item)" :alt="itemTitle(item)" cover /><VIcon v-else icon="mdi-movie-open-outline" /></VAvatar>
            <div class="organize-media-copy">
              <VBtn v-if="item.item_id" variant="text" color="default" class="media-title text-none px-0" :to="`/edit-media/${encodeURIComponent(item.item_id)}`">{{ itemTitle(item) }}</VBtn>
              <div v-else class="font-weight-medium">{{ itemTitle(item) }}</div>
              <div v-if="episodeLabel(item)" class="text-caption text-medium-emphasis">{{ episodeLabel(item) }}</div>
              <div class="text-caption text-medium-emphasis mt-1">{{ formatDate(item.created_at) }}</div>
              <div class="text-caption text-medium-emphasis">{{ item.library_name || '—' }} · {{ item.item_type === 'Movie' ? '电影' : '剧集' }}</div>
              <VChip size="small" variant="tonal" :color="statusPresentation(item).color" class="mt-2">{{ statusPresentation(item).title }}</VChip>
              <div v-if="item.message" class="text-body-2 mt-2 history-message">{{ item.message }}</div>
              <div v-if="!item.item_id" class="text-caption text-medium-emphasis mt-1">尚未关联媒体，暂不能重新整理。</div>
            </div>
          </VCardText>
        </VCard>
        <div v-if="!items.length && !loadingItems" class="py-8 text-center text-medium-emphasis">{{ emptyText }}</div>
      </div>
      <VDataTable v-else :headers="headers" :items="items" item-value="id" show-select :model-value="selectedIds" :items-per-page="pageSize" :loading="loadingItems" :item-selectable="canSelectRecord" hide-default-footer data-testid="organize-items" @update:model-value="updateSelection">
        <template #item.name="{ item }">
          <div class="d-flex align-center ga-3 py-2"><VAvatar rounded width="42" height="63"><VImg v-if="poster(item)" :src="poster(item)" :alt="itemTitle(item)" cover /><VIcon v-else icon="mdi-movie-open-outline" /></VAvatar><div class="organize-media-copy"><VBtn v-if="item.item_id" variant="text" color="default" class="media-title text-none px-0" :to="`/edit-media/${encodeURIComponent(item.item_id)}`">{{ itemTitle(item) }}</VBtn><div v-else class="font-weight-medium">{{ itemTitle(item) }}</div><div v-if="episodeLabel(item)" class="text-caption text-medium-emphasis">{{ episodeLabel(item) }}</div><div v-if="!item.item_id" class="text-caption text-medium-emphasis">尚未关联媒体</div></div></div>
        </template>
        <template #item.item_type="{ item }"><VChip size="small" variant="tonal" :color="item.item_type === 'Movie' ? 'primary' : 'success'">{{ item.item_type === 'Movie' ? '电影' : '剧集' }}</VChip></template>
        <template #item.created_at="{ item }">{{ formatDate(item.created_at) }}</template>
        <template #item.status="{ item }"><VChip size="small" variant="tonal" :color="statusPresentation(item).color">{{ statusPresentation(item).title }}</VChip><div v-if="item.message" class="text-body-2 mt-1 history-message">{{ item.message }}</div></template>
        <template #no-data><div class="py-8 text-medium-emphasis">{{ emptyText }}</div></template>
      </VDataTable>
      <VCardActions class="d-flex flex-wrap justify-space-between"><span class="text-body-2 text-medium-emphasis">{{ total === null ? '总数未确认' : `共 ${total} 条记录` }}</span><VPagination v-if="pages > 1" v-model="page" :length="pages" :total-visible="5" :disabled="loadingItems || processing" @update:model-value="loadItems" /></VCardActions>
    </VCard>

    <VDialog v-model="confirmationOpen" max-width="640" :persistent="processing">
      <VCard>
        <VCardTitle class="text-wrap">确认重新整理 {{ confirmationMediaCount }} 项媒体</VCardTitle>
        <VCardText>
          <p>将重新执行媒体信息重建、回读验收和元数据增强。电影包含关联版本，剧集只处理所选单集。同一媒体的多条记录合并为一次整理。</p>
          <VList bg-color="transparent" density="compact"><VListItem v-for="item in confirmationItems" :key="item.id" :title="itemTitle(item)" :subtitle="`${episodeLabel(item) || (item.item_type === 'Movie' ? '电影' : '剧集')} · ${item.item_id}`" /></VList>
        </VCardText>
        <VCardActions><VSpacer /><VBtn variant="text" :disabled="processing" @click="confirmationOpen = false">取消</VBtn><VBtn color="primary" variant="flat" :loading="processing" data-testid="organize-confirm" @click="reorganize">开始重新整理</VBtn></VCardActions>
      </VCard>
    </VDialog>
  </div>
</template>

<script setup>
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useContextSearch } from '@/composables/useContextSearch';
import { useDisplay } from 'vuetify';
import axios from 'axios';
import { useMessage } from '@/composables/useFeedback';

const message = useMessage();
const { smAndDown } = useDisplay();
const items = ref([]), search = ref(''), appliedSearch = ref(''), statusFilter = ref('all'), page = ref(1), total = ref(null), searchComposing = ref(false);
const statusOptions = [{ title: '全部状态', value: 'all' }, { title: '成功', value: 'success' }, { title: '失败', value: 'failed' }, { title: '有提醒', value: 'warning' }, { title: '正在处理', value: 'processing' }];
const pageSize = 24;
const selectedItems = ref([]), confirmationItems = ref([]), confirmationOpen = ref(false);
const loadingItems = ref(false), itemsError = ref(''), processing = ref(false), feedback = ref(null);
let active = true, itemSequence = 0, searchTimer;
const loadedQuery = ref('');
const queryKey = () => JSON.stringify([page.value, appliedSearch.value, statusFilter.value]);
const selectedIds = computed(() => selectedItems.value.map(item => item.id));
const selectedMediaCount = computed(() => new Set(selectedItems.value.filter(item => item.item_id).map(item => item.item_id)).size);
const canReorganize = computed(() => !processing.value && selectedItems.value.length > 0 && selectedItems.value.every(isReadyRecord));
const confirmationMediaCount = computed(() => new Set(confirmationItems.value.map(item => item.item_id)).size);
const pages = computed(() => Math.max(1, Math.ceil((total.value || 0) / pageSize)));
const emptyText = computed(() => itemsError.value ? '入库处理记录尚未读取成功' : '当前条件下没有入库处理记录');
const canSelectItems = () => !processing.value && !loadingItems.value && !itemsError.value && loadedQuery.value === queryKey();
const isReadyRecord = item => ['Movie', 'Episode'].includes(item.item_type) && typeof item.item_id === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(item.item_id) && ['success', 'warning', 'failed'].includes(item.status);
const canSelectRecord = item => canSelectItems() && isReadyRecord(item);
const headers = [{ title: '媒体', key: 'name', sortable: false }, { title: '媒体库', key: 'library_name', sortable: false }, { title: '类型', key: 'item_type', sortable: false }, { title: '处理开始时间', key: 'created_at', sortable: false }, { title: '状态', key: 'status', sortable: false }];
const itemTitle = item => (item.item_type === 'Episode' ? item.series_name || item.name : item.name) || '未知媒体';
const episodeLabel = item => {
  if (item.item_type !== 'Episode') return '';
  const season = Number.isInteger(item.season_number) ? `S${String(item.season_number).padStart(2, '0')}` : '';
  const episode = Number.isInteger(item.episode_number) ? `E${String(item.episode_number).padStart(2, '0')}` : '';
  return [[season, episode].join(''), item.name !== item.series_name ? item.name : ''].filter(Boolean).join(' · ');
};
const statusPresentation = item => ({ success: { title: '成功', color: 'success' }, warning: { title: '有提醒', color: 'warning' }, failed: { title: '失败', color: 'error' }, processing: { title: '正在处理', color: 'primary' } }[item.status] || { title: '未知', color: 'secondary' });
const formatDate = value => {
  if (!value) return '—';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '—' : date.toLocaleString('zh-CN', { hour12: false });
};
const poster = item => {
  const seriesPoster = item.item_type === 'Episode' && item.series_id && item.series_image_tag;
  const id = seriesPoster ? item.series_id : item.item_id, tag = seriesPoster ? item.series_image_tag : item.image_tag;
  return id && tag ? `/image_proxy/Items/${encodeURIComponent(id)}/Images/Primary?tag=${encodeURIComponent(tag)}&quality=90` : '';
};

const loadItems = async () => {
  const sequence = ++itemSequence, targetQuery = queryKey();
  loadingItems.value = true;
  try {
    const { data } = await axios.get('/api/media_organize/history', {
      params: { page: page.value, page_size: pageSize, search: appliedSearch.value, status: statusFilter.value },
    });
    if (!active || sequence !== itemSequence || targetQuery !== queryKey()) return;
    if (!Array.isArray(data?.items) || !Number.isInteger(data?.total) || data.total < data.items.length) throw new Error('入库处理记录或分页总数无效');
    const validItems = data.items.every(item => ((typeof item.id === 'string' && item.id.length > 0) || Number.isSafeInteger(item.id)) && ['Movie', 'Episode'].includes(item.item_type));
    if (!validItems || new Set(data.items.map(item => item.id)).size !== data.items.length) throw new Error('入库处理记录身份或类型无效');
    items.value = data.items;
    const refreshed = new Map(data.items.map(item => [item.id, item]));
    selectedItems.value = selectedItems.value.map(item => refreshed.get(item.id) || item);
    total.value = data.total;
    loadedQuery.value = targetQuery;
    itemsError.value = '';
  } catch (error) {
    if (active && sequence === itemSequence) itemsError.value = error.response?.data?.error || error.message || '读取入库处理记录失败，已保留此前选择';
  } finally {
    if (active && sequence === itemSequence) loadingItems.value = false;
  }
};

const searchItems = () => { clearTimeout(searchTimer); if (searchComposing.value || processing.value) return; appliedSearch.value = search.value?.trim() || ''; page.value = 1; loadItems(); };
const clearSearch = () => { search.value = ''; searchItems(); };
const scheduleSearch = () => {
  clearTimeout(searchTimer);
  if (!searchComposing.value && !processing.value && (search.value?.trim() || '') !== appliedSearch.value) searchTimer = setTimeout(searchItems, smAndDown.value ? 600 : 1000);
};
const finishSearchComposition = () => { searchComposing.value = false; scheduleSearch(); };
const removeSelection = id => { if (!processing.value) selectedItems.value = selectedItems.value.filter(item => item.id !== id); };
const updateSelection = ids => {
  if (!canSelectItems()) return;
  const current = new Set(items.value.map(item => item.id)), next = new Set(ids);
  selectedItems.value = selectedItems.value.filter(item => !current.has(item.id) || next.has(item.id));
  for (const item of items.value) {
    if (next.has(item.id) && canSelectRecord(item) && !selectedItems.value.some(selected => selected.id === item.id)) selectedItems.value.push({ ...item });
  }
};
const toggleSelection = (item, enabled) => updateSelection(enabled ? [...selectedIds.value, item.id] : selectedIds.value.filter(id => id !== item.id));
const openConfirmation = () => {
  if (processing.value || !selectedItems.value.length || !selectedItems.value.every(isReadyRecord)) return;
  confirmationItems.value = selectedItems.value.map(item => ({ ...item }));
  confirmationOpen.value = true;
};
const reorganize = async () => {
  if (processing.value || !confirmationItems.value.length) return;
  const ids = [...new Set(confirmationItems.value.map(item => item.item_id))];
  const historyIds = confirmationItems.value.map(item => item.id);
  processing.value = true;
  try {
    const response = await axios.post('/api/actions/reorganize', { item_ids: ids, history_ids: historyIds });
    if (!active) return;
    const result = response.data || {};
    const failures = Array.isArray(result.failures) ? result.failures : [];
    const warnings = Array.isArray(result.warning_details) ? result.warning_details : [];
    const details = [...failures, ...warnings].map(detail => typeof detail === 'string' ? detail : JSON.stringify(detail));
    const failed = failures.length > 0 || Number(result.failed) > 0 || result.success === false || Boolean(result.error);
    const verified = result.selected_count === ids.length && Array.isArray(result.items)
      && result.items.length === ids.length && new Set(result.items.map(item => item.item_id)).size === ids.length
      && result.items.every(item => ids.includes(item.item_id) && ['completed', 'failed'].includes(item.status));
    if (result.error) details.unshift(result.error);
    if (!verified) details.push('尚未取得完整任务结果，请在通知中心检查处理状态。');
    const type = failed ? 'error' : response.status === 207 || Number(result.warnings) > 0 || details.length ? 'warning' : 'success';
    feedback.value = { type, message: verified || failed ? result.message || result.error || '媒体重新整理完成' : '媒体整理结果尚未确认', details };
    message[type](feedback.value.message);
    confirmationOpen.value = false;
    if (verified) await loadItems();
  } catch (error) {
    if (!active) return;
    const result = error.response?.data || {};
    feedback.value = { type: 'error', message: result.error || result.message || error.message || '重新整理失败', details: [...(Array.isArray(result.failures) ? result.failures : []), ...(Array.isArray(result.warning_details) ? result.warning_details : [])] };
    message.error(feedback.value.message);
    confirmationOpen.value = false;
  } finally {
    processing.value = false;
  }
};

watch(search, scheduleSearch);
watch(statusFilter, () => { clearTimeout(searchTimer); appliedSearch.value = search.value?.trim() || ''; page.value = 1; loadItems(); });
useContextSearch({ label: '搜索入库记录', query: search, submit: searchItems, clear: clearSearch, composing: searchComposing, compositionEnd: finishSearchComposition, disabled: processing, loading: loadingItems });
onMounted(loadItems);
onBeforeUnmount(() => { active = false; ++itemSequence; clearTimeout(searchTimer); });
</script>

<style scoped>
.media-organize-page { width: 100%; min-width: 0; }
.organize-toolbar { display: flex; flex-wrap: wrap; align-items: center; gap: 12px; }
.organize-search { flex: 1 1 280px; min-width: 0; }
.organize-media-copy { min-width: 0; flex: 1; }
.media-title { max-width: 100%; height: auto; min-height: 36px; padding-block: 4px !important; border-radius: 0; white-space: normal; text-align: start; }
.media-title :deep(.v-btn__content) { white-space: normal; overflow-wrap: anywhere; }
.media-organize-page :deep(.v-card-title), .media-organize-page :deep(.v-card-subtitle) { white-space: normal; overflow-wrap: anywhere; }
.history-message { white-space: pre-wrap; overflow-wrap: anywhere; line-height: 1.5; }
.media-organize-page :deep(.v-data-table__td) { padding-block: 12px; }
.media-organize-page :deep(.v-alert-title), .media-organize-page :deep(.v-chip__content) { white-space: normal; overflow-wrap: anywhere; }
.media-organize-page :deep(.v-chip) { height: auto; min-height: 26px; padding-block: 4px; }
.history-cards { display: grid; gap: 12px; }
.history-card { min-width: 0; }
.history-card-body { display: flex; align-items: flex-start; gap: 12px; }
.history-card-body > .v-selection-control { flex: 0 0 auto; }
@media (max-width: 540px) { .media-organize-page :deep(.v-card-actions .v-pagination) { width: 100%; } }
</style>
