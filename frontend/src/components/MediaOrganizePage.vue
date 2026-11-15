<!-- 布局参考 MoviePilot-Frontend v3 TransferHistoryView/ReorganizeDialog（MIT），许可证见 frontend/third-party/MoviePilot-LICENSE。 -->
<template>
  <div ref="pageElement" class="mp-page media-organize-page" :class="{ 'media-organize-page--desktop': !smAndDown }" :style="smAndDown ? undefined : { blockSize: `${desktopPageHeight}px` }">
    <div class="organize-controls">
          <header class="organize-toolbar">
            <h2 class="text-h5 organize-page-title">媒体整理</h2>
            <div class="organize-toolbar-actions">
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
            </div>
          </header>

      <div v-if="selectedItems.length" class="organize-selection" role="group" aria-label="批量选择" data-testid="organize-selection">
        <div class="organize-selection-actions">
          <VCheckbox :model-value="allPageSelected" :indeterminate="pageSelectedCount > 0 && !allPageSelected" label="全选本页" density="compact" hide-details :disabled="!canSelectItems() || !selectablePageItems.length" data-testid="organize-select-page" @update:model-value="selectPage" />
          <span class="text-caption text-medium-emphasis" data-testid="organize-selection-count">{{ pageSelectedCount }}/{{ items.length }} 项</span>
          <VBtn size="small" color="primary" variant="tonal" prepend-icon="mdi-redo-variant" :disabled="!canReorganize" :loading="processing" data-testid="organize-selected" @click="openReprocessConfirmation">批量重新整理</VBtn>
          <VBtn size="small" color="error" variant="tonal" prepend-icon="mdi-trash-can-outline" :disabled="!canDelete" data-testid="organize-selected-delete" @click="openDeleteConfirmation()">批量删除</VBtn>
          <VBtn size="small" variant="text" :disabled="processing" @click="selectedItems = []">取消全选</VBtn>
        </div>
      </div>
      <div v-else class="organize-summary text-medium-emphasis">{{ total === null ? '正在读取入库记录' : `共 ${total} 条记录，${pages} 页` }}</div>

      <VAlert v-if="itemsError" type="error" variant="tonal" class="mb-4" data-testid="organize-load-error">{{ itemsError }}<VBtn variant="text" :loading="loadingItems" @click="loadItems">重新读取</VBtn></VAlert>
      <VAlert v-if="feedback" :type="feedback.type" :title="feedback.message" variant="tonal" class="mb-4" data-testid="organize-result">
        <div v-for="(detail, index) in feedback.details" :key="index" class="text-wrap">{{ detail }}</div>
      </VAlert>

    </div>

    <div class="organize-history-card">
      <div v-if="smAndDown" class="history-cards pa-3" data-testid="organize-items-mobile">
        <VProgressLinear v-if="loadingItems" indeterminate />
        <VVirtualScroll :key="loadedQuery" :items="items" :item-height="160" :height="largePage ? listHeight : undefined" :renderless="!largePage" class="history-card-scroll">
          <template #default="{ item, itemRef }">
            <div :ref="itemRef" class="history-card-wrapper">
              <VCard variant="outlined" class="history-card">
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
                  <VMenu location="bottom end"><template #activator="{ props }"><VBtn v-bind="props" icon="mdi-dots-vertical" variant="text" color="default" size="small" :aria-label="`${itemTitle(item)} 操作`" /></template><VList min-width="180"><VListItem title="重新整理" prepend-icon="mdi-redo-variant" :disabled="!canReorganizeRecord(item)" @click="openRecordConfirmation(item, 'reprocess')" /><VListItem title="重新刮削" prepend-icon="mdi-movie-search-outline" :disabled="!canReorganizeRecord(item)" @click="openRecordConfirmation(item, 'rescrape')" /><VListItem v-if="item.item_id" title="媒体信息编辑" prepend-icon="mdi-pencil-outline" :to="`/edit-media/${encodeURIComponent(item.item_id)}`" /><VListItem title="删除" prepend-icon="mdi-trash-can-outline" :disabled="!canSelectRecord(item)" @click="openDeleteConfirmation(item)" /></VList></VMenu>
                </VCardText>
              </VCard>
            </div>
          </template>
        </VVirtualScroll>
        <div v-if="!items.length && !loadingItems" class="py-8 text-center text-medium-emphasis">{{ emptyText }}</div>
      </div>
      <VDataTableVirtual v-else :key="loadedQuery" :headers="headers" :items="items" item-value="id" :loading="loadingItems" density="compact" height="100%" :item-height="100" class="organize-items-desktop" hide-default-header hide-default-footer data-testid="organize-items">
        <template #item="{ item, columns, itemRef }">
          <tr :ref="itemRef" class="organize-record-row"><td :colspan="columns.length">
            <VCard class="organize-record" :class="{ 'organize-record--selected': selectedIds.includes(item.id) }">
              <VCheckbox :model-value="selectedIds.includes(item.id)" :disabled="!canSelectRecord(item)" :aria-label="`选择 ${itemTitle(item)}`" density="compact" hide-details @update:model-value="toggleSelection(item, $event)" />
              <VAvatar rounded width="42" height="63"><VImg v-if="poster(item)" :src="poster(item)" :alt="itemTitle(item)" cover /><VIcon v-else icon="mdi-movie-open-outline" /></VAvatar>
              <div class="organize-media-copy"><VBtn v-if="item.item_id" variant="text" color="default" class="media-title text-none px-0" :to="`/edit-media/${encodeURIComponent(item.item_id)}`">{{ itemTitle(item) }}</VBtn><div v-else class="font-weight-medium">{{ itemTitle(item) }}</div><div class="text-caption text-medium-emphasis">{{ episodeLabel(item) || (item.item_type === 'Movie' ? '电影' : '剧集') }}</div><div v-if="!item.item_id" class="text-caption text-medium-emphasis">尚未关联媒体</div></div>
              <div class="organize-record-meta text-caption text-medium-emphasis"><div>{{ item.library_name || '—' }}</div><div v-if="item.file_name" :title="item.file_name" class="organize-file-name">{{ item.file_name }}</div><div>{{ formatDate(item.created_at) }}</div></div>
              <div class="organize-record-status"><VChip size="small" variant="tonal" :color="statusPresentation(item).color">{{ statusPresentation(item).title }}</VChip><div v-if="item.message" class="text-body-2 mt-1 history-message">{{ item.message }}</div></div>
              <VMenu location="bottom end"><template #activator="{ props }"><VBtn v-bind="props" icon="mdi-dots-vertical" variant="text" color="default" :aria-label="`${itemTitle(item)} 操作`" /></template><VList min-width="180"><VListItem title="重新整理" prepend-icon="mdi-redo-variant" :disabled="!canReorganizeRecord(item)" @click="openRecordConfirmation(item, 'reprocess')" /><VListItem title="重新刮削" prepend-icon="mdi-movie-search-outline" :disabled="!canReorganizeRecord(item)" @click="openRecordConfirmation(item, 'rescrape')" /><VListItem v-if="item.item_id" title="媒体信息编辑" prepend-icon="mdi-pencil-outline" :to="`/edit-media/${encodeURIComponent(item.item_id)}`" /><VListItem title="删除" prepend-icon="mdi-trash-can-outline" :disabled="!canSelectRecord(item)" @click="openDeleteConfirmation(item)" /></VList></VMenu>
            </VCard>
          </td></tr>
        </template>
        <template #no-data><div class="py-8 text-medium-emphasis">{{ emptyText }}</div></template>
      </VDataTableVirtual>
      <footer ref="paginationElement" class="organize-pagination" data-testid="organize-footer">
        <VSelect :model-value="pageSize" :items="pageSizeOptions" density="compact" variant="solo" flat rounded="pill" hide-details aria-label="每页条数" title="每页条数" class="organize-page-size" :disabled="processing" data-testid="organize-page-size" @update:model-value="changePageSize" />
        <span class="organize-pagination-info text-medium-emphasis" :aria-label="total === null ? '总数未确认' : `第 ${pageTip.begin} 至 ${pageTip.end} 条，共 ${total} 条记录`" data-testid="organize-page-info">{{ total === null ? '总数未确认' : `${pageTip.begin} - ${pageTip.end} / ${total}` }}</span>
        <VPagination v-model="page" show-first-last-page :length="pages" :total-visible="smAndDown ? 3 : 7" :disabled="loadingItems || processing" data-testid="organize-pagination" @update:model-value="loadItems" />
      </footer>
    </div>

    <VDialog v-model="confirmationOpen" max-width="720" scrollable :fullscreen="smAndDown" :persistent="processing">
      <VCard class="organize-confirmation-card">
        <VCardItem class="py-2">
          <template #prepend><VIcon icon="mdi-folder-move" class="me-2" /></template>
          <VCardTitle>确认{{ operationLabel(confirmationMode) }}</VCardTitle>
          <VCardSubtitle>{{ confirmationItems.length }} 条记录，{{ confirmationMediaCount }} 项媒体</VCardSubtitle>
          <template #append><VBtn icon="mdi-close" variant="text" aria-label="关闭整理确认" :disabled="processing" @click="confirmationOpen = false" /></template>
        </VCardItem>
        <VDivider />
        <VCardText>
          <VSelect v-model="confirmationMode" :items="operationOptions" :disabled="processing" label="整理方式" prepend-inner-icon="mdi-swap-horizontal" :hint="confirmationMode === 'rescrape' ? '更新元数据、NFO 和图片，保留已有媒体信息、音轨和章节。' : '从所选媒体重新执行完整入库流程，包括媒体信息重建、回读验收和元数据增强。'" persistent-hint data-testid="organize-mode" />
          <VAlert type="info" variant="tonal" density="compact" class="my-4">电影包含关联版本，剧集只处理所选单集。同一媒体的多条记录合并为一次{{ confirmationMode === 'rescrape' ? '刮削' : '整理' }}。</VAlert>
          <VList bg-color="transparent" density="compact">
            <VVirtualScroll v-if="confirmationItems.length > 100" :items="confirmationItems" :height="Math.min(listHeight, 384)" :item-height="64" tabindex="0" aria-label="本次整理的媒体" data-testid="organize-confirmation-scroll">
              <template #default="{ item }"><VListItem :title="itemTitle(item)" :subtitle="`${episodeLabel(item) || (item.item_type === 'Movie' ? '电影' : '剧集')} · ${item.item_id}`" /></template>
            </VVirtualScroll>
            <template v-else><VListItem v-for="item in confirmationItems" :key="item.id" :title="itemTitle(item)" :subtitle="`${episodeLabel(item) || (item.item_type === 'Movie' ? '电影' : '剧集')} · ${item.item_id}`" /></template>
          </VList>
        </VCardText>
        <VDivider /><VCardActions><VSpacer /><VBtn variant="tonal" color="secondary" :disabled="processing" @click="confirmationOpen = false">取消</VBtn><VBtn color="primary" variant="flat" :loading="processing" data-testid="organize-confirm" @click="reorganize">加入{{ operationLabel(confirmationMode) }}队列</VBtn></VCardActions>
      </VCard>
    </VDialog>
    <VBottomSheet v-model="deleteOpen" inset :persistent="processing">
      <VCard class="text-center">
        <VCardTitle>删除选中的 {{ deleteItems.length }} 条整理记录</VCardTitle>
        <VCardText>
          <p>仅删除整理记录，媒体文件和媒体库项目保留。</p>
          <VVirtualScroll v-if="deleteItems.length > 100" :items="deleteItems" :height="Math.min(listHeight, 256)" :item-height="48" data-testid="organize-delete-scroll">
            <template #default="{ item }"><VListItem :title="itemTitle(item)" :subtitle="episodeLabel(item) || item.file_name || item.id" /></template>
          </VVirtualScroll>
          <VList v-else bg-color="transparent" density="compact"><VListItem v-for="item in deleteItems" :key="item.id" :title="itemTitle(item)" :subtitle="episodeLabel(item) || item.file_name || item.id" /></VList>
        </VCardText>
        <VCardActions class="justify-center"><VBtn variant="text" :disabled="processing" @click="deleteOpen = false">取消</VBtn><VBtn color="error" variant="flat" :loading="processing" data-testid="organize-delete-confirm" @click="deleteHistory">仅删除整理记录</VBtn></VCardActions>
      </VCard>
    </VBottomSheet>
    <MediaOrganizeQueueDialog v-model="queueOpen" :queue="queue" />
    <Teleport to="body">
      <div class="compact-fab-stack compact-fab-stack--history organize-queue-fab" :class="{ 'organize-queue-fab--desktop': !smAndDown }" :style="smAndDown ? undefined : { insetBlockEnd: `${desktopFabBottom}px` }">
        <VBadge :content="pendingQueueCount" :model-value="pendingQueueCount > 0" color="error">
          <VBtn icon="mdi-timer-sand-paused" color="primary" size="large" class="compact-fab compact-fab--primary" aria-label="整理队列" title="整理队列" data-testid="organize-queue-open" @click="openQueue" />
        </VBadge>
      </div>
    </Teleport>
  </div>
