<!-- 布局参考 MoviePilot-Frontend v3 TransferHistoryView/TransferRecoveryDialog/ReorganizeDialog/TransferHistoryDeleteDialog（MIT），许可证见 frontend/third-party/MoviePilot-LICENSE。 -->
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
            <template v-if="!smAndDown">
              <VMenu location="bottom end" :close-on-content-click="false">
                <template #activator="{ props }"><VBtn v-bind="props" icon="mdi-sort-variant" variant="text" :color="sortKey ? 'primary' : 'default'" aria-label="排序" /></template>
                <VList min-width="180">
                  <VListItem v-for="option in sortOptions" :key="option.value" :title="option.title" :active="sortKey === option.value" @click="sortKey = option.value" />
                  <VDivider /><VListItem :disabled="!sortKey" :title="descending ? '降序' : '升序'" :prepend-icon="descending ? 'mdi-sort-descending' : 'mdi-sort-ascending'" @click="descending = !descending" />
                </VList>
              </VMenu>
              <VMenu location="bottom end">
                <template #activator="{ props }"><VBtn v-bind="props" :icon="grouped ? 'mdi-format-list-group' : 'mdi-format-list-bulleted'" variant="text" aria-label="视图布局" /></template>
                <VList><VListItem title="列表模式" :active="!grouped" @click="grouped = false" /><VListItem title="分组模式" :active="grouped" @click="grouped = true" /></VList>
              </VMenu>
            </template>
            <VBtn v-if="smAndDown" icon="mdi-checkbox-multiple-marked-outline" variant="text" :color="mobileBatch ? 'primary' : 'default'"
              :aria-label="mobileBatch ? '退出批量选择' : '批量选择'" :aria-pressed="mobileBatch" :disabled="processing" @click="toggleMobileBatch" />
            <VBtn icon="mdi-refresh" variant="text" color="default" aria-label="刷新入库记录" :loading="loadingItems" :disabled="processing" @click="loadItems" />
            </div>
          </header>

      <div v-if="selectedItems.length || (smAndDown && mobileBatch)" class="organize-selection" role="group" aria-label="批量选择" data-testid="organize-selection">
        <div class="organize-selection-actions">
          <VCheckbox :model-value="allPageSelected" :indeterminate="pageSelectedCount > 0 && !allPageSelected" :label="smAndDown ? '全选已加载' : '全选本页'" density="compact" hide-details :disabled="!canSelectItems() || !selectablePageItems.length" data-testid="organize-select-page" @update:model-value="selectPage" />
          <span v-if="pageSelectedCount > 1000" class="text-caption">单次最多处理 1000 项</span>
          <span class="text-caption text-medium-emphasis" data-testid="organize-selection-count">{{ pageSelectedCount }}/{{ items.length }} 项</span>
          <VBtn size="small" color="primary" variant="tonal" prepend-icon="mdi-redo-variant" :disabled="!canReorganize" :loading="processing" data-testid="organize-selected" @click="openReprocessConfirmation">批量重新整理</VBtn>
          <VBtn size="small" color="error" variant="tonal" prepend-icon="mdi-trash-can-outline" :disabled="!canDelete" data-testid="organize-selected-delete" @click="openDeleteConfirmation()">批量删除</VBtn>
          <VBtn size="small" variant="text" :disabled="processing" @click="selectedItems = []">取消全选</VBtn>
        </div>
      </div>
      <div v-else class="organize-summary text-medium-emphasis">{{ total === null ? '正在读取入库记录' : smAndDown ? `共 ${total} 条记录` : `共 ${total} 条记录，${pages} 页` }}</div>

      <VAlert v-if="itemsError" type="error" variant="tonal" class="mb-4" data-testid="organize-load-error">{{ itemsError }}<VBtn variant="text" :loading="loadingItems" @click="loadItems">重新读取</VBtn></VAlert>
      <VAlert v-if="feedback" :type="feedback.type" :title="feedback.message" variant="tonal" class="mb-4" data-testid="organize-result">
        <div v-for="(detail, index) in feedback.details" :key="index" class="text-wrap">{{ detail }}</div>
      </VAlert>

    </div>

    <DefineRecord v-slot="{ item }">
      <VCard :variant="smAndDown ? 'outlined' : undefined" class="organize-record" :class="{ 'history-card': smAndDown, 'organize-record--selected': selectedIds.includes(item.id) }">
        <VCheckbox v-if="!smAndDown || mobileBatch" class="organize-check" :model-value="selectedIds.includes(item.id)" :disabled="!canSelectRecord(item)"
          :aria-label="`选择 ${itemTitle(item)}`" density="compact" hide-details @click="range.onCheckboxClick" @input="range.onCheckboxInput(item, $event)" @update:model-value="checked => range.selectFromCheckbox(item, !!checked)" />
        <OrganizePoster class="record-poster" :item="item" :title="itemTitle(item)" :width="smAndDown ? 56 : 42" :height="smAndDown ? 84 : 63" />
        <div class="organize-media-copy">
          <RouterLink v-if="item.item_id" class="media-title" :to="`/edit-media/${encodeURIComponent(item.item_id)}`">{{ itemTitle(item) }}</RouterLink>
          <strong v-else>{{ itemTitle(item) }}</strong>
          <small v-if="!item.item_id">尚未关联媒体</small>
        </div>
        <small class="organize-episode-label">{{ episodeLabel(item) || (item.item_type === 'Movie' ? '电影' : '剧集') }}</small>
        <div v-if="mediaFileName(item) || item.library_name || item.source_path" class="organize-record-file">
          <small v-if="item.library_name">{{ item.library_name }}</small>
          <button v-if="smAndDown && item.source_path" type="button" class="organize-file-name text-start" :aria-expanded="expandedPath === item.id" @click="expandedPath = expandedPath === item.id ? '' : item.id">{{ mediaFileName(item) || item.source_path }}</button>
          <div v-else class="organize-file-name" :title="item.source_path">{{ mediaFileName(item) || item.source_path }}</div>
          <small v-if="smAndDown && expandedPath === item.id">{{ item.source_path }}</small>
        </div>
        <VChip tag="button" type="button" size="small" variant="tonal" class="record-status" :color="statusPresentation(item).color"
          :aria-label="`${statusPresentation(item).title}，查看${itemTitle(item)}详情`" @click.stop="openDetails(item)">{{ statusPresentation(item).title }}</VChip>
        <VMenu v-if="!smAndDown || !mobileBatch" location="bottom end">
          <template #activator="{ props }">
            <VBtn v-bind="props" class="record-menu" icon="mdi-dots-vertical" variant="text" size="small" :aria-label="`${itemTitle(item)} 操作`" />
          </template>
          <VList min-width="180">
            <VListItem title="重新整理" prepend-icon="mdi-redo-variant" :disabled="!canReorganizeRecord(item)" @click="openRecordConfirmation(item, 'reprocess')" />
            <VListItem title="重新刮削" prepend-icon="mdi-movie-search-outline" :disabled="!canReorganizeRecord(item)" @click="openRecordConfirmation(item, 'rescrape')" />
            <VListItem v-if="item.item_id" title="媒体信息编辑" prepend-icon="mdi-pencil-outline" :to="`/edit-media/${encodeURIComponent(item.item_id)}`" />
            <VListItem title="删除" prepend-icon="mdi-trash-can-outline" :disabled="!canSelectRecord(item)" @click="openDeleteConfirmation(item)" />
          </VList>
        </VMenu>
        <time v-if="item.created_at" :datetime="item.created_at" :title="formatDate(item.created_at)">{{ recordDate(item.created_at) }}</time>
      </VCard>
    </DefineRecord>

    <div class="organize-history-card">
      <div v-if="smAndDown" class="history-cards pa-3" data-testid="organize-items-mobile">
        <VProgressLinear v-if="loadingItems" indeterminate />
        <VVirtualScroll :key="loadedQuery" :items="items" :item-height="160" renderless class="history-card-scroll">
          <template #default="{ item, itemRef }">
            <div :key="item.id" :ref="itemRef" class="history-card-wrapper">
              <RecordCard :item="item" />
            </div>
          </template>
        </VVirtualScroll>
        <VBtn v-if="!mobileEnd && items.length && !itemsError && !loadingItems" ref="moreElement" block variant="text" @click="loadItems(true)">加载更多</VBtn>
        <div v-if="!items.length && !loadingItems" class="py-8 text-center text-medium-emphasis">{{ emptyText }}</div>
      </div>
      <VDataTableVirtual v-else :key="loadedQuery" :headers="headers" :items="desktopRows" item-value="id" :loading="loadingItems" density="compact" height="100%" :item-height="100" class="organize-items-desktop" hide-default-header hide-default-footer data-testid="organize-items">
        <template #item="{ item, columns, itemRef }">
          <tr :key="item.id" :ref="itemRef" class="organize-record-row"><td :colspan="columns.length">
            <div v-if="item.members" class="organize-group">
              <VCheckbox :model-value="item.members.filter(isDeletableRecord).every(row => selectedIds.includes(row.id))" :indeterminate="item.members.some(row => selectedIds.includes(row.id)) && !item.members.filter(isDeletableRecord).every(row => selectedIds.includes(row.id))" :disabled="!canSelectItems() || !item.members.some(isDeletableRecord)" :aria-label="`选择分组 ${item.title}`" density="compact" hide-details @update:model-value="setSelection(item.members, $event)" />
              <VBtn variant="text" :aria-expanded="openGroups.has(item.title)" :prepend-icon="openGroups.has(item.title) ? 'mdi-chevron-down' : 'mdi-chevron-right'" @click="toggleGroup(item.title)">{{ item.title }} · {{ item.members.length }} 项</VBtn>
            </div>
            <RecordCard v-else :item="item" />
          </td></tr>
        </template>
        <template #no-data><div class="py-8 text-medium-emphasis">{{ emptyText }}</div></template>
      </VDataTableVirtual>
      <footer v-if="!smAndDown" ref="paginationElement" class="organize-pagination" data-testid="organize-footer">
        <VSelect :model-value="pageSize" :items="pageSizeOptions" density="compact" variant="solo" flat rounded="pill" hide-details aria-label="每页条数" title="每页条数" class="organize-page-size" :disabled="processing" data-testid="organize-page-size" @update:model-value="changePageSize" />
        <span class="organize-pagination-info text-medium-emphasis" :aria-label="total === null ? '总数未确认' : `第 ${pageTip.begin} 至 ${pageTip.end} 条，共 ${total} 条记录`" data-testid="organize-page-info">{{ total === null ? '总数未确认' : `${pageTip.begin} - ${pageTip.end} / ${total}` }}</span>
        <VPagination v-model="page" show-first-last-page :length="pages" :total-visible="smAndDown ? 3 : 7" :disabled="loadingItems || processing" data-testid="organize-pagination" @update:model-value="loadItems" />
      </footer>
    </div>

    <VDialog v-model="detailOpen" max-width="42rem" scrollable :fullscreen="smAndDown">
      <VCard v-if="detailItem" data-testid="organize-detail-dialog">
        <VCardItem class="pe-16">
          <template #prepend><VIcon icon="mdi-history" /></template>
          <VCardTitle>整理详情</VCardTitle>
        </VCardItem>
        <DialogCloseBtn aria-label="关闭整理详情" @click="detailOpen = false" />
        <VDivider />
        <VCardText class="organize-detail-content">
          <div class="text-subtitle-1 font-weight-medium">{{ itemTitle(detailItem) }}<span v-if="episodeLabel(detailItem)" class="text-body-2 text-medium-emphasis ms-2">{{ episodeLabel(detailItem) }}</span></div>
          <div v-if="mediaFileName(detailItem)"><strong>文件：</strong>{{ mediaFileName(detailItem) }}</div>
          <div v-if="detailItem.source_path"><strong>入库路径：</strong>{{ detailItem.source_path }}</div>
          <div v-if="detailItem.library_name"><strong>媒体库：</strong>{{ detailItem.library_name }}</div>
          <div><strong>开始时间：</strong>{{ formatDate(detailItem.created_at) }}</div>
          <div v-if="detailItem.completed_at"><strong>完成时间：</strong>{{ formatDate(detailItem.completed_at) }}</div>
          <div><strong>状态：</strong><VChip size="small" variant="tonal" :color="statusPresentation(detailItem).color">{{ statusPresentation(detailItem).title }}</VChip></div>
          <VAlert :type="detailItem.status === 'failed' ? 'error' : detailItem.status === 'warning' ? 'warning' : detailItem.status === 'success' ? 'success' : 'info'" variant="tonal" class="history-message"><template v-if="detailItem.item_warnings?.length || detailItem.batch_warnings?.length">
            <div v-for="(line, i) in detailItem.item_warnings" :key="i">{{ line }}</div>
            <details v-if="detailItem.batch_warnings?.length"><summary>本批处理提醒</summary><div v-for="line in detailItem.batch_warnings" :key="line">{{ line }}</div></details>
          </template><template v-else>{{ detailItem.message || '暂无详细处理信息' }}</template></VAlert>
          <div v-if="!detailItem.item_id" class="text-body-2 text-medium-emphasis">尚未关联媒体，暂不能重新整理或刮削。</div>
        </VCardText>
        <VDivider />
        <VCardActions class="app-dialog-actions">
          <VBtn variant="tonal" prepend-icon="mdi-format-list-checks" @click="openQueue">查看队列</VBtn>
          <VSpacer />
          <VBtn variant="tonal" prepend-icon="mdi-movie-search-outline" :disabled="!canReorganizeRecord(detailItem)" @click="openRecordConfirmation(detailItem, 'rescrape')">重新刮削</VBtn>
          <VBtn color="primary" variant="flat" prepend-icon="mdi-redo-variant" :disabled="!canReorganizeRecord(detailItem)" @click="openRecordConfirmation(detailItem, 'reprocess')">重新整理</VBtn>
        </VCardActions>
      </VCard>
    </VDialog>
    <VDialog v-model="confirmationOpen" max-width="min(45rem, calc(100vw - 2rem))" scrollable :fullscreen="smAndDown" :persistent="processing">
      <VCard class="organize-confirmation-card">
        <VCardItem class="py-2 pe-16">
          <template #prepend><VIcon icon="mdi-folder-move" class="me-2" /></template>
          <VCardTitle>{{ confirmationMode === 'rescrape' ? '重新刮削' : '手动整理' }}</VCardTitle>
          <VCardSubtitle>共 {{ confirmationItems.length }} 项</VCardSubtitle>
        </VCardItem>
        <DialogCloseBtn aria-label="关闭整理确认" :disabled="processing" @click="confirmationOpen = false" />
        <VDivider />
        <VCardText class="pa-6">
          <p class="text-body-1 mb-2">{{ confirmationMode === 'rescrape' ? '更新元数据、NFO 和图片，保留已有媒体信息、音轨和章节。' : '重新执行完整入库流程，按当前入库配置处理所选媒体。' }}</p>
          <p class="text-body-2 text-medium-emphasis mb-0">仅处理所选媒体文件，剧集只处理所选单集。同一媒体的多条记录合并为一次{{ confirmationMode === 'rescrape' ? '刮削' : '整理' }}。</p>
        </VCardText>
        <VDivider /><VCardActions class="app-dialog-actions"><VBtn color="success" variant="tonal" prepend-icon="mdi-plus" :loading="processing" data-testid="organize-confirm" @click="reorganize">加入队列</VBtn><VSpacer /><VBtn color="secondary" variant="tonal" :disabled="processing" @click="confirmationOpen = false">取消</VBtn></VCardActions>
      </VCard>
    </VDialog>
    <VBottomSheet v-model="deleteOpen" inset :persistent="processing">
      <VCard class="text-center">
        <DialogCloseBtn aria-label="关闭删除确认" :disabled="processing" @click="deleteOpen = false" />
        <VCardTitle class="pe-10">确认删除 {{ deleteItems.length }} 条记录？</VCardTitle>
        <div class="d-flex flex-column flex-lg-row justify-center my-3">
          <VBtn color="primary" class="mb-2 mx-2" :loading="processing" data-testid="organize-delete-confirm" @click="deleteHistory">仅删除整理记录</VBtn>
        </div>
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
import { createReusableTemplate, formatTimeAgoIntl, useElementBounding, useIntersectionObserver, useNow } from '@vueuse/core';
import { useRangeSelection } from '@/composables/useRangeSelection';
import { useContextSearch } from '@/composables/useContextSearch';
import { useDisplay } from 'vuetify';
import { VDataTableVirtual } from 'vuetify/components';
import axios from 'axios';
import { useMessage } from '@/composables/useFeedback';
import { queueReceiptMatches, useMediaOrganizeQueue } from '@/composables/useMediaOrganizeQueue';
import OrganizePoster from '@/components/OrganizePoster.vue';
import MediaOrganizeQueueDialog from '@/components/MediaOrganizeQueueDialog.vue';
import DialogCloseBtn from '@/components/theme/DialogCloseBtn.vue';

