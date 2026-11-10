<template>
  <div ref="mediaPage" class="media-center-page">
    <div class="media-center-controls-slot" :style="{ height: `${controlsHeight}px` }">
    <div ref="stickyControls" class="media-center-sticky-controls app-surface" :style="controlsStyle">
      <header class="d-flex align-center justify-space-between ga-3">
        <div class="d-flex align-center ga-3 min-width-0">
          <VBtn v-if="libraryId" icon="mdi-arrow-left" variant="text" color="default"
            aria-label="返回仪表盘" title="返回我的媒体库" @click="goToLibraries" />
          <MpPageTitle :title="libraryId ? (library?.name || '媒体库') : '我的媒体库'" />
          <VChip v-if="libraryId && library" size="small" variant="tonal" :aria-label="`媒体数量 ${total}`">{{ total.toLocaleString('zh-CN') }}</VChip>
        </div>
        <VBtn icon="mdi-refresh" variant="text" color="default" aria-label="刷新" title="刷新"
          :loading="loading" @click="refresh" />
      </header>
    </div>
    </div>

    <VAlert v-if="error" type="error" variant="tonal" closable class="my-4" @click:close="error = ''">{{ error }}</VAlert>
    <VAlert v-if="navigationNotice" type="info" variant="tonal" closable class="my-4" @click:close="navigationNotice = ''">{{ navigationNotice }}</VAlert>

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
                  :class="{ 'app-hover-lift-card--hovering': isHovering, 'media-card--image-loaded': posterIsLoaded(itemsByIndex[index - 1]) }"
                  :data-glass-optical-mode="posterIsLoaded(itemsByIndex[index - 1]) ? 'excluded' : undefined"
                  :aria-label="`编辑 ${itemsByIndex[index - 1].Name}`" @click="openMedia(itemsByIndex[index - 1].Id)">
                  <VImg :src="itemImage(itemsByIndex[index - 1]) || undefined" :aspect-ratio="2 / 3" cover
                    class="media-poster-wrap" @load="markPosterLoaded" @error="markPosterFailed">
                    <template #placeholder><div class="poster-fallback"><VProgressCircular v-if="itemImage(itemsByIndex[index - 1])" indeterminate size="24" width="2" /></div></template>
                    <template #error><div class="poster-fallback"><VIcon :icon="libraryIcon(itemsByIndex[index - 1].Type)" size="36" /></div></template>
                    <div v-if="!itemImage(itemsByIndex[index - 1])" class="poster-fallback"><VIcon :icon="libraryIcon(itemsByIndex[index - 1].Type)" size="36" /></div>
                    <VChip v-if="itemsByIndex[index - 1].CommunityRating" class="media-rating" color="#9333ea" variant="elevated" size="small" prepend-icon="mdi-star" :aria-label="`Emby 社区评分 ${Number(itemsByIndex[index - 1].CommunityRating).toFixed(1)}`">
                      {{ Number(itemsByIndex[index - 1].CommunityRating).toFixed(1) }}
                    </VChip>
                    <div v-if="isHovering" class="media-card-detail">
                      <span class="media-detail-year">{{ itemsByIndex[index - 1].ProductionYear }}</span>
                      <h2 class="media-detail-title my-2" :title="itemsByIndex[index - 1].Name">{{ itemsByIndex[index - 1].Name }}</h2>
                      <p v-if="itemsByIndex[index - 1].Overview" class="media-overview mb-3">{{ itemsByIndex[index - 1].Overview }}</p>
                      <span class="d-flex align-center ga-2 text-caption"><VIcon icon="mdi-pencil-outline" size="18" />打开媒体编辑</span>
                    </div>
                  </VImg>
                  <VCardText class="media-card-caption">
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
        <nav class="alphabet-nav" :style="alphabetStyle" aria-label="按首字母定位">
          <VBtn v-for="letter in letters" :key="letter" :variant="selectedLetter === letter ? 'tonal' : 'text'"
            :color="selectedLetter === letter ? 'primary' : 'default'" size="x-small"
            :title="letter === '#' ? '返回开头' : `定位首字母 ${letter}`" :aria-label="letter === '#' ? '返回开头' : `定位首字母 ${letter}`" :aria-pressed="selectedLetter === letter"
            @click="selectLetter(letter)">{{ letter }}</VBtn>
        </nav>
      </div>
    </div>
    <div v-if="loading && !hasContent" class="media-center-loading"><VProgressCircular indeterminate size="40" width="3" /></div>
  </div>