</template>

<script setup>
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useElementBounding } from '@vueuse/core';
import { useContextSearch } from '@/composables/useContextSearch';
import { useDisplay } from 'vuetify';
import { VDataTableVirtual } from 'vuetify/components';
import axios from 'axios';
import { useMessage } from '@/composables/useFeedback';
import { queueReceiptMatches, useMediaOrganizeQueue } from '@/composables/useMediaOrganizeQueue';
import MediaOrganizeQueueDialog from '@/components/MediaOrganizeQueueDialog.vue';

const message = useMessage();
const { smAndDown, height: viewportHeight, width: viewportWidth } = useDisplay();
const items = ref([]), search = ref(''), appliedSearch = ref(''), statusFilter = ref('all'), page = ref(1), total = ref(null), searchComposing = ref(false);
const statusOptions = [{ title: '全部状态', value: 'all' }, { title: '成功', value: 'success' }, { title: '失败', value: 'failed' }, { title: '有提醒', value: 'warning' }, { title: '正在处理', value: 'processing' }];
const pageSizeOptions = [25, 50, 100, 500, 1000];
const pageSize = ref(50);
const largePage = computed(() => pageSize.value >= 500);
const listHeight = computed(() => Math.min(640, Math.max(240, viewportHeight.value * 0.65)));
const pageElement = ref(null), paginationElement = ref(null), contentBottomInset = ref(24);
const { top: pageTop } = useElementBounding(pageElement);
const { top: paginationTop } = useElementBounding(paginationElement);
const desktopPageHeight = computed(() => Math.max(0, viewportHeight.value - Math.max(0, pageTop.value) - contentBottomInset.value));
const desktopFabBottom = computed(() => Math.max(72, viewportHeight.value - paginationTop.value + 24));
// 布局已为 Dock 和安全区域预留底部 padding，按实际值计算，避免重复扣除。
watch([pageElement, viewportWidth, viewportHeight], () => {
  const layout = pageElement.value?.closest('.layout-page-content');
  if (layout) contentBottomInset.value = parseFloat(getComputedStyle(layout).paddingBottom) || 0;
}, { flush: 'post' });
const selectedItems = ref([]), confirmationItems = ref([]), confirmationOpen = ref(false), confirmationMode = ref('reprocess'), queueOpen = ref(false), deleteItems = ref([]), deleteOpen = ref(false);
const loadingItems = ref(false), itemsError = ref(''), processing = ref(false), feedback = ref(null);
let active = true, itemSequence = 0, searchTimer;
const loadedQuery = ref('');
const queryKey = () => JSON.stringify([page.value, pageSize.value, appliedSearch.value, statusFilter.value]);
const selectedIds = computed(() => selectedItems.value.map(item => item.id));
const selectedPageItems = computed(() => items.value.filter(item => selectedIds.value.includes(item.id)));
const canReorganize = computed(() => canSelectItems() && selectedPageItems.value.length > 0 && selectedPageItems.value.every(isReadyRecord));
const canDelete = computed(() => canSelectItems() && selectedPageItems.value.length > 0 && selectedPageItems.value.every(isDeletableRecord));
const confirmationMediaCount = computed(() => new Set(confirmationItems.value.map(item => item.item_id)).size);
const pages = computed(() => Math.max(1, Math.ceil((total.value || 0) / pageSize.value)));
const pageTip = computed(() => ({ begin: total.value ? pageSize.value * (page.value - 1) + 1 : 0, end: Math.min(pageSize.value * page.value, total.value || 0) }));
const emptyText = computed(() => itemsError.value ? '入库处理记录尚未读取成功' : '当前条件下没有入库处理记录');
const canSelectItems = () => !processing.value && !loadingItems.value && !itemsError.value && loadedQuery.value === queryKey();
const isDeletableRecord = item => ['Movie', 'Episode'].includes(item.item_type) && ['success', 'warning', 'failed'].includes(item.status);
const isReadyRecord = item => isDeletableRecord(item) && typeof item.item_id === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(item.item_id);
const canSelectRecord = item => canSelectItems() && isDeletableRecord(item);
const canReorganizeRecord = item => canSelectItems() && isReadyRecord(item);
const headers = [{ title: '媒体', key: 'name', sortable: false }, { title: '媒体库', key: 'library_name', sortable: false }, { title: '类型', key: 'item_type', sortable: false }, { title: '处理开始时间', key: 'created_at', sortable: false }, { title: '状态', key: 'status', sortable: false }];
const selectablePageItems = computed(() => items.value.filter(isDeletableRecord));
const pageSelectedCount = computed(() => selectablePageItems.value.filter(item => selectedIds.value.includes(item.id)).length);
const allPageSelected = computed(() => selectablePageItems.value.length > 0 && pageSelectedCount.value === selectablePageItems.value.length);
const selectPage = enabled => updateSelection(enabled ? [...selectedIds.value, ...selectablePageItems.value.map(item => item.id)] : selectedIds.value.filter(id => !selectablePageItems.value.some(item => item.id === id)));
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
      params: { page: page.value, page_size: pageSize.value, search: appliedSearch.value, status: statusFilter.value },
    });
    if (!active || sequence !== itemSequence || targetQuery !== queryKey()) return;
    if (!Array.isArray(data?.items) || !Number.isInteger(data?.total) || data.total < data.items.length) throw new Error('入库处理记录或分页总数无效');
    const validItems = data.items.every(item => ((typeof item.id === 'string' && item.id.length > 0) || Number.isSafeInteger(item.id)) && ['Movie', 'Episode'].includes(item.item_type));
    if (!validItems || new Set(data.items.map(item => item.id)).size !== data.items.length) throw new Error('入库处理记录身份或类型无效');
    const lastPage = Math.max(1, Math.ceil(data.total / pageSize.value));
    if (page.value > lastPage) {
      page.value = lastPage;
      await loadItems();
      return;
    }
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