const [DefineRecord, RecordCard] = createReusableTemplate();
const mobileBatch = ref(false);
const toggleMobileBatch = () => {
  mobileBatch.value = !mobileBatch.value;
  if (!mobileBatch.value) selectedItems.value = [];
};
const message = useMessage();
const { smAndDown, height: viewportHeight, width: viewportWidth } = useDisplay();
const items = ref([]), search = ref(''), appliedSearch = ref(''), statusFilter = ref('all'), page = ref(1), total = ref(null), searchComposing = ref(false);
const statusOptions = [{ title: '全部状态', value: 'all' }, { title: '成功', value: 'success' }, { title: '失败', value: 'failed' }, { title: '有提醒', value: 'warning' }, { title: '正在处理', value: 'processing' }];
const pageSizeOptions = [25, 50, 100, 500, 1000];
const pageSize = ref(50);
const mobilePage = ref(1), mobileEnd = ref(false), moreElement = ref(null);
const expandedPath = ref('');
const grouped = ref(false), sortKey = ref(''), descending = ref(false), openGroups = ref(new Set());
const sortOptions = [
  { title: '默认顺序', value: '' }, { title: '媒体标题', value: 'name' }, { title: '文件路径', value: 'source_path' },
  { title: '整理时间', value: 'created_at' }, { title: '处理状态', value: 'status' },
];
const sortedItems = computed(() => {
  if (!sortKey.value || smAndDown.value) return items.value;
  const value = item => sortKey.value === 'name' ? itemTitle(item) : String(item[sortKey.value] || '');
  return [...items.value].sort((a, b) => value(a).localeCompare(value(b), 'zh-CN', { numeric: true }) * (descending.value ? -1 : 1));
});
const desktopRows = computed(() => {
  if (!grouped.value) return sortedItems.value;
  const groups = new Map();
  for (const item of sortedItems.value) {
    const title = itemTitle(item);
    if (!groups.has(title)) groups.set(title, []);
    groups.get(title).push(item);
  }
  return [...groups].flatMap(([title, members]) => [
    { id: 'group:' + title, title, members }, ...(openGroups.value.has(title) ? members : []),
  ]);
});
const toggleGroup = title => {
  if (openGroups.value.has(title)) openGroups.value.delete(title);
  else openGroups.value.add(title);
};
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
const detailOpen = ref(false), detailRecord = ref(null);
const detailItem = computed(() => items.value.find(item => item.id === detailRecord.value?.id) || detailRecord.value);
const openDetails = item => { detailRecord.value = { ...item }; detailOpen.value = true; };
const loadingItems = ref(false), itemsError = ref(''), processing = ref(false), feedback = ref(null);
let active = true, itemSequence = 0, searchTimer;
const loadedQuery = ref('');
const queryKey = () => JSON.stringify([page.value, pageSize.value, appliedSearch.value, statusFilter.value]);
const selectedIds = computed(() => selectedItems.value.map(item => item.id));
const selectedPageItems = computed(() => items.value.filter(item => selectedIds.value.includes(item.id)));
const canReorganize = computed(() => canSelectItems() && selectedPageItems.value.length <= 1000 && selectedPageItems.value.length > 0 && selectedPageItems.value.every(isReadyRecord));
const canDelete = computed(() => canSelectItems() && selectedPageItems.value.length <= 1000 && selectedPageItems.value.length > 0 && selectedPageItems.value.every(isDeletableRecord));
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
const mediaFileName = item => item.file_name?.replace(/\.strm$/i, '') || '';
const itemTitle = item => (item.item_type === 'Episode' && item.series_name) || (item.name === item.file_name ? mediaFileName(item) : item.name) || '未知媒体';
const episodeLabel = item => {
  if (item.item_type !== 'Episode') return '';
  const season = Number.isInteger(item.season_number) ? `S${String(item.season_number).padStart(2, '0')}` : '';
  const episode = Number.isInteger(item.episode_number) ? `E${String(item.episode_number).padStart(2, '0')}` : '';
  return [[season, episode].join(''), item.name !== item.series_name ? item.name : ''].filter(Boolean).join(' · ');
};
const statusPresentation = item => ({ success: { title: '成功', color: 'success' }, warning: { title: '成功', color: 'success' }, failed: { title: '失败', color: 'error' }, processing: { title: '正在处理', color: 'primary' } }[item.status] || { title: '未知', color: 'secondary' });
const formatDate = value => {
  if (!value) return '—';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '—' : date.toLocaleString('zh-CN', { hour12: false });
};
const now = useNow({ interval: 60_000 });
const recordDate = value => {
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return '';
  if (smAndDown.value) return date.toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hour12: false });
  return formatTimeAgoIntl(date, { locale: 'zh-CN', insertSpace: false }, now.value);
};

