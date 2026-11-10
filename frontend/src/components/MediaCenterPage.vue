<template>
  <div ref="mediaPage" class="media-center-page">
    <div ref="stickyControls" class="media-center-sticky-controls">
      <header class="d-flex align-center justify-space-between ga-3 mb-4">
        <div class="d-flex align-center ga-3 min-width-0">
          <VBtn v-if="libraryId" icon="mdi-arrow-left" variant="text" color="default"
            aria-label="返回仪表盘" title="返回我的媒体库" @click="goToLibraries" />
          <div class="min-width-0">
            <h1 class="text-h5 font-weight-bold text-truncate">{{ libraryId ? (library?.name || '媒体库') : '我的媒体库' }}</h1>
            <p class="text-body-2 text-medium-emphasis mt-1">{{ libraryId ? `浏览${mediaTypeLabel} · ${total} 项媒体` : '浏览和管理 Emby 媒体库' }}</p>
          </div>
        </div>
        <VBtn icon="mdi-refresh" variant="text" color="default" aria-label="刷新" title="刷新"
          :loading="loading" @click="refresh" />
      </header>
      <VTextField v-if="libraryId" v-model="search" class="library-search"
        :placeholder="`搜索当前${mediaTypeLabel}`" label="搜索媒体" prepend-inner-icon="mdi-magnify"
        clearable density="compact" hide-details
        @update:model-value="onSearchInput" @keyup.enter="searchItems" @click:clear="searchItems">
        <template #append-inner>
          <VBtn icon="mdi-magnify" variant="text" size="small" aria-label="搜索" title="搜索"
            @click.stop="searchItems" />
        </template>
      </VTextField>
    </div>

    <VAlert v-if="error" type="error" variant="tonal" closable class="my-4" @click:close="error = ''">{{ error }}</VAlert>

    <div v-if="!libraryId" class="library-grid" v-show="!loading || libraries.length">
      <VCard v-for="item in libraries" :key="item.id" tag="button" type="button"
        class="library-card app-hover-lift-card" :aria-label="`打开${item.name}`" @click="openLibrary(item.id)">
        <VImg :src="libraryImage(item) || undefined" :aspect-ratio="16 / 9" cover class="library-art"
          gradient="to bottom, rgba(0,0,0,.02), rgba(0,0,0,.55)">
          <template #error><div class="poster-fallback"><VIcon :icon="libraryIcon(item.collection_type)" size="54" /></div></template>
          <div v-if="!libraryImage(item)" class="poster-fallback"><VIcon :icon="libraryIcon(item.collection_type)" size="54" /></div>
          <VChip class="library-count" color="success" variant="flat" size="small">{{ item.item_count }}</VChip>
        </VImg>
        <VCardTitle class="text-body-1 font-weight-bold text-center">{{ item.name }}</VCardTitle>
        <VCardSubtitle class="pb-4 text-center">{{ libraryType(item.collection_type) }}</VCardSubtitle>
      </VCard>
    </div>

    <div v-else class="library-content">
      <div class="media-browser">
        <div v-if="total" ref="mediaGrid" class="media-grid">
          <template v-for="index in total" :key="index - 1">
            <VHover v-if="itemsByIndex[index - 1]" v-slot="{ isHovering, props: hoverProps }">
              <div v-bind="hoverProps" class="media-card-hover-area" :data-media-index="index - 1">
                <VCard tag="button" type="button" :ripple="false" class="media-card app-hover-lift-card"
                  :class="{ 'app-hover-lift-card--hovering': isHovering }"
                  :data-glass-optical-mode="posterIsLoaded(itemsByIndex[index - 1]) ? 'excluded' : undefined"
                  :aria-label="`编辑 ${itemsByIndex[index - 1].Name}`" @click="openMedia(itemsByIndex[index - 1].Id)">
                  <VImg :src="itemImage(itemsByIndex[index - 1]) || undefined" :aspect-ratio="2 / 3" cover
                    class="media-poster-wrap" @load="markPosterLoaded">
                    <template #placeholder><div class="poster-fallback"><VProgressCircular v-if="itemImage(itemsByIndex[index - 1])" indeterminate size="24" width="2" /></div></template>
                    <template #error><div class="poster-fallback"><VIcon :icon="libraryIcon(itemsByIndex[index - 1].Type)" size="36" /></div></template>
                    <div v-if="!itemImage(itemsByIndex[index - 1])" class="poster-fallback"><VIcon :icon="libraryIcon(itemsByIndex[index - 1].Type)" size="36" /></div>
                    <VChip v-if="itemsByIndex[index - 1].CommunityRating" class="media-rating" color="warning" variant="flat" size="x-small" prepend-icon="mdi-star">
                      {{ Number(itemsByIndex[index - 1].CommunityRating).toFixed(1) }}
                    </VChip>
                    <div v-if="isHovering" class="media-card-detail">
                      <span class="text-caption">{{ itemsByIndex[index - 1].ProductionYear }}</span>
                      <h2 class="text-body-1 font-weight-bold my-2">{{ itemsByIndex[index - 1].Name }}</h2>
                      <p v-if="itemsByIndex[index - 1].Overview" class="media-overview text-caption mb-3">{{ itemsByIndex[index - 1].Overview }}</p>
                      <span class="d-flex align-center ga-2 text-caption"><VIcon icon="mdi-pencil-outline" size="18" />打开媒体编辑</span>
                    </div>
                  </VImg>
                  <VCardText class="pa-3">
                    <div class="media-card-title" :title="itemsByIndex[index - 1].Name">{{ itemsByIndex[index - 1].Name }}</div>
                    <div class="media-card-meta">
                      <span v-if="itemsByIndex[index - 1].ProductionYear">{{ itemsByIndex[index - 1].ProductionYear }}</span>
                      <span v-if="itemsByIndex[index - 1].Type === 'Series' && itemsByIndex[index - 1].ChildCount">{{ itemsByIndex[index - 1].ChildCount }} 季</span>
                    </div>
                  </VCardText>
                </VCard>
              </div>
            </VHover>
            <div v-else class="media-card-placeholder" aria-hidden="true"></div>
          </template>
        </div>
        <VCard v-else-if="!loading" variant="tonal" class="media-empty pa-8 text-center">
          <VIcon icon="mdi-movie-search-outline" size="54" color="secondary" />
          <p class="text-body-1 text-medium-emphasis mt-4">{{ emptyDescription }}</p>
        </VCard>
        <nav class="alphabet-nav" :style="alphabetStyle" aria-label="按首字母筛选">
          <VBtn v-for="letter in letters" :key="letter" :variant="selectedLetter === letter ? 'tonal' : 'text'"
            :color="selectedLetter === letter ? 'primary' : 'default'" size="x-small"
            :title="letter === '#' ? '全部' : `按首字母 ${letter} 筛选`" :aria-pressed="selectedLetter === letter"
            @click="selectLetter(letter)">{{ letter }}</VBtn>
        </nav>
      </div>
    </div>
    <div v-if="loading && !hasContent" class="media-center-loading"><VProgressCircular indeterminate size="40" width="3" /></div>
  </div>
