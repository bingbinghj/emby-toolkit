<template>
  <div class="dashboard-page mp-page">
    <header class="d-flex flex-wrap align-center justify-space-between ga-3 mb-4">
      <div><h1 class="text-h5 font-weight-medium mb-1">仪表盘</h1><p class="text-body-2 text-medium-emphasis mb-0">115 网盘与 Emby 媒体概览</p></div>
      <div class="d-flex flex-wrap ga-2">
        <VBtn variant="text" prepend-icon="mdi-refresh" :loading="loadingMedia || loadingStorage" @click="refresh">刷新</VBtn>
        <VBtn variant="tonal" prepend-icon="mdi-view-dashboard-edit-outline" @click="isLayoutEditing = !isLayoutEditing">{{ isLayoutEditing ? '完成调整' : '调整布局' }}</VBtn>
        <VBtn variant="text" prepend-icon="mdi-eye-outline" @click="showModules = true">显示模块</VBtn>
      </div>
    </header>
    <VAlert v-if="layoutError" type="warning" variant="tonal" class="mb-4">{{ layoutError }}</VAlert>
    <VAlert v-if="isLayoutEditing" type="info" variant="tonal" class="mb-4">拖动卡片右上角调整位置，也可在“显示模块”中调整顺序。布局自动保存到当前浏览器。</VAlert>
    <VAlert v-if="!visibleModules.length" type="info" variant="tonal">所有模块均已隐藏，可点击“显示模块”重新开启。</VAlert>
    <Draggable v-model="visibleModules" item-key="id" tag="div" class="dashboard-grid" handle=".dashboard-drag-handle" direction="vertical" :force-fallback="true" :fallback-on-body="true" :disabled="!isLayoutEditing" :animation="180">
      <template #item="{ element }">
        <section :class="['dashboard-module', `dashboard-module-${element.width}`]" :data-dashboard-module="element.id">
          <VCard class="dashboard-card h-100">
            <VCardItem>
              <VCardTitle class="dashboard-card-title">{{ element.title }}</VCardTitle>
              <template #append>
                <VBtn v-if="isLayoutEditing" class="dashboard-drag-handle" icon="mdi-drag" variant="text" size="small" :aria-label="`拖动${element.title}`" />
                <VBtn v-else :icon="moduleError(element.id) ? undefined : 'mdi-refresh'" :prepend-icon="moduleError(element.id) ? 'mdi-refresh' : undefined" variant="text" size="small" :aria-label="`${moduleError(element.id) ? '重试' : '刷新'}${element.title}`" :loading="element.id === 'storage' ? loadingStorage : loadingMedia" @click="element.id === 'storage' ? loadStorage() : loadMedia()">{{ moduleError(element.id) ? '重试' : undefined }}</VBtn>
              </template>
            </VCardItem>
            <VCardText v-if="element.id === 'storage'" class="storage-content">
              <VSkeletonLoader v-if="loadingStorage && !storage" type="heading, text, text" />
              <template v-else-if="storage">
                <div class="storage-number">{{ storage.totalText }}</div>
                <div class="text-body-2 text-medium-emphasis mt-1">{{ storage.percent === null ? '使用比例未知' : `已使用 ${storage.percent}%` }}</div>
                <VProgressLinear v-if="storage.percent !== null" :model-value="storage.percent" height="6" rounded class="my-3" />
                <div class="text-caption text-medium-emphasis mt-3">可用 {{ storage.availableText }} / 已用 {{ storage.usedText }}</div>
                <VIcon icon="mdi-harddisk" class="storage-icon" size="58" color="primary" />
              </template>
              <div v-else class="text-body-2 text-medium-emphasis">空间信息未知</div>
              <VAlert v-if="storageError" type="warning" variant="tonal" class="mt-2" density="compact">{{ storageError }}；{{ staleMessage(storageSavedAt) }}</VAlert>
            </VCardText>
            <VCardText v-else-if="element.id === 'statistics'">
              <div class="dashboard-stat-grid">
                <div v-for="item in statisticTypes" :key="item.key" class="dashboard-stat-item">
                  <VAvatar :color="item.color" size="44" variant="tonal"><VIcon :icon="item.icon" size="24" /></VAvatar>
                  <div><span class="text-caption text-medium-emphasis">{{ item.label }}</span><div class="dashboard-number" :data-testid="`dashboard-${item.key}`">{{ formatDashboardCount(statistics[item.key]?.count) }}</div><div v-if="statistics[item.key]?.error" class="text-caption text-warning">读取未确认</div></div>
                </div>
              </div>
              <VAlert v-if="mediaErrors.statistics" type="warning" variant="tonal" class="mt-3" density="compact">{{ mediaErrors.statistics }}；{{ staleMessage(snapshotTimes.statistics) }}</VAlert>
            </VCardText>
            <VCardText v-else-if="element.id === 'recent'" class="pt-1">
              <VAlert v-if="mediaErrors.recent" type="warning" variant="tonal" class="mb-3" density="compact">{{ mediaErrors.recent }}；{{ staleMessage(snapshotTimes.recent) }}</VAlert>
              <VSkeletonLoader v-if="loadingMedia && recent === null" type="image" />
              <div v-else-if="recent?.length" class="dashboard-poster-grid">
                <VCard v-for="item in recent" :key="item.id" class="dashboard-media-card app-hover-lift-card" :to="`/edit-media/${encodeURIComponent(item.id)}`" :aria-label="`编辑${item.name}`" variant="flat">
                  <VImg :src="embyDashboardImage(item) || undefined" aspect-ratio="0.6666667" cover>
                    <template #error><div class="dashboard-art-fallback"><VIcon :icon="mediaIcon(item.item_type)" size="40" /></div></template>
                    <VChip class="dashboard-media-type" size="x-small" variant="flat" :color="item.item_type === 'Movie' ? 'primary' : 'info'">{{ mediaType(item.item_type) }}</VChip>
                  </VImg>
                  <div class="dashboard-poster-copy"><div class="text-body-2 text-truncate">{{ item.name }}</div><div class="text-caption text-medium-emphasis">{{ item.item_type === 'Episode' ? `S${item.season_number ?? '?'} · E${item.episode_number ?? '?'}` : item.production_year || 'Emby 新入库' }}</div></div>
                </VCard>
              </div>
              <p v-else-if="recent !== null" class="text-body-2 text-medium-emphasis mb-0">暂无入库媒体</p>
              <p v-else class="text-body-2 text-medium-emphasis mb-0">最新入库未知</p>
            </VCardText>
            <VCardText v-else class="pt-1">
              <VAlert v-if="mediaErrors.libraries" type="warning" variant="tonal" class="mb-3" density="compact">{{ mediaErrors.libraries }}；{{ staleMessage(snapshotTimes.libraries) }}</VAlert>
              <VSkeletonLoader v-if="loadingMedia && libraries === null" type="image" />
              <div v-else-if="libraries?.length" class="dashboard-library-row" tabindex="0" role="region" aria-label="我的媒体库列表">
                <VCard v-for="library in libraries" :key="library.id" :to="`/library/${encodeURIComponent(library.id)}`" class="dashboard-library-card app-hover-lift-card" :aria-label="`打开${library.name}`" variant="flat">
                  <VImg :src="embyDashboardImage(library, true) || undefined" :aspect-ratio="16 / 9" cover>
                    <template #error><div class="dashboard-art-fallback"><VIcon :icon="mediaIcon(library.collection_type)" size="44" /></div></template>
                  </VImg>
                  <div class="dashboard-library-title text-truncate">{{ library.name }}</div>
                </VCard>
              </div>
              <p v-else-if="libraries !== null" class="text-body-2 text-medium-emphasis mb-0">Emby 暂无媒体库</p>
              <p v-else class="text-body-2 text-medium-emphasis mb-0">媒体库信息未知</p>
            </VCardText>
          </VCard>
        </section>
      </template>
    </Draggable>
    <VDialog v-model="showModules" max-width="560">
      <VCard title="仪表盘模块" subtitle="选择显示模块，并调整显示顺序。">
        <VCardText>
          <div v-for="(id, index) in layout.order" :key="id" class="d-flex align-center ga-2">
            <VCheckbox :model-value="!layout.hidden.includes(id)" :label="moduleById(id).title" hide-details class="flex-grow-1" @update:model-value="toggleModule(id, $event)" />
            <VBtn icon="mdi-arrow-up" variant="text" size="small" :disabled="index === 0" :aria-label="`上移${moduleById(id).title}`" @click="moveModule(index, -1)" />
            <VBtn icon="mdi-arrow-down" variant="text" size="small" :disabled="index === layout.order.length - 1" :aria-label="`下移${moduleById(id).title}`" @click="moveModule(index, 1)" />
          </div>
        </VCardText>
        <VCardActions><VBtn variant="text" @click="layout = normalizeDashboardLayout(null)">恢复默认布局</VBtn><VSpacer /><VBtn variant="flat" @click="showModules = false">完成</VBtn></VCardActions>
      </VCard>
    </VDialog>
  </div>