const loadItems = async (append = false) => {
  const more = append === true && smAndDown.value;
  if (more && (loadingItems.value || mobileEnd.value || processing.value || loadedQuery.value !== queryKey())) return;
  const requestPage = smAndDown.value ? (more ? mobilePage.value + 1 : 1) : page.value;
  const size = smAndDown.value ? 25 : pageSize.value;
  const sequence = ++itemSequence, targetQuery = queryKey();
  loadingItems.value = true;
  try {
    const { data } = await axios.get('/api/media_organize/history', {
      params: { page: requestPage, page_size: size, search: appliedSearch.value, status: statusFilter.value },
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
    if (more) {
      const known = new Set(items.value.map(row => row.id));
      items.value = [...items.value, ...data.items.filter(row => !known.has(row.id))];
    } else items.value = data.items;
    mobilePage.value = requestPage;
    mobileEnd.value = data.items.length < size || requestPage * size >= data.total;
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
const openQueue = () => { detailOpen.value = false; queueOpen.value = true; void queue.refresh(); };

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
const setSelection = (rows, enabled) => {
  const ids = new Set(rows.filter(isDeletableRecord).map(row => row.id));
  updateSelection(enabled ? [...selectedIds.value, ...ids] : selectedIds.value.filter(id => !ids.has(id)));
};
const range = useRangeSelection({
  items: () => smAndDown.value ? items.value : desktopRows.value.filter(row => !row.members),
  getKey: row => row.id, isSelected: row => selectedIds.value.includes(row.id), setSelected: setSelection,
});
watch([loadedQuery, grouped, sortKey, descending], () => { range.reset(); expandedPath.value = ''; });
watch(smAndDown, () => { page.value = 1; mobileBatch.value = false; selectedItems.value = []; loadItems(); });
useIntersectionObserver(moreElement, ([entry]) => { if (entry?.isIntersecting) void loadItems(true); });
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
  detailOpen.value = false;
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
      if (!selectedItems.value.length) mobileBatch.value = false;
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
.organize-record {
  display: grid;
  grid-template-columns: 36px 42px minmax(5rem, max-content) minmax(0, 1fr) minmax(5rem, 11rem) 36px;
  grid-template-areas: "check poster identity file status menu" "check poster episode file time time";
  align-items: center;
  gap: 4px 12px;
  min-block-size: 90px;
  padding: 10px 14px;
}
.organize-group { display: flex; align-items: center; gap: 12px; padding: 10px 14px; }
.organize-group > .v-input { flex: 0 0 36px; }
.organize-group .v-btn { height: auto; min-height: 40px; min-width: 0; white-space: normal; overflow-wrap: anywhere; }
.organize-check { grid-area: check; }
.record-poster { grid-area: poster; }
.organize-media-copy { grid-area: identity; display: grid; gap: 6px; min-inline-size: 0; overflow-wrap: anywhere; }
.organize-episode-label { grid-area: episode; min-inline-size: 0; white-space: nowrap; overflow-x: auto; scrollbar-width: thin; }
.organize-media-copy, .organize-episode-label { max-inline-size: min(35vw, 28rem); }
.media-title { color: inherit; font-size: 1.05rem; font-weight: 600; line-height: 1.35; }
.media-title:hover { text-decoration: underline; }
.organize-record small, .organize-record time { color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity)); line-height: 1.4; }
.organize-record-file { grid-area: file; display: grid; gap: 4px; min-inline-size: 0; overflow-wrap: anywhere; }
.organize-file-name { font-size: 0.9rem; line-height: 1.5; font-weight: 500; word-break: keep-all; overflow-wrap: anywhere; }
.record-status { grid-area: status; justify-self: start; }
.record-menu { grid-area: menu; justify-self: end; }
.organize-record time { grid-area: time; justify-self: end; font-size: 0.8rem; }
.organize-record--selected { outline: 1px solid rgba(var(--v-theme-primary), 0.55); outline-offset: -1px; }
.organize-items-desktop { background: transparent; }
.organize-items-desktop :deep(.v-table__wrapper) { scrollbar-gutter: stable; }
.media-organize-page :deep(.v-card-title), .media-organize-page :deep(.v-card-subtitle) { white-space: normal; overflow-wrap: anywhere; }
.history-message { white-space: pre-wrap; overflow-wrap: anywhere; line-height: 1.5; }
.organize-detail-content { display: flex; flex-direction: column; gap: 1rem; min-inline-size: 0; overflow-wrap: anywhere; }
.organize-detail-content > * { flex: 0 0 auto; }
.media-organize-page :deep(.v-data-table__td) { padding-block: 12px; }
.media-organize-page :deep(.v-alert-title), .media-organize-page :deep(.v-chip__content) { white-space: normal; overflow-wrap: anywhere; }
.media-organize-page :deep(.v-chip) { height: auto; min-height: 26px; padding-block: 4px; }
.history-cards { min-width: 0; }
.history-card {
  min-width: 0;
  grid-template-columns: 56px minmax(0, 1fr) auto 36px;
  grid-template-areas: "poster identity status menu" "poster identity time time" "episode episode episode episode" "file file file file";
  align-items: start;
  gap: 8px;
}
.history-card .organize-media-copy, .history-card .organize-episode-label { max-inline-size: none; }
.history-card .organize-check { grid-area: menu; }
.history-card time { justify-self: start; }
.history-card .organize-record-file { border-block-start: 1px solid rgba(var(--v-theme-on-surface), 0.12); padding-block-start: 10px; }
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
