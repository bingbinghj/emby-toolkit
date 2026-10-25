<template>
  <div ref="mediaPage" class="mp-page media-center-page">
    <div ref="stickyControls" class="media-center-sticky-controls">
      <n-page-header
        class="mp-page-header"
        @back="libraryId ? goToLibraries() : undefined"
      >
        <template #title>{{ libraryId ? (library?.name || '媒体库') : '媒体中心' }}</template>
        <template #back v-if="libraryId">
          <n-button quaternary circle aria-label="返回媒体库" title="返回媒体库">
            <template #icon><n-icon><ArrowBackOutline /></n-icon></template>
          </n-button>
        </template>
        <template #extra>
          <n-button quaternary circle class="refresh-button" aria-label="刷新" title="刷新" @click="refresh" :loading="loading">
            <template #icon><n-icon><RefreshOutline /></n-icon></template>
          </n-button>
        </template>
      </n-page-header>

      <div v-if="libraryId" class="library-toolbar">
        <n-input
          v-model:value="search"
          clearable
          :placeholder="`搜索当前${mediaTypeLabel}`"
          class="library-search"
          @update:value="onSearchInput"
          @keyup.enter="searchItems"
          @clear="searchItems"
        >
          <template #prefix><n-icon><SearchOutline /></n-icon></template>
          <template #suffix>
            <n-button quaternary circle size="tiny" class="search-submit" aria-label="搜索" title="搜索" @click.stop="searchItems">
              <template #icon><n-icon><SearchOutline /></n-icon></template>
            </n-button>
          </template>
        </n-input>
      </div>
    </div>

    <n-alert v-if="error" type="error" closable class="media-center-alert" @close="error = ''">
      {{ error }}
    </n-alert>

    <div v-if="!libraryId" class="library-grid" v-show="!loading || libraries.length">
      <button
        v-for="item in libraries"
        :key="item.id"
        class="library-card"
        type="button"
        @click="openLibrary(item.id)"
      >
        <div class="library-art">
          <img v-if="libraryImage(item)" :src="libraryImage(item)" :alt="item.name" />
          <n-icon v-else class="library-art-icon" size="54">
            <component :is="libraryIcon(item.collection_type)" />
          </n-icon>
          <span class="library-count">{{ item.item_count }}</span>
        </div>
        <div class="library-card-body">
          <strong>{{ item.name }}</strong>
        </div>
      </button>
    </div>

    <div v-else class="library-content">
      <div class="media-browser">
        <div v-if="total" ref="mediaGrid" class="media-grid">
          <template v-for="index in total" :key="index - 1">
            <button
              v-if="itemsByIndex[index - 1]"
              class="media-card"
              :data-media-index="index - 1"
              type="button"
              @click="openMedia(itemsByIndex[index - 1].Id)"
            >
              <div class="media-poster-wrap">
                <img v-if="itemImage(itemsByIndex[index - 1])" :src="itemImage(itemsByIndex[index - 1])" :alt="itemsByIndex[index - 1].Name" loading="lazy" />
                <div v-else class="poster-fallback"><n-icon size="32"><component :is="libraryIcon(itemsByIndex[index - 1].Type)" /></n-icon></div>
              </div>
              <div class="media-card-title" :title="itemsByIndex[index - 1].Name">{{ itemsByIndex[index - 1].Name }}</div>
              <div class="media-card-meta">
                <span v-if="itemsByIndex[index - 1].ProductionYear">{{ itemsByIndex[index - 1].ProductionYear }}</span>
                <span v-if="itemsByIndex[index - 1].Type === 'Series' && itemsByIndex[index - 1].ChildCount">{{ itemsByIndex[index - 1].ChildCount }} 季</span>
              </div>
            </button>
            <div v-else class="media-card-placeholder" aria-hidden="true"></div>
          </template>
        </div>

        <n-empty v-else-if="!loading" :description="emptyDescription" class="media-empty" />

        <nav class="alphabet-nav" :style="alphabetStyle" aria-label="按首字母筛选">
          <button
            v-for="letter in letters"
            :key="letter"
            type="button"
            :class="{ 'is-active': selectedLetter === letter }"
            :title="letter === '#' ? '全部' : '按首字母筛选'"
            @click="selectLetter(letter)"
          >
            {{ letter }}
          </button>
        </nav>
      </div>
    </div>

    <div v-if="loading && !hasContent" class="media-center-loading"><n-spin size="large" /></div>
  </div>