</template>

<script setup>
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import axios from 'axios';
import Draggable from 'vuedraggable';
import { useAuthStore } from '@/stores/auth';
import { dashboardModules, normalizeDashboardLayout, reorderVisibleDashboard, storageFrom115, embyDashboardImage, formatDashboardCount, dashboardSnapshot, projectDashboardItems, projectDashboardStatistics } from '@/utils/dashboard';

const layoutKey = 'etk-dashboard-layout-v1';
const layoutError = ref('');
const readLayout = () => {
  try { return normalizeDashboardLayout(JSON.parse(localStorage.getItem(layoutKey) || 'null')); }
  catch { return normalizeDashboardLayout(null); }
};
const layout = ref(readLayout());
const moduleById = id => dashboardModules.find(item => item.id === id);
const visibleModules = computed({
  get: () => layout.value.order.filter(id => !layout.value.hidden.includes(id)).map(moduleById),
  set: value => { layout.value.order = reorderVisibleDashboard(layout.value.order, value.map(item => item.id)); },
});
watch(layout, value => {
  try { localStorage.setItem(layoutKey, JSON.stringify(value)); layoutError.value = ''; }
  catch { layoutError.value = '当前浏览器无法保存布局，刷新后将使用默认布局'; }
}, { deep: true });
const showModules = ref(false);
const isLayoutEditing = ref(false);
const toggleModule = (id, shown) => { layout.value.hidden = shown ? layout.value.hidden.filter(item => item !== id) : [...new Set([...layout.value.hidden, id])]; };
const moveModule = (index, direction) => {
  const order = [...layout.value.order];
  [order[index], order[index + direction]] = [order[index + direction], order[index]];
  layout.value.order = order;
};
const statisticTypes = [
  { key: 'movie_count', label: '电影', icon: 'mdi-movie-roll', color: 'primary' },
  { key: 'tv_count', label: '电视剧', icon: 'mdi-television', color: 'success' },
  { key: 'episode_count', label: '剧集', icon: 'mdi-television-classic', color: 'warning' },
  { key: 'music_count', label: '音乐（歌曲）', icon: 'mdi-music', color: 'info' },
];
const authStore = useAuthStore();
const snapshotUserId = String(authStore.user?.id || '');
const projections = { statistics: projectDashboardStatistics, recent: value => projectDashboardItems(value), libraries: value => projectDashboardItems(value, true) };
const snapshots = Object.fromEntries(Object.keys(projections).map(key => [key, dashboardSnapshot(snapshotUserId, key)]));
const readSnapshot = key => {
  const snapshot = snapshots[key].read();
  if (!snapshot) return undefined;
  try { return { ...snapshot, value: projections[key](snapshot.value) }; }
  catch { snapshots[key].clear(); return undefined; }
};
const initialSnapshots = Object.fromEntries(Object.keys(projections).map(key => [key, readSnapshot(key)]));
const snapshotTimes = ref(Object.fromEntries(Object.keys(projections).map(key => [key, initialSnapshots[key]?.savedAt])));
const statistics = ref(initialSnapshots.statistics?.value || {});
const recent = ref(initialSnapshots.recent?.value ?? null);
const libraries = ref(initialSnapshots.libraries?.value ?? null);
const mediaErrors = ref({});
const storage = ref(null);
const storageSavedAt = ref(null);
const storageError = ref('');
const loadingMedia = ref(false);
const loadingStorage = ref(false);
let mediaSequence = 0;
let storageSequence = 0;
let active = true;
const isCurrentDashboard = () => active && String(authStore.user?.id || '') === snapshotUserId;
const moduleError = id => id === 'storage' ? storageError.value : mediaErrors.value[id];
const staleMessage = time => time ? `显示上次成功数据（${new Date(time).toLocaleString('zh-CN')}），请重试` : '尚未成功读取，请重试';
const loadMedia = async () => {
  const sequence = ++mediaSequence;
  loadingMedia.value = true;
  try {
    const { data } = await axios.get('/api/dashboard/media');
    if (!isCurrentDashboard() || sequence !== mediaSequence) return;
    if (!data || typeof data !== 'object' || Array.isArray(data)) throw new Error('仪表盘响应无效');
    const values = { statistics, recent, libraries };
    const errors = {};
    for (const key of Object.keys(values)) {
      try {
        if (data.errors?.[key]) throw new Error(data.errors[key]);
        const value = projections[key](data[key]);
        values[key].value = value;
        snapshotTimes.value[key] = snapshots[key].write(value).savedAt;
      } catch (error) { errors[key] = error.message || '读取未确认'; }
    }
    mediaErrors.value = errors;
  } catch (error) {
    if (isCurrentDashboard() && sequence === mediaSequence) {
      const reason = error.response?.data?.error || error.message || '无法读取 Emby 数据';
      mediaErrors.value = { statistics: reason, recent: reason, libraries: reason };
    }
  } finally { if (isCurrentDashboard() && sequence === mediaSequence) loadingMedia.value = false; }
};
const loadStorage = async () => {
  const sequence = ++storageSequence;
  loadingStorage.value = true;
  try {
    const { data } = await axios.get('/api/p115/status');
    if (!isCurrentDashboard() || sequence !== storageSequence) return;
    storage.value = storageFrom115(data);
    storageSavedAt.value = Date.now();
    storageError.value = '';
  } catch (error) {
    if (isCurrentDashboard() && sequence === storageSequence) {
      storageError.value = error.response?.data?.error || error.message || '无法读取 115 空间';
    }
  } finally { if (isCurrentDashboard() && sequence === storageSequence) loadingStorage.value = false; }
};
const refresh = () => { loadStorage(); loadMedia(); };
const mediaType = type => ({ Movie: '电影', Series: '电视剧', Episode: '剧集', MusicAlbum: '音乐' }[type] || '媒体');
const mediaIcon = type => /music|Audio|Music/.test(type || '') ? 'mdi-music' : /tv|Series|Episode/.test(type || '') ? 'mdi-television' : 'mdi-movie-outline';
onMounted(refresh);
onBeforeUnmount(() => { active = false; ++mediaSequence; ++storageSequence; });
</script>