</template>

<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import axios from 'axios';

const route = useRoute();
const router = useRouter();
const libraries = ref([]);
const library = ref(null);
const total = ref(0);
const pageSize = 30;
const search = ref('');
const selectedLetter = ref('#');
const mediaPage = ref(null);
const stickyControls = ref(null);
const mediaGrid = ref(null);
const alphabetStyle = ref({});
const letters = ['#', ...'ABCDEFGHIJKLMNOPQRSTUVWXYZ'];
const loading = ref(false);
const error = ref('');
let requestVersion = 0;
let scrollContainer = null;
let scrollContainers = [];
let searchTimer = null;
let gridResizeObserver;
const itemsByIndex = ref({});
const pendingPageStarts = new Set();
const loadedPageStarts = new Set();
const pendingScrollLoads = new Set();
const targetIndex = ref(0);
const loadingDirection = ref('');
const hasLoadedItems = computed(() => Object.keys(itemsByIndex.value).length > 0);
const hasContent = computed(() => (libraryId.value ? hasLoadedItems.value : libraries.value.length > 0));

const libraryId = computed(() => route.params.libraryId || '');

const libraryType = (type) => ({
  movie: '电影',
  movies: '电影',
  tvshows: '电视剧',
  mixed: '电影 / 电视剧',
  music: '音乐',
}[String(type || '').toLowerCase()] || '媒体库');