</template>

<script setup>
import MpPageTitle from './MpPageTitle.vue';
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import axios from 'axios';
import { libraryViewport, visibleLibraryPageStarts } from '../utils/mediaLibraryNavigation.js';
import { useContextSearch } from '@/composables/useContextSearch';

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
const controlsStyle = ref({});
const controlsHeight = ref(76);
const letters = ['#', ...'ABCDEFGHIJKLMNOPQRSTUVWXYZ'];
const loading = ref(false);
const error = ref('');
const navigationNotice = ref('');
let requestVersion = 0;
let scrollContainer = null;
let scrollContainers = [];
let scrollTargets = [];
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
const markPosterFailed = url => { if (url) loadedPosterUrls.value.delete(absolutePosterUrl(url)); };
const posterIsLoaded = item => loadedPosterUrls.value.has(absolutePosterUrl(itemImage(item)));


const loadLibraries = async () => {
  loading.value = true;
  error.value = '';
  navigationNotice.value = '';
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
  if (reset) navigationNotice.value = '';
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
      if (response.data.anchor_found === false) {
        navigationNotice.value = `没有可定位到 ${selectedLetter.value} 的媒体，已返回媒体库开头`;
        selectedLetter.value = '#';
      }
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
      if (reset && !error.value) await nextTick(() => loadVisiblePages());
    }
  }
};