<style scoped>
.dashboard-grid { display: grid; grid-template-columns: repeat(12, minmax(0, 1fr)); gap: 20px; }
.dashboard-module { min-width: 0; }
.dashboard-module-summary { grid-column: span 4; }
.dashboard-module-statistics { grid-column: span 8; }
.dashboard-module-full { grid-column: span 12; }
.dashboard-card-title { font-size: 1rem; font-weight: 500; }
.dashboard-card { border-radius: var(--v-card-border-radius, 12px); }
.dashboard-drag-handle { cursor: grab; touch-action: none; }
.storage-content { position: relative; min-height: 114px; padding-top: 0; }
.storage-number { font-size: clamp(1.5rem, 1.8vw, 1.75rem); font-weight: 700; line-height: 1.2; font-variant-numeric: tabular-nums; padding-right: 70px; }
.storage-icon { position: absolute; right: 24px; top: 0; opacity: 0.8; }
.dashboard-stat-grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); min-height: 98px; align-items: center; }
.dashboard-stat-item { display: flex; gap: 12px; align-items: center; padding: 0 18px; min-width: 0; }
.dashboard-stat-item:first-child { padding-left: 0; }
.dashboard-stat-item + .dashboard-stat-item { border-left: 1px solid rgba(var(--v-border-color), var(--v-border-opacity)); }
.dashboard-number { color: rgba(var(--v-theme-on-surface), var(--v-high-emphasis-opacity)); font-size: 1.2rem; font-weight: 700; font-variant-numeric: tabular-nums; }
.dashboard-poster-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(132px, 1fr)); gap: 16px; }
.dashboard-library-row { display: flex; gap: 21px; overflow-x: auto; padding-bottom: 8px; scroll-snap-type: x proximity; scrollbar-width: thin; }
.dashboard-media-card, .dashboard-library-card { min-width: 0; background: transparent; }
.dashboard-library-card { flex: 0 0 clamp(154px, 14vw, 245px); scroll-snap-align: start; }
.dashboard-library-card :deep(.v-img) { border-radius: 9px; }
.dashboard-media-type { position: absolute; top: 8px; left: 8px; }
.dashboard-poster-copy { padding-top: 8px; }
.dashboard-library-title { padding-top: 8px; text-align: center; font-size: .9rem; font-weight: 400; line-height: 1.4; }
.dashboard-art-fallback { height: 100%; width: 100%; display: flex; align-items: center; justify-content: center; background: rgba(var(--v-theme-on-surface), 0.08); }
@media (max-width: 1200px) { .dashboard-module-summary, .dashboard-module-statistics { grid-column: span 12; } }
@media (max-width: 740px) {
  .dashboard-grid { gap: 16px; }
  .dashboard-stat-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 20px 8px; }
  .dashboard-stat-item { padding: 0 8px; gap: 10px; }
  .dashboard-stat-item:nth-child(odd) { border-left: 0; padding-left: 0; }
  .dashboard-poster-grid { grid-template-columns: repeat(auto-fill, minmax(96px, 1fr)); gap: 12px; }
  .dashboard-library-row { gap: 11px; }
}
</style>