const libraryIcon = (type) => {
  const normalized = String(type || '').toLowerCase();
  return normalized === 'music' || normalized === 'musicalbum'
    ? 'mdi-music-note'
    : normalized === 'tvshows' || normalized === 'series'
      ? 'mdi-television'
      : 'mdi-movie-open-outline';
};

const mediaTypeLabel = computed(() => library.value
  ? libraryType(library.value.collection_type)
  : '媒体');
const emptyDescription = computed(() => '当前媒体库没有匹配的' + mediaTypeLabel.value);
const libraryImage = (item) => item.image_tag
  ? `/image_proxy/Items/${item.id}/Images/Primary?tag=${encodeURIComponent(item.image_tag)}&quality=90`
  : '';

const itemImage = (item) => {
  const tag = item.ImageTags?.Primary;
  return tag ? `/image_proxy/Items/${item.Id}/Images/Primary?tag=${encodeURIComponent(tag)}&quality=90` : '';
};

const loadedPosterUrls = ref(new Set());
const absolutePosterUrl = url => url ? new URL(url, window.location.href).href : '';
const markPosterLoaded = url => { if (url) loadedPosterUrls.value.add(absolutePosterUrl(url)); };
const posterIsLoaded = item => loadedPosterUrls.value.has(absolutePosterUrl(itemImage(item)));


const loadLibraries = async () => {
  loading.value = true;
  error.value = '';
  try {
    const response = await axios.get('/api/media_center/libraries');
    libraries.value = response.data.libraries || [];
  } catch (err) {
    error.value = err.response?.data?.error || '获取媒体库失败';
  } finally {
    loading.value = false;
  }
};

const loadPage = async (startIndex, { reset = false, target = null } = {}) => {
  if (!libraryId.value) return;
  const normalizedStart = Math.max(Number(startIndex) || 0, 0);
  if (!reset && (pendingPageStarts.has(normalizedStart) || normalizedStart >= total.value)) return;
  const requestId = reset ? ++requestVersion : requestVersion;
  if (!reset) pendingPageStarts.add(normalizedStart);
  if (reset) {
    itemsByIndex.value = {};
    total.value = 0;
    loadedPageStarts.clear();
    pendingPageStarts.clear();
    pendingScrollLoads.clear();
  }
  loading.value = true;
  loadingDirection.value = target === 'before' ? 'before' : target === 'after' ? 'after' : 'reset';
  error.value = '';
  try {
    const response = await axios.get(`/api/media_center/libraries/${encodeURIComponent(libraryId.value)}/items`, {
      params: {
        page_size: pageSize,
        search: String(search.value || '').trim(),
        ...(reset && selectedLetter.value !== '#'
          ? { letter: selectedLetter.value === '#' ? '' : selectedLetter.value }
          : { start_index: normalizedStart }),
      },
    });
    if (requestId !== requestVersion) return;
    library.value = response.data.library;
    const pageItems = response.data.items || [];
    const responseStart = Number(response.data.start_index || 0);
    if (reset) {
      total.value = Math.max(Number(response.data.total || 0), responseStart + pageItems.length);
      targetIndex.value = Number(response.data.anchor_index ?? responseStart);
      itemsByIndex.value = {};
    }
    const nextItems = { ...itemsByIndex.value };
    pageItems.forEach((item, offset) => { nextItems[responseStart + offset] = item; });
    itemsByIndex.value = nextItems;
    loadedPageStarts.add(responseStart);
    pendingPageStarts.delete(normalizedStart);
    await nextTick();
    if (reset) scrollToIndex(targetIndex.value);
  } catch (err) {
    if (requestId !== requestVersion) return;
    error.value = err.response?.data?.error || '获取媒体库内容失败';
    if (reset) itemsByIndex.value = {};
    if (reset) total.value = 0;
  } finally {
    if (requestId === requestVersion) {
      pendingPageStarts.delete(normalizedStart);
      loading.value = false;
      loadingDirection.value = '';
    }
  }
};