const openLibrary = (id) => router.push({ name: 'MediaCenterLibrary', params: { libraryId: id } });
const goToLibraries = () => router.push({ name: 'Dashboard' });
const openMedia = (id) => router.push({ name: 'MediaEditPage', params: { itemId: id } });
const navbarBottom = () => Math.ceil(document.querySelector('.layout-navbar')?.getBoundingClientRect().bottom || 64);
const contentTopGap = () => parseFloat(getComputedStyle(mediaPage.value?.closest('.layout-page-content') || document.body).paddingTop) || 0;
const updateFixedControls = () => {
  if (!mediaPage.value || !stickyControls.value) return;
  const bounds = mediaPage.value.getBoundingClientRect();
  const top = navbarBottom() + contentTopGap();
  controlsStyle.value = { top: `${top}px`, left: `${bounds.left}px`, width: `${bounds.width - 4}px` };
  controlsHeight.value = stickyControls.value.offsetHeight;
  alphabetStyle.value = {
    top: `${top + controlsHeight.value + 16}px`,
    right: `${Math.max(window.innerWidth - bounds.right + 4, 4)}px`,
    maxHeight: `${Math.max(window.innerHeight - top - controlsHeight.value - 48, 32)}px`,
  };
};
const findScrollContainer = () => scrollContainer || document.scrollingElement;
const resetBrowserPosition = () => {
  const container = findScrollContainer();
  const top = container === document.scrollingElement
    ? Math.max((mediaPage.value?.getBoundingClientRect().top || 0) + container.scrollTop - navbarBottom() - contentTopGap(), 0)
    : 0;
  container?.scrollTo({ top, behavior: 'auto' });
};
const searchItems = () => {
  clearTimeout(searchTimer);
  selectedLetter.value = '#';
  resetBrowserPosition();
  loadPage(0, { reset: true });
};
const onSearchInput = (value) => {
  search.value = String(value ?? '');
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => searchItems(), 320);
};
useContextSearch({ label: '搜索当前媒体库', query: search, submit: searchItems,
  onInput: onSearchInput, clear: searchItems, loading });
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
  const container = findScrollContainer();
  const element = mediaGrid.value?.querySelector(`[data-media-index="${index}"]`);
  if (!container || !element) return;
  const viewport = libraryViewport(container, document.scrollingElement, window.innerHeight);
  const top = element.getBoundingClientRect().top + viewport.scrollTop
    - (stickyControls.value?.getBoundingClientRect().bottom || viewport.top) - 16;
  container.scrollTo({ top: Math.max(top, 0), behavior: 'auto' });
};
const loadVisiblePages = (event) => {
  const container = event?.currentTarget === window ? document.scrollingElement : (event?.currentTarget || scrollContainer);
  if (!container || loading.value) return;
  scrollContainer = container;
  const grid = mediaGrid.value;
  if (!grid || !total.value) return;
  const firstSlot = grid.firstElementChild;
  const columns = Math.max(getComputedStyle(grid).gridTemplateColumns.split(' ').length, 1);
  const rowHeight = firstSlot?.getBoundingClientRect().height
    ? firstSlot.getBoundingClientRect().height + parseFloat(getComputedStyle(grid).rowGap || '0')
    : 270;
  const viewport = libraryViewport(container, document.scrollingElement, window.innerHeight);
  const gridTop = grid.getBoundingClientRect().top - viewport.top + viewport.scrollTop;
  const firstRow = Math.max(Math.floor(Math.max(viewport.scrollTop - gridTop, 0) / rowHeight) - 1, 0);
  const lastRow = Math.ceil((viewport.scrollTop + viewport.height - gridTop) / rowHeight) + 1;
  const firstIndex = firstRow * columns;
  const lastIndex = Math.min(lastRow * columns, total.value - 1);
  for (const pageStart of visibleLibraryPageStarts(firstIndex, lastIndex, pageSize)) {
    if (!loadedPageStarts.has(pageStart) && !pendingScrollLoads.has(pageStart)) {
      pendingScrollLoads.add(pageStart);
      const version = requestVersion;
      loadPage(pageStart, { target: pageStart < targetIndex.value ? 'before' : 'after' })
        .finally(() => { if (version === requestVersion) pendingScrollLoads.delete(pageStart); });
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
  scrollTargets.forEach((container) => container.removeEventListener('scroll', loadVisiblePages));
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
  if (document.scrollingElement && !scrollContainers.includes(document.scrollingElement)) {
    scrollContainers.push(document.scrollingElement);
  }
  scrollContainer = scrollContainers.find((container) => container !== document.scrollingElement && container.scrollHeight > container.clientHeight)
    || document.scrollingElement;
  scrollTargets = scrollContainers.map(container => container === document.scrollingElement ? window : container);
  scrollTargets.forEach((container) => container.addEventListener('scroll', loadVisiblePages, { passive: true }));
  gridResizeObserver = new ResizeObserver(() => {
    updateFixedControls();
    loadVisiblePages();
  });
  gridResizeObserver.observe(sentinel);
  gridResizeObserver.observe(mediaPage.value);
  gridResizeObserver.observe(stickyControls.value);
  const navbar = document.querySelector('.layout-navbar');
  if (navbar) gridResizeObserver.observe(navbar);
};

watch(mediaGrid, setupScrollListeners, { flush: 'post' });
onMounted(() => {
  updateFixedControls();
  setupScrollListeners();
  window.addEventListener('scroll', updateFixedControls, { passive: true });
  window.addEventListener('resize', setupScrollListeners);
});

onBeforeUnmount(() => {
  requestVersion += 1;
  clearTimeout(searchTimer);
  gridResizeObserver?.disconnect();
  scrollTargets.forEach((container) => container.removeEventListener('scroll', loadVisiblePages));
  window.removeEventListener('scroll', updateFixedControls);
  window.removeEventListener('resize', setupScrollListeners);
});
</script>

<style scoped>
.media-center-page { position:relative; min-height:0; padding:0 4px 24px 0; }
.media-center-sticky-controls { position:fixed; z-index:4; padding:12px 16px 16px; background:rgb(var(--v-theme-surface)); border-radius:var(--app-theme-surface-radius); }
.min-width-0 { min-width:0; }
.library-grid { display:grid; grid-template-columns:repeat(auto-fill,minmax(min(100%,220px),1fr)); gap:24px; margin-top:20px; }
.library-card { min-width:0; width:100%; text-align:left; cursor:pointer; }
.library-art { background:rgba(var(--v-theme-on-surface),.06); }
.library-count { position:absolute; top:12px; right:12px; }
.media-browser { position:relative; padding-right:50px; }
.media-grid { display:grid; grid-template-columns:repeat(auto-fill,minmax(min(100%,144px),1fr)); gap:16px; margin-top:16px; }
.media-card-hover-area { min-width:0; height:100%; }
.media-card { min-width:0; width:100%; height:100%; text-align:left; cursor:pointer; }
.media-card-caption { padding:12px max(12px, var(--app-theme-surface-radius)) max(12px, var(--app-theme-surface-radius)); }
.media-poster-wrap { position:relative; background:rgba(var(--v-theme-on-surface),.06); }
.media-card-placeholder::before { content:''; display:block; aspect-ratio:2/3; }
.media-card-placeholder::after { content:''; display:block; height:calc(76px + max(12px, var(--app-theme-surface-radius))); }
.poster-fallback { display:flex; align-items:center; justify-content:center; height:100%; width:100%; color:rgba(var(--v-theme-on-surface),.45); }
.media-card-detail { position:absolute; inset:0; display:flex; flex-direction:column; justify-content:flex-end; padding:12px max(12px, var(--app-theme-surface-radius)); color:#fff; text-align:left; background:linear-gradient(rgba(45,55,72,.7),rgba(45,55,72,.9)); }
.media-detail-year { font-size:14px; font-weight:600; line-height:20px; }
.media-detail-title { display:-webkit-box; -webkit-box-orient:vertical; -webkit-line-clamp:2; overflow:hidden; overflow-wrap:anywhere; flex-shrink:0; color:#fff; font-size:18px; font-weight:700; line-height:20px; }
.media-overview { display:-webkit-box; -webkit-line-clamp:3; -webkit-box-orient:vertical; overflow:hidden; font-size:14px; line-height:16px; }
.media-rating { position:absolute; z-index:1; top:8px; right:8px; background-color:rgba(147,51,234,.8) !important; background-image:linear-gradient(rgba(0,0,0,.2),rgba(0,0,0,.2)) !important; color:#fff !important; font-weight:700; }
.media-rating :deep(.v-chip__content), .media-rating :deep(.v-chip__prepend) { color:#fff !important; }
.media-card-title { display:-webkit-box; -webkit-box-orient:vertical; -webkit-line-clamp:2; min-height:40px; overflow:hidden; overflow-wrap:anywhere; font-size:14px; font-weight:600; line-height:20px; text-align:center; }
.media-card-meta { display:flex; min-height:18px; justify-content:center; gap:8px; margin-top:6px; color:rgba(var(--v-theme-on-surface),var(--v-medium-emphasis-opacity)); font-size:12px; line-height:18px; }
.media-empty { margin:50px 0; }
.alphabet-nav { position:fixed; z-index:3; display:flex; flex-direction:column; gap:2px; padding:4px 2px; overflow-y:auto; border:1px solid rgba(var(--v-border-color),var(--v-border-opacity)); border-radius:12px; background:rgb(var(--v-theme-surface)); scrollbar-width:none; }
.alphabet-nav::-webkit-scrollbar { display:none; }
.alphabet-nav :deep(.v-btn[aria-pressed='true'] .v-btn__content) { color: rgba(var(--v-theme-on-surface), var(--v-high-emphasis-opacity)); }
.alphabet-nav :deep(.v-btn) { flex:0 0 22px; min-width:28px; width:28px; height:22px; padding:0; font-size:12px; }
.media-center-loading { position:absolute; inset:100px 0 0; display:grid; place-items:center; pointer-events:none; }
@media (max-width:600px) { .library-grid { grid-template-columns:repeat(2,minmax(0,1fr)); gap:18px 12px; } .media-grid { grid-template-columns:repeat(2,minmax(0,1fr)); gap:20px 12px; } .media-browser { padding-right:44px; } .media-center-sticky-controls { padding-inline:12px; } }
</style>