</template>

<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import axios from 'axios';
import {
  NAlert, NButton, NEmpty, NIcon, NInput, NPageHeader, NSpin,
} from 'naive-ui';
import {
  ArrowBackOutline,
  FilmOutline,
  MusicalNoteOutline,
  RefreshOutline,
  SearchOutline,
  TvOutline,
} from '@vicons/ionicons5';

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
    ? MusicalNoteOutline
    : normalized === 'tvshows' || normalized === 'series'
      ? TvOutline
      : FilmOutline;
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
        search: search.value.trim(),
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
const goToLibraries = () => router.push({ name: 'MediaCenter' });
const openMedia = (id) => router.push({ name: 'MediaEditPage', params: { itemId: id } });
const resetBrowserPosition = () => {
  mediaPage.value?.scrollTo({ top: 0, behavior: 'auto' });
};
const searchItems = () => {
  resetBrowserPosition();
  loadPage(0, { reset: true });
};
const onSearchInput = (value) => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => searchItems(), 320);
};
const selectLetter = (letter) => {
  if (selectedLetter.value === letter) return;
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
.media-center-page { position:relative; height:100%; min-height:0; overflow-y:auto; padding:0 4px 24px 0; color:var(--ui-text); }
:global(.page-content-inner-wrapper.media-center-layout) { height: 100%; overflow: visible; }
.media-center-sticky-controls { position:sticky; z-index:4; top:0; padding:0 0 12px; background:color-mix(in srgb,var(--ui-page-bg) 94%,transparent); backdrop-filter:blur(8px); }
.media-center-sticky-controls .mp-page-header { margin-bottom:0; }
.media-center-alert { margin: 18px 0; }
.library-grid { display:grid; grid-template-columns:repeat(auto-fill,minmax(min(100%,220px),1fr)); gap:24px 20px; margin-top:20px; }
.library-card { position: relative; min-width: 0; padding: 0; border: 0; border-radius: 8px; color: var(--ui-text); background: transparent; cursor: pointer; text-align: left; }
.library-card:hover .library-art { border-color:var(--accent-color); box-shadow:var(--ui-shadow-hover); transform:translateY(-2px); }
.library-art { position: relative; display: grid; aspect-ratio: 16 / 9; place-items: center; overflow: hidden; border: 1px solid var(--ui-border); border-radius: 8px; color: #fff; background: var(--ui-surface-muted); box-shadow: var(--ui-shadow); transition: border-color .2s, transform .2s, box-shadow .2s; }
.library-art::before { position: absolute; z-index: 1; inset: 0; background: linear-gradient(90deg, rgba(0, 0, 0, .56), rgba(0, 0, 0, .06)); content: ''; }
.library-art img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; }
.library-art-icon, .library-art-title, .library-count { z-index: 2; }
.library-art-icon { position: absolute; left: 16px; opacity: .9; }
.library-count { position:absolute; top:10px; right:10px; display:grid; min-width:28px; height:24px; padding:0 8px; place-items:center; border:1px solid rgba(255,255,255,.32); border-radius:6px; color:#fff; background:rgba(14,17,26,.82); font-size:11px; font-weight:600; }
.library-card-body { display: flex; min-width: 0; flex-direction: column; align-items: center; justify-content: center; gap: 3px; padding: 9px 4px 0; text-align: center; }
.library-card-body strong { width:100%; overflow:hidden; font-size:14px; text-overflow:ellipsis; white-space:nowrap; }
.library-card-body span, .result-count { flex-shrink: 0; color: var(--ui-text-muted); font-size: 12px; }
.library-toolbar { display:flex; align-items:center; gap:14px; margin:20px 0 8px; }
.library-search { width: min(460px, 100%); }
.library-search :deep(.n-input) { border: 1px solid var(--ui-border-strong); border-radius: 10px; background: var(--ui-surface); box-shadow: 0 2px 10px rgba(15, 23, 42, .06); transition: border-color .2s, box-shadow .2s; }
.library-search :deep(.n-input:hover), .library-search :deep(.n-input.n-input--focus) { border-color: var(--n-primary-color); box-shadow: 0 0 0 3px color-mix(in srgb, var(--n-primary-color) 15%, transparent); }
.search-submit { color: var(--n-primary-color); }
.media-browser { position:relative; display:grid; grid-template-columns:minmax(0,1fr) 34px; gap:16px; align-items:start; }
.media-grid { display:grid; grid-auto-rows:auto; grid-template-columns:repeat(auto-fill,minmax(min(100%,148px),1fr)); gap:24px 18px; margin-top:16px; }
.media-card, .media-card-placeholder { min-width:0; height:auto; }
.media-card { padding: 0; border: 0; color: var(--ui-text); background: transparent; cursor: pointer; text-align: left; }
.media-card-placeholder::before { content:''; display:block; aspect-ratio:2/3; }
.media-card-placeholder::after { content:''; display:block; height:50px; }
.media-card:hover .media-poster-wrap { box-shadow:0 0 0 2px var(--accent-color),var(--ui-shadow-hover); transform:translateY(-3px); }
.media-poster-wrap { position: relative; aspect-ratio: 2 / 3; overflow: hidden; border: 1px solid var(--ui-border); border-radius: 7px; background: var(--ui-surface-muted); box-shadow: var(--ui-shadow); transition: border-color .2s, transform .2s, box-shadow .2s; }
.media-poster-wrap img { display: block; width: 100%; height: 100%; object-fit: cover; }
.poster-fallback { display: grid; width: 100%; height: 100%; place-items: center; color: var(--ui-text-muted); }
.media-type { position: absolute; top: 8px; left: 8px; padding: 3px 7px; border-radius: 4px; color: #fff; background: rgba(15, 23, 42, .72); font-size: 11px; }
.media-card-title { margin:9px 2px 3px; overflow:hidden; font-size:14px; font-weight:600; line-height:20px; text-align:center; text-overflow:ellipsis; white-space:nowrap; }
.media-card-meta { display:flex; min-height:18px; justify-content:center; gap:8px; margin:0 2px; color:var(--ui-text-muted); font-size:12px; line-height:18px; text-align:center; }
.media-empty { margin: 70px 0; }
.load-before-sentinel { min-height: 12px; display: flex; align-items: center; justify-content: center; }
.load-more-sentinel { display: flex; min-height: 54px; align-items: center; justify-content: center; gap: 10px; color: var(--ui-text-muted); font-size: 12px; }
.alphabet-nav { position:sticky; z-index:3; top:0; grid-column:2; display:flex; flex-direction:column; gap:2px; margin-top:16px; padding:4px 2px; overflow-y:auto; border:1px solid var(--ui-border); border-radius:8px; background:var(--modal-solid-bg-color); box-shadow:var(--ui-shadow); scrollbar-width:none; }
.alphabet-nav::-webkit-scrollbar { display: none; }
.alphabet-nav button { flex:0 0 20px; width:28px; height:20px; padding:0; border:0; border-radius:5px; color:var(--ui-text-muted); background:transparent; cursor:pointer; font-size:12px; line-height:20px; }
.alphabet-nav button:hover, .alphabet-nav button.is-active { color:var(--accent-color); background:var(--accent-glow-color); font-weight:700; }
.media-center-loading { position: fixed; inset: 64px 0 0 244px; display: grid; place-items: center; pointer-events: none; background: color-mix(in srgb, var(--ui-page-bg) 45%, transparent); }
@container (max-width:540px) {
  .library-grid { grid-template-columns:repeat(2,minmax(0,1fr)); gap:18px 12px; }
  .media-grid { grid-template-columns:repeat(2,minmax(0,1fr)); gap:20px 12px; }
  .library-toolbar { align-items:stretch; }
  .library-search { width:100%; }
}
@media (max-width:959px) { .media-center-loading { left:0; } }
</style>