const openLibrary = (id) => router.push({ name: 'MediaCenterLibrary', params: { libraryId: id } });
const goToLibraries = () => router.push({ name: 'Dashboard' });
const openMedia = (id) => router.push({ name: 'MediaEditPage', params: { itemId: id } });
const resetBrowserPosition = () => {
  mediaPage.value?.scrollTo({ top: 0, behavior: 'auto' });
};
const searchItems = () => {
  clearTimeout(searchTimer);
  resetBrowserPosition();
  loadPage(0, { reset: true });
};
const onSearchInput = (value) => {
  search.value = String(value ?? '');
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => searchItems(), 320);
};
const selectLetter = (letter) => {
  if (selectedLetter.value === letter) return;
  clearTimeout(searchTimer);
  selectedLetter.value = letter;
  resetBrowserPosition();
  loadPage(0, { reset: true });
};
const refresh = () => libraryId.value ? loadPage(0, { reset: true }) : loadLibraries();
const scrollToIndex = (index) => {
  if (!index) return resetBrowserPosition();
  const container = mediaPage.value;
  const element = mediaGrid.value?.querySelector(`[data-media-index="${index}"]`);
  if (!container || !element) return;
  const top = element.getBoundingClientRect().top - container.getBoundingClientRect().top
    + container.scrollTop - (stickyControls.value?.offsetHeight || 0) - 16;
  container.scrollTo({ top: Math.max(top, 0), behavior: 'auto' });
};
const loadVisiblePages = (event) => {
  const container = event?.currentTarget || scrollContainer;
  if (!container || loading.value) return;
  scrollContainer = container;
  const grid = mediaGrid.value;
  if (!grid || !total.value) return;
  const firstSlot = grid.firstElementChild;
  const columns = Math.max(getComputedStyle(grid).gridTemplateColumns.split(' ').length, 1);
  const rowHeight = firstSlot?.getBoundingClientRect().height
    ? firstSlot.getBoundingClientRect().height + parseFloat(getComputedStyle(grid).rowGap || '0')
    : 270;
  const containerRect = container.getBoundingClientRect();
  const gridTop = grid.getBoundingClientRect().top - containerRect.top + container.scrollTop;
  const firstRow = Math.max(Math.floor(Math.max(container.scrollTop - gridTop, 0) / rowHeight) - 1, 0);
  const lastRow = Math.ceil((container.scrollTop + container.clientHeight - gridTop) / rowHeight) + 1;
  const firstIndex = firstRow * columns;
  const lastIndex = Math.min(lastRow * columns, total.value - 1);
  for (let index = firstIndex; index <= lastIndex; index += pageSize) {
    const pageStart = Math.floor(index / pageSize) * pageSize;
    if (!loadedPageStarts.has(pageStart) && !pendingScrollLoads.has(pageStart)) {
      pendingScrollLoads.add(pageStart);
      loadPage(pageStart, { target: pageStart < targetIndex.value ? 'before' : 'after' })
        .finally(() => pendingScrollLoads.delete(pageStart));
    }
  }
};

watch(libraryId, () => {
  requestVersion += 1;
  clearTimeout(searchTimer);
  selectedLetter.value = '#';
  search.value = '';
  library.value = null;
  total.value = 0;
  itemsByIndex.value = {};
  loadedPageStarts.clear();
  pendingPageStarts.clear();
  pendingScrollLoads.clear();
  if (libraryId.value) loadPage(0, { reset: true });
  else loadLibraries();
}, { immediate: true });

const setupScrollListeners = () => {
  gridResizeObserver?.disconnect();
  scrollContainers.forEach((container) => container.removeEventListener('scroll', loadVisiblePages));
  scrollContainers = [];
  const sentinel = mediaGrid.value;
  if (!sentinel) return;
  let current = sentinel.parentElement;
  while (current && current !== document.body) {
    const style = window.getComputedStyle(current);
    if (['auto', 'scroll'].includes(style.overflowY)) {
      scrollContainers.push(current);
    }
    current = current.parentElement;
  }
  if (!scrollContainers.length && document.scrollingElement) {
    scrollContainers = [document.scrollingElement];
  }
  scrollContainer = scrollContainers.find((container) => container.scrollHeight > container.clientHeight)
    || scrollContainers[0]
    || document.scrollingElement;
  scrollContainers.forEach((container) => container.addEventListener('scroll', loadVisiblePages, { passive: true }));
  gridResizeObserver = new ResizeObserver(() => {
    const controlsHeight = stickyControls.value?.offsetHeight || 0;
    alphabetStyle.value = {
      top: `${controlsHeight + 16}px`,
      maxHeight: `${Math.max((mediaPage.value?.clientHeight || 0) - controlsHeight - 32, 32)}px`,
    };
    loadVisiblePages();
  });
  gridResizeObserver.observe(sentinel);
  gridResizeObserver.observe(mediaPage.value);
  gridResizeObserver.observe(stickyControls.value);
};