const changePageSize = async value => {
  if (processing.value || !pageSizeOptions.includes(value) || value === pageSize.value) return;
  pageSize.value = value;
  page.value = 1;
  await loadItems();
};

const queue = useMediaOrganizeQueue({ onSettled: loadItems });
const pendingQueueCount = computed(() => queue.queuedCount + queue.runningCount);
const operationLabel = mode => mode === 'rescrape' ? '重新刮削' : '重新整理入库';
const operationOptions = [{ title: '重新整理入库', value: 'reprocess' }, { title: '重新刮削', value: 'rescrape' }];
const openQueue = () => { queueOpen.value = true; void queue.refresh(); };

const searchItems = () => { clearTimeout(searchTimer); if (searchComposing.value || processing.value) return; appliedSearch.value = search.value?.trim() || ''; page.value = 1; loadItems(); };
const clearSearch = () => { search.value = ''; searchItems(); };
const scheduleSearch = () => {
  clearTimeout(searchTimer);
  if (!searchComposing.value && !processing.value && (search.value?.trim() || '') !== appliedSearch.value) searchTimer = setTimeout(searchItems, smAndDown.value ? 600 : 1000);
};
const finishSearchComposition = () => { searchComposing.value = false; scheduleSearch(); };
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
  const targets = selectedPageItems.value;
  if (!canSelectItems() || !targets.length || !targets.every(isReadyRecord)) return;
  selectedItems.value = targets.map(item => ({ ...item }));
  confirmationItems.value = selectedItems.value.map(item => ({ ...item }));
  confirmationOpen.value = true;
};
const openReprocessConfirmation = () => { if (canReorganize.value) { confirmationMode.value = 'reprocess'; openConfirmation(); } };
const openRecordConfirmation = (item, mode) => {
  if (!canReorganizeRecord(item)) return;
  confirmationItems.value = [{ ...item }];
  confirmationMode.value = mode;
  confirmationOpen.value = true;
};
const openDeleteConfirmation = item => {
  const targets = item ? [item] : selectedPageItems.value;
  if (!canSelectItems() || !targets.length || !targets.every(isDeletableRecord)) return;
  if (!item) selectedItems.value = targets.map(target => ({ ...target }));
  deleteItems.value = targets.map(target => ({ ...target }));
  deleteOpen.value = true;
};
const deleteHistory = async () => {
  if (processing.value || !deleteItems.value.length || !deleteItems.value.every(isDeletableRecord)) return;
  const ids = deleteItems.value.map(item => item.id);
  processing.value = true;
  try {
    const { data } = await axios.delete('/api/media_organize/history', { data: { history_ids: ids } });
    if (!active) return;
    const verified = data?.deleted_count === ids.length && Array.isArray(data.deleted_ids)
      && data.deleted_ids.length === ids.length && new Set(data.deleted_ids).size === ids.length
      && data.deleted_ids.every(id => ids.includes(id));
    feedback.value = { type: verified ? 'success' : 'warning', message: verified ? data.message || '整理记录已删除' : '整理记录删除结果尚未确认', details: verified ? [] : ['已保留选择，请刷新记录后核对。'] };
    message[feedback.value.type](feedback.value.message);
    deleteOpen.value = false;
    if (verified) {
      selectedItems.value = selectedItems.value.filter(item => !ids.includes(item.id));
      deleteItems.value = [];
      await loadItems();
    }
  } catch (error) {
    if (!active) return;
    feedback.value = { type: 'error', message: error.response?.data?.error || error.message || '删除整理记录失败', details: [] };
    message.error(feedback.value.message);
    deleteOpen.value = false;
  } finally {
    processing.value = false;
  }
};
const reorganize = async () => {
  if (processing.value || !confirmationItems.value.length || !['reprocess', 'rescrape'].includes(confirmationMode.value)) return;
  const mode = confirmationMode.value;
  const ids = [...new Set(confirmationItems.value.map(item => item.item_id))];
  const historyIds = confirmationItems.value.map(item => item.id);
  processing.value = true;
  try {
    const response = await axios.post('/api/actions/reorganize', { item_ids: ids, history_ids: historyIds, mode });
    if (!active) return;
    const result = response.data || {};
    if (response.status === 202) {
      const accepted = queueReceiptMatches(result, ids, mode);
      feedback.value = accepted ? null : { type: 'warning', message: '整理队列提交结果尚未确认', details: ['请打开整理队列核对所选媒体的状态。'] };
      if (!accepted) message.warning(feedback.value.message);
      confirmationOpen.value = false;
      queueOpen.value = true;
      void queue.refresh();
      if (accepted) void loadItems();
      return;
    }
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
onMounted(() => { queue.start(); void loadItems(); });
onBeforeUnmount(() => { active = false; ++itemSequence; clearTimeout(searchTimer); queue.stop(); });
</script>

<style scoped>
.media-organize-page { width: 100%; min-width: 0; }
.media-organize-page--desktop { display: flex; flex-direction: column; min-block-size: 0; overflow: hidden; }
.media-organize-page { display: flex; flex-direction: column; gap: 12px; }
.media-organize-page--desktop .organize-controls { flex: 0 1 auto; min-block-size: 0; max-block-size: 50%; overflow: auto; overscroll-behavior: contain; }
.organize-controls { display: flex; flex-direction: column; gap: 12px; }
.media-organize-page--desktop .organize-history-card { display: flex; flex-direction: column; flex: 1 1 0; min-block-size: 0; }
.organize-items-desktop { flex: 1 1 0; min-block-size: 0; }
.organize-queue-fab { z-index: 1100; }
.organize-queue-fab--desktop { inset-inline-end: max(2rem, calc(env(safe-area-inset-right) + 2rem)); }
.organize-toolbar { display: flex; flex-wrap: wrap; justify-content: space-between; align-items: center; gap: 12px; }
.organize-page-title { margin: 0; flex: 0 0 auto; }
.organize-toolbar-actions { display: flex; align-items: center; gap: 4px; min-width: 0; }
.organize-search { inline-size: clamp(12rem, 24vw, 24rem); flex: 0 1 auto; min-width: 0; margin-inline-end: 8px; }
.organize-search :deep(.v-field), .organize-page-size :deep(.v-field) { border: 0 !important; border-radius: var(--app-vuetify-rounded-pill); background: rgba(var(--v-theme-surface-variant), 0.04) !important; box-shadow: none !important; min-block-size: 42px; }
.organize-search :deep(.v-field__outline), .organize-search :deep(.v-field__overlay), .organize-page-size :deep(.v-field__outline), .organize-page-size :deep(.v-field__overlay) { display: none; }
.organize-selection { flex: 0 0 auto; }
.organize-selection-actions { display: flex; flex-wrap: wrap; align-items: center; gap: 12px; padding: 8px 16px; min-block-size: 58px; }
.organize-selection-actions > .v-input { flex: 0 0 auto; }
.organize-summary { padding-inline: 16px; font-size: 0.85rem; }
.organize-record-row > td { padding: 0 0 10px !important; border: 0 !important; }
.organize-record { display: flex; align-items: center; gap: 12px; min-block-size: 90px; padding: 8px 12px; }
.organize-record > .v-input { flex: 0 0 auto; }
.organize-record--selected { outline: 1px solid rgba(var(--v-theme-primary), 0.45); outline-offset: -1px; }
.organize-record-meta { flex: 0 1 28%; min-width: 0; }
.organize-file-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.organize-record-status { flex: 0 1 22%; min-width: 120px; }
.organize-items-desktop { background: transparent; }
.organize-items-desktop :deep(.v-table__wrapper) { scrollbar-gutter: stable; }
.organize-media-copy { min-width: 0; flex: 1; }
.media-title { max-width: 100%; height: auto; min-height: 36px; padding-block: 4px !important; border-radius: 0; white-space: normal; text-align: start; }
.media-title :deep(.v-btn__content) { white-space: normal; overflow-wrap: anywhere; }
.media-organize-page :deep(.v-card-title), .media-organize-page :deep(.v-card-subtitle) { white-space: normal; overflow-wrap: anywhere; }
.history-message { white-space: pre-wrap; overflow-wrap: anywhere; line-height: 1.5; }
.media-organize-page :deep(.v-data-table__td) { padding-block: 12px; }
.media-organize-page :deep(.v-alert-title), .media-organize-page :deep(.v-chip__content) { white-space: normal; overflow-wrap: anywhere; }
.media-organize-page :deep(.v-chip) { height: auto; min-height: 26px; padding-block: 4px; }
.history-cards { min-width: 0; }
.history-card { min-width: 0; }
.history-card-body { display: flex; align-items: flex-start; gap: 12px; }
.history-card-body > .v-selection-control { flex: 0 0 auto; }
.history-card-wrapper { padding-bottom: 12px; }
.history-card-scroll { min-width: 0; }
.organize-pagination { display: flex; flex-wrap: wrap; flex: 0 0 auto; align-items: center; gap: 12px; }
.organize-page-size { flex: 0 0 100px; }
.organize-pagination-info { font-size: 0.8rem; }
.organize-pagination :deep(.v-pagination) { margin-inline-start: auto; }
.organize-pagination :deep(.v-pagination__item), .organize-pagination :deep(.v-pagination__first), .organize-pagination :deep(.v-pagination__prev), .organize-pagination :deep(.v-pagination__next), .organize-pagination :deep(.v-pagination__last) { margin-block: 0; }
@media (max-width: 959px) { .organize-toolbar-actions { flex: 1 1 100%; } .organize-search { flex: 1 1 auto; } }
@media (max-width: 540px) { .organize-pagination :deep(.v-pagination) { width: 100%; } }
</style>