watch(mediaGrid, setupScrollListeners, { flush: 'post' });
onMounted(() => {
  document.querySelector('.page-content-inner-wrapper')?.classList.add('media-center-layout');
  setupScrollListeners();
});

onBeforeUnmount(() => {
  requestVersion += 1;
  clearTimeout(searchTimer);
  gridResizeObserver?.disconnect();
  scrollContainers.forEach((container) => container.removeEventListener('scroll', loadVisiblePages));
  document.querySelector('.page-content-inner-wrapper')?.classList.remove('media-center-layout');
});
</script>

<style scoped>
.media-center-page { position:relative; height:100%; min-height:0; overflow-y:auto; padding:0 4px 24px 0; }
:global(.page-content-inner-wrapper.media-center-layout) { height:100%; overflow:visible; }
.media-center-sticky-controls { position:sticky; z-index:4; top:0; padding:0 0 16px; background:rgba(var(--v-theme-background),.84); backdrop-filter:blur(12px); }
.min-width-0 { min-width:0; }
.library-grid { display:grid; grid-template-columns:repeat(auto-fill,minmax(min(100%,220px),1fr)); gap:24px; margin-top:20px; }
.library-card { min-width:0; width:100%; text-align:left; cursor:pointer; }
.library-art { background:rgba(var(--v-theme-on-surface),.06); }
.library-count { position:absolute; top:12px; right:12px; }
.library-search { width:min(480px,100%); }
.media-browser { position:relative; display:grid; grid-template-columns:minmax(0,1fr) 34px; gap:16px; align-items:start; }
.media-grid { display:grid; grid-template-columns:repeat(auto-fill,minmax(min(100%,148px),1fr)); gap:24px 18px; margin-top:16px; }
.media-card-hover-area { min-width:0; height:100%; }
.media-card { min-width:0; width:100%; height:100%; text-align:left; cursor:pointer; }
.media-poster-wrap { position:relative; background:rgba(var(--v-theme-on-surface),.06); }
.media-card-placeholder::before { content:''; display:block; aspect-ratio:2/3; }
.media-card-placeholder::after { content:''; display:block; height:72px; }
.poster-fallback { display:flex; align-items:center; justify-content:center; height:100%; width:100%; color:rgba(var(--v-theme-on-surface),.45); }
.media-card-detail { position:absolute; inset:0; display:flex; flex-direction:column; justify-content:flex-end; padding:12px; color:#fff; text-align:left; background:linear-gradient(rgba(45,55,72,.4),rgba(45,55,72,.9)); }
.media-overview { display:-webkit-box; -webkit-line-clamp:3; -webkit-box-orient:vertical; overflow:hidden; }
.media-rating { position:absolute; top:8px; right:8px; }
.media-card-title { overflow:hidden; font-size:14px; font-weight:600; line-height:20px; text-align:center; text-overflow:ellipsis; white-space:nowrap; }
.media-card-meta { display:flex; min-height:18px; justify-content:center; gap:8px; margin-top:6px; color:rgba(var(--v-theme-on-surface),.6); font-size:12px; line-height:18px; }
.media-empty { margin:50px 0; }
.alphabet-nav { position:sticky; z-index:3; top:0; grid-column:2; display:flex; flex-direction:column; gap:2px; margin-top:16px; padding:4px 2px; overflow-y:auto; border:1px solid rgba(var(--v-border-color),var(--v-border-opacity)); border-radius:12px; background:rgb(var(--v-theme-surface)); scrollbar-width:none; }
.alphabet-nav::-webkit-scrollbar { display:none; }
.alphabet-nav :deep(.v-btn) { flex:0 0 22px; min-width:28px; width:28px; height:22px; padding:0; font-size:12px; }
.media-center-loading { position:absolute; inset:100px 0 0; display:grid; place-items:center; pointer-events:none; }
@media (max-width:600px) { .library-grid { grid-template-columns:repeat(2,minmax(0,1fr)); gap:18px 12px; } .media-grid { grid-template-columns:repeat(2,minmax(0,1fr)); gap:20px 12px; } .media-browser { gap:10px; } }
</style>
