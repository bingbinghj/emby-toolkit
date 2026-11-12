<template>
  <div class="dashboard-page mp-page">
    <VAlert v-if="layoutError" type="warning" variant="tonal" class="mb-4">{{ layoutError }}</VAlert>
    <VAlert v-if="layoutSyncError" type="warning" variant="tonal" class="mb-4">{{ layoutSyncError }}<VBtn variant="text" size="small" class="ml-2" @click="loadRemoteLayout(layoutProfile)">重试布局同步</VBtn></VAlert>
    <VAlert v-if="isLayoutEditing" type="info" variant="tonal" class="mb-4">拖动卡片右上角调整位置，点击右下角勾选按钮保存布局。</VAlert>
    <VAlert v-if="!visibleModules.length" type="info" variant="tonal">所有模块均已隐藏，可点击右下角设置按钮重新开启。</VAlert>
    <div ref="gridRef" class="grid-stack dashboard-grid" :class="{ 'is-editing': isLayoutEditing }" :data-layout-profile="layoutProfile">
        <section v-for="element in visibleModules" :key="element.id" class="grid-stack-item dashboard-module" :class="{ 'is-manual-height': layout.items[element.id]?.manualHeight }" :data-dashboard-module="element.id" :gs-id="element.id" :gs-x="widgetFor(element.id).x" :gs-y="widgetFor(element.id).y" :gs-w="widgetFor(element.id).w" :gs-h="widgetFor(element.id).h">
          <div class="grid-stack-item-content">
          <VCard class="dashboard-card">
            <VCardItem>
              <VCardTitle class="dashboard-card-title">{{ element.title }}</VCardTitle>
              <template #append>
                <span v-if="isLayoutEditing" class="dashboard-drag-handle" role="button" tabindex="0" :aria-label="`拖动${element.title}`"><VIcon icon="mdi-drag" size="small" /></span>
                <VBtn v-else-if="moduleError(element.id)" prepend-icon="mdi-refresh" variant="text" size="small" :aria-label="`重试${element.title}`" :loading="element.id === 'storage' ? loadingStorage : loadingMedia" @click="element.id === 'storage' ? loadStorage() : loadMedia()">重试</VBtn>
              </template>
            </VCardItem>
            <VCardText v-if="element.id === 'storage'" class="storage-content">
              <VSkeletonLoader v-if="loadingStorage && !storage" type="heading, text, text" />
              <template v-else-if="storage">
                <div class="storage-number">{{ storage.totalText }}</div>
                <div class="text-body-2 text-medium-emphasis mt-1">{{ storage.percent === null ? '使用比例未知' : `已使用 ${storage.percent.toFixed(1)}%` }}</div>
                <VProgressLinear v-if="storage.percent !== null" :model-value="storage.percent" height="6" rounded class="my-3" />
                <div class="storage-caption text-medium-emphasis">可用 {{ storage.availableText }} / 总容量 {{ storage.totalText }}</div>
                <VImg :src="storageImage" class="storage-icon" />
              </template>
              <div v-else class="text-body-2 text-medium-emphasis">空间信息未知</div>
              <VAlert v-if="storageError" type="warning" variant="tonal" class="mt-2" density="compact">{{ storageError }}；{{ staleMessage(storageSavedAt) }}</VAlert>
            </VCardText>
            <VCardText v-else-if="element.id === 'statistics'">
              <div class="dashboard-stat-grid">
                <div v-for="item in statisticTypes" :key="item.key" class="dashboard-stat-item">
                  <VAvatar :color="item.color" size="46"><VIcon :icon="item.icon" size="24" /></VAvatar>
                  <div class="dashboard-stat-copy"><span class="dashboard-stat-label">{{ item.label }}</span><div class="dashboard-number" :data-testid="`dashboard-${item.key}`">{{ formatDashboardCount(statistics[item.key]?.count) }}</div><span class="dashboard-stat-addition" :data-testid="`dashboard-${item.key}-month`" :title="monthlySourceText">本月新增 {{ formatDashboardCount(dashboardMonthlyCount(statistics, item.key)) }}</span><div v-if="statistics[item.key]?.error" class="text-caption text-warning">读取未确认</div></div>
                </div>
              </div>
              <VAlert v-if="mediaErrors.statistics" type="warning" variant="tonal" class="mt-3" density="compact">{{ mediaErrors.statistics }}；{{ staleMessage(snapshotTimes.statistics) }}</VAlert>
              <VAlert v-else-if="statistics.month?.error" type="warning" variant="tonal" class="mt-3" density="compact">本月入库统计读取失败：{{ statistics.month.error }}</VAlert>
            </VCardText>
            <VCardText v-else-if="element.id === 'recent'" class="pt-1">
              <VAlert v-if="mediaErrors.recent" type="warning" variant="tonal" class="mb-3" density="compact">{{ mediaErrors.recent }}；{{ staleMessage(snapshotTimes.recent) }}</VAlert>
              <VSkeletonLoader v-if="loadingMedia && recent === null" type="image" />
              <div v-else-if="recent?.length" class="dashboard-poster-grid">
                <VHover v-for="item in visibleRecent" :key="item.id">
                  <template #default="hover">
                    <div v-bind="hover.props" class="dashboard-poster-hover-area">
                      <VCard class="dashboard-media-card app-hover-lift-card" :class="{ 'app-hover-lift-card--hovering': hover.isHovering, 'dashboard-poster-fallback': failedPosterImages.has(item.id) }" :to="`/edit-media/${encodeURIComponent(item.id)}`" :aria-label="`编辑${item.name}`" variant="flat">
                        <VImg class="dashboard-poster-image" :src="embyDashboardImage(item) || undefined" aspect-ratio="0.6666667" cover @error="failedPosterImages.add(item.id)" @load="failedPosterImages.delete(item.id)">
                          <template #placeholder><VSkeletonLoader type="image" class="h-100" /></template>
                          <template #error><div class="dashboard-art-fallback"><VIcon :icon="mediaIcon(item.item_type)" size="40" /></div></template>
                        </VImg>
                        <VChip class="dashboard-media-type" size="small" variant="elevated" :color="item.item_type === 'Movie' ? 'info' : 'primary'">{{ mediaType(item.item_type) }}</VChip>
                        <div class="dashboard-poster-overlay" :class="{ 'is-visible': hover.isHovering }">
                          <span class="dashboard-poster-subtitle" :title="String(item.production_year || 'Emby 新入库')">{{ String(item.production_year || 'Emby 新入库') }}</span>
                          <h3 class="dashboard-poster-title" :title="item.name">{{ item.name }}</h3>
                        </div>
                      </VCard>
                    </div>
                  </template>
                </VHover>
              </div>
              <p v-else-if="recent !== null" class="text-body-2 text-medium-emphasis mb-0">暂无最近入库记录</p>
              <p v-else class="text-body-2 text-medium-emphasis mb-0">最新入库未知</p>
            </VCardText>
            <VCardText v-else class="pt-1">
              <VAlert v-if="mediaErrors.libraries" type="warning" variant="tonal" class="mb-3" density="compact">{{ mediaErrors.libraries }}；{{ staleMessage(snapshotTimes.libraries) }}</VAlert>
              <VSkeletonLoader v-if="loadingMedia && libraries === null" type="image" />
              <div v-else-if="libraries?.length" class="dashboard-library-row" tabindex="0" role="region" aria-label="我的媒体库列表">
                <VCard v-for="library in libraries" :key="library.id" :to="`/library/${encodeURIComponent(library.id)}`" class="dashboard-library-card app-hover-lift-card" :aria-label="`打开${library.name}`" variant="flat">
                  <VImg :src="embyDashboardImage(library, true) || undefined" :aspect-ratio="16 / 9" cover>
                    <template #error><div class="dashboard-art-fallback"><VIcon :icon="mediaIcon(library.collection_type)" size="44" /></div></template>
                    <span class="dashboard-library-count" :aria-label="`媒体数量 ${formatDashboardCount(library.item_count)}`" :title="library.count_error || '媒体数量'">{{ formatDashboardCount(library.item_count) }}</span>
                  </VImg>
                  <div class="dashboard-library-title text-truncate">{{ library.name }}</div>
                </VCard>
              </div>
              <p v-else-if="libraries !== null" class="text-body-2 text-medium-emphasis mb-0">暂无媒体库数据</p>
              <p v-else class="text-body-2 text-medium-emphasis mb-0">媒体库信息未知</p>
            </VCardText>
          </VCard>
          </div>
        </section>
    </div>
    <VDialog v-model="showModules" max-width="560" :fullscreen="width < 960" scrollable>
      <VCard class="dashboard-settings-card">
        <VCardItem><VCardTitle><VIcon icon="mdi-tune" size="small" class="me-2" />仪表盘设置</VCardTitle><template #append><VBtn icon="mdi-close" variant="text" color="default" aria-label="关闭仪表盘设置" @click="showModules = false" /></template></VCardItem>
        <VDivider />
        <VCardText>
          <p class="text-body-2 text-medium-emphasis mb-4">选择仪表盘中显示的内容，位置与大小可通过右下角布局按钮调整。</p>
          <div class="dashboard-settings-grid">
            <button v-for="element in dashboardModules" :key="element.id" type="button" class="dashboard-setting-item" :class="{ enabled: !settingsHidden.includes(element.id) }" :aria-pressed="!settingsHidden.includes(element.id)" @click="toggleSettingsModule(element.id)">
              <VIcon :icon="settingsHidden.includes(element.id) ? 'mdi-circle-outline' : 'mdi-check-circle'" :color="settingsHidden.includes(element.id) ? undefined : 'primary'" size="small" />
              <span class="dashboard-setting-label">{{ element.title }}</span>
            </button>
          </div>
        </VCardText>
        <VCardActions class="app-dialog-actions"><VSpacer /><VBtn color="primary" variant="flat" prepend-icon="mdi-content-save" @click="saveModules">保存</VBtn></VCardActions>
      </VCard>
    </VDialog>
    <Teleport to="body">
      <div ref="fabStack" class="compact-fab-stack dashboard-fab-stack" :class="{ 'is-expanded': fabExpanded }">
        <div class="compact-fab compact-fab--secondary"><VBtn icon="mdi-tune" color="info" variant="tonal" aria-label="仪表盘设置" title="仪表盘设置" @click="showModules = true" /></div>
        <div v-if="isLayoutEditing" class="compact-fab compact-fab--secondary"><VBtn icon="mdi-restore" color="warning" variant="tonal" aria-label="恢复默认布局" title="恢复默认布局" @click="resetLayout" /></div>
        <div class="compact-fab compact-fab--primary"><VBtn :icon="isLayoutEditing ? 'mdi-check' : 'mdi-view-dashboard-edit'" color="primary" :aria-label="isLayoutEditing ? '完成调整' : '调整布局'" :title="isLayoutEditing ? '完成调整' : '调整布局'" @click="toggleLayoutEditing" /></div>
      </div>
    </Teleport>
  </div>
</template>

<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useDisplay } from 'vuetify';
import { GridStack } from 'gridstack';
import 'gridstack/dist/gridstack.min.css';
import axios from 'axios';
import storageImage from '@/assets/mp-storage.png';
import { useAuthStore } from '@/stores/auth';
import { dashboardModules, normalizeDashboardProfile, dashboardLayoutProfile, dashboardColumns, storageFrom115, embyDashboardImage, formatDashboardCount, dashboardSnapshot, projectDashboardItems, projectDashboardStatistics, dashboardMonthlyCount } from '@/utils/dashboard';

const authStore = useAuthStore();
const snapshotUserId = String(authStore.user?.id || '');
const { width } = useDisplay();
const layoutProfile = computed(() => dashboardLayoutProfile(width.value));
const layoutKey = profile => `etk-dashboard-layout-v2:${snapshotUserId}:${profile}`;
const layoutError = ref('');
const profileDrafts = new Map();
const confirmedProfiles = new Map();
const layoutSyncErrors = ref({});
const layoutSyncError = computed(() => layoutSyncErrors.value[layoutProfile.value] || '');
let profileSaveQueue = Promise.resolve();
let applyingLayout = 0;
let layoutRevision = 0;
const layoutReadSequences = new Map();
const readLayout = profile => {
  try { return normalizeDashboardProfile(JSON.parse(localStorage.getItem(layoutKey(profile)) || localStorage.getItem('etk-dashboard-layout-v1') || 'null'), profile); }
  catch { return normalizeDashboardProfile(null, profile); }
};
const layout = ref(readLayout(layoutProfile.value));
const isLayoutEditing = ref(false);
const moduleById = id => dashboardModules.find(item => item.id === id);
const visibleModules = computed(() => layout.value.order.filter(id => !layout.value.hidden.includes(id)).map(moduleById));
const queueRemoteLayout = (value, profile) => {
  profileSaveQueue = profileSaveQueue.catch(() => {}).then(async () => {
    if (String(authStore.user?.id || '') !== snapshotUserId) return;
    try {
      const { data } = await axios.post(`/api/dashboard/layout/${profile}`, { layout: value });
      const comparable = input => {
        const result = normalizeDashboardProfile(input, profile);
        return JSON.stringify({ ...result, hidden: [...result.hidden].sort() });
      };
      if (!data?.layout || comparable(data.layout) !== comparable(value)) throw new Error('布局保存未确认');
      if (active && confirmedProfiles.get(profile)?.updatedAt === value.updatedAt) delete layoutSyncErrors.value[profile];
    } catch (error) {
      if (active) layoutSyncErrors.value[profile] = `布局同步失败：${error.response?.data?.error || error.message || '请重试'}`;
    }
  });
  return profileSaveQueue;
};
const saveLayout = (value, profile = layoutProfile.value) => {
  const saved = { ...normalizeDashboardProfile(value, profile), updatedAt: Date.now() };
  confirmedProfiles.set(profile, saved);
  try { localStorage.setItem(layoutKey(profile), JSON.stringify(saved)); layoutError.value = ''; }
  catch { layoutError.value = '当前浏览器无法保存布局，刷新后将使用服务器布局'; }
  return queueRemoteLayout(saved, profile);
};
watch(layout, value => {
  if (applyingLayout) return;
  ++layoutRevision;
  if (!isLayoutEditing.value) saveLayout(value);
}, { deep: true });
const applyLayout = async value => {
  ++applyingLayout;
  layout.value = JSON.parse(JSON.stringify(value));
  await nextTick();
  --applyingLayout;
};
const loadRemoteLayout = async profile => {
  await nextTick();
  if (!isCurrentDashboard() || profile !== layoutProfile.value) return;
  const sequence = (layoutReadSequences.get(profile) || 0) + 1;
  layoutReadSequences.set(profile, sequence);
  const revision = layoutRevision;
  try {
    const { data } = await axios.get(`/api/dashboard/layout/${profile}`);
    if (!isCurrentDashboard() || sequence !== layoutReadSequences.get(profile)) return;
    if (!data || !Object.hasOwn(data, 'layout') || (data.layout !== null && (typeof data.layout !== 'object' || Array.isArray(data.layout)))) throw new Error('布局响应无效');
    const local = confirmedProfiles.get(profile) || readLayout(profile);
    const remote = data.layout === null ? null : normalizeDashboardProfile(data.layout, profile);
    let hasLocal = confirmedProfiles.has(profile);
    try { hasLocal ||= !!(localStorage.getItem(layoutKey(profile)) || localStorage.getItem('etk-dashboard-layout-v1')); } catch {}
    if (hasLocal && (!remote || (local.updatedAt || 0) > (remote.updatedAt || 0))) {
      if (!local.updatedAt) await saveLayout(local, profile);
      else await queueRemoteLayout(local, profile);
      return;
    }
    delete layoutSyncErrors.value[profile];
    if (!remote || profile !== layoutProfile.value || isLayoutEditing.value || revision !== layoutRevision) return;
    confirmedProfiles.set(profile, remote);
    try { localStorage.setItem(layoutKey(profile), JSON.stringify(remote)); } catch {}
    await applyLayout(remote);
    await rebuildGrid();
  } catch (error) {
    if (isCurrentDashboard() && sequence === layoutReadSequences.get(profile)) layoutSyncErrors.value[profile] = `布局读取失败：${error.response?.data?.error || error.message || '请重试'}`;
  }
};
const gridRef = ref(null);
let dashboardGrid;
let contentObserver;
let resizeFrame;
let gridGeneration = 0;
const resizeStartHeights = new Map();
const widgetFor = id => {
  if (layout.value.items[id]) return layout.value.items[id];
  const columns = dashboardColumns(layoutProfile.value);
  let x = 0, y = 0, rowHeight = 0;
  for (const module of visibleModules.value) {
    const w = module.width === 'full' || columns === 1 ? columns : Math.round(columns * (module.width === 'summary' ? 1 / 3 : 2 / 3));
    const h = module.width === 'full' ? 20 : 8;
    if (x + w > columns) { x = 0; y += rowHeight; rowHeight = 0; }
    if (module.id === id) return { x, y, w, h };
    x += w; rowHeight = Math.max(rowHeight, h);
  }
  return { x: 0, y: 0, w: columns, h: 8 };
};
const captureGrid = () => {
  if (!dashboardGrid) return;
  for (const node of dashboardGrid.engine.nodes) {
    layout.value.items[node.id] = { x: node.x, y: node.y, w: node.w, h: node.h, manualHeight: layout.value.items[node.id]?.manualHeight === true };
  }
};
const resizeToContent = () => {
  if (resizeFrame) cancelAnimationFrame(resizeFrame);
  resizeFrame = requestAnimationFrame(() => {
    resizeFrame = undefined;
    if (!dashboardGrid) return;
    const posterGrid = gridRef.value.querySelector('.dashboard-poster-grid');
    if (posterGrid) recentCapacity.value = Math.max(1, Math.floor((posterGrid.clientWidth + 16) / 160)) * (width.value < 960 ? 3 : 2);
    if (isLayoutEditing.value) return;
    dashboardGrid.batchUpdate();
    for (const node of dashboardGrid.engine.nodes) {
      if (layout.value.items[node.id]?.manualHeight) continue;
      const card = node.el.querySelector('.dashboard-card');
      const h = Math.max(4, Math.ceil((card.getBoundingClientRect().height + 16) / 16));
      if (node.h !== h) dashboardGrid.update(node.el, { h });
    }
    dashboardGrid.batchUpdate(false);
  });
};
const rebuildGrid = async () => {
  const generation = ++gridGeneration;
  contentObserver?.disconnect();
  dashboardGrid?.destroy(false);
  dashboardGrid = undefined;
  await nextTick();
  if (generation !== gridGeneration || !gridRef.value) return;
  // GridStack mutates attributes outside Vue; always restore the current profile before registration.
  gridRef.value.querySelectorAll('[data-dashboard-module]').forEach(element => {
    for (const [key, value] of Object.entries(widgetFor(element.dataset.dashboardModule))) {
      if (['x', 'y', 'w', 'h'].includes(key)) element.setAttribute(`gs-${key}`, String(value));
    }
  });
  // MP GridStack uses a 16px cell, 8px margins and separate device profiles.
  dashboardGrid = GridStack.init({ animate: false, cellHeight: 16, margin: 8, column: dashboardColumns(layoutProfile.value), float: false, draggable: { cancel: 'input,textarea,button,select,option,a', handle: '.dashboard-drag-handle' }, resizable: { handles: 'e,s,se' }, staticGrid: !isLayoutEditing.value }, gridRef.value);
  dashboardGrid.on('dragstop', captureGrid);
  dashboardGrid.on('resizestart', (_event, element) => resizeStartHeights.set(element.gridstackNode.id, element.gridstackNode.h));
  dashboardGrid.on('resizestop', (_event, element) => {
    const node = element.gridstackNode;
    layout.value.items[node.id] = { ...widgetFor(node.id), manualHeight: layout.value.items[node.id]?.manualHeight === true || node.h !== resizeStartHeights.get(node.id) };
    resizeStartHeights.delete(node.id);
    captureGrid();
  });
  contentObserver = new ResizeObserver(resizeToContent);
  gridRef.value.querySelectorAll('.dashboard-card').forEach(card => contentObserver.observe(card));
  resizeToContent();
};
watch(() => layout.value.order.join(',') + '|' + layout.value.hidden.join(','), rebuildGrid, { flush: 'post' });
watch(layoutProfile, async (profile, previous) => {
  if (isLayoutEditing.value) { captureGrid(); profileDrafts.set(previous, JSON.parse(JSON.stringify(layout.value))); }
  await applyLayout(profileDrafts.get(profile) || confirmedProfiles.get(profile) || readLayout(profile));
  await rebuildGrid();
  loadRemoteLayout(profile);
}, { flush: 'post' });
const fabStack = ref(null);
const fabExpanded = ref(false);
const handleFabClick = event => {
  if (window.matchMedia('(hover: hover) and (pointer: fine)').matches) return;
  if (!fabStack.value?.contains(event.target)) { fabExpanded.value = false; return; }
  if (fabExpanded.value) return;
  event.preventDefault();
  event.stopPropagation();
  fabExpanded.value = true;
};
const showModules = ref(false);
const settingsHidden = ref([]);
watch(showModules, shown => { if (shown) settingsHidden.value = [...layout.value.hidden]; });
const toggleSettingsModule = id => { settingsHidden.value = settingsHidden.value.includes(id) ? settingsHidden.value.filter(item => item !== id) : [...settingsHidden.value, id]; };
const saveModules = () => { layout.value.hidden = [...settingsHidden.value]; showModules.value = false; };
const resetLayout = () => { layout.value = normalizeDashboardProfile(null, layoutProfile.value); rebuildGrid(); };
const toggleLayoutEditing = async () => {
  if (isLayoutEditing.value) {
    ++applyingLayout;
    try {
      captureGrid();
      for (const [profile, draft] of profileDrafts) saveLayout(draft, profile);
      profileDrafts.clear();
      saveLayout(layout.value);
      isLayoutEditing.value = false;
      await nextTick();
    } finally { --applyingLayout; }
  } else { isLayoutEditing.value = true; await nextTick(); }
  dashboardGrid?.setStatic(!isLayoutEditing.value);
  if (isLayoutEditing.value) { dashboardGrid?.enableMove(true); dashboardGrid?.enableResize(true); }
  else resizeToContent();
};
const statisticTypes = [
  { key: 'movie_count', label: '电影', icon: 'mdi-movie-roll', color: 'primary' },
  { key: 'tv_count', label: '电视剧', icon: 'mdi-television-box', color: 'success' },
  { key: 'episode_count', label: '剧集', icon: 'mdi-television-classic', color: 'warning' },
];
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
const monthlySourceText = computed(() => {
  const month = statistics.value.month;
  if (!month) return '本月入库统计尚未读取';
  if (month.error) return month.error;
  const since = month.recording_since && new Date(month.recording_since);
  return `按 ETK 成功入库记录统计${month.partial && since && !Number.isNaN(since.getTime()) ? `；记录始于 ${since.toLocaleString('zh-CN')}` : ''}`;
});
const recent = ref(initialSnapshots.recent?.value ?? null);
const recentCapacity = ref(24);
const failedPosterImages = ref(new Set());
const visibleRecent = computed(() => recent.value?.slice(0, recentCapacity.value) || []);
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
const moduleError = id => id === 'storage' ? storageError.value : mediaErrors.value[id] || (id === 'statistics' ? statistics.value.month?.error : '');
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
onMounted(() => { refresh(); rebuildGrid(); loadRemoteLayout(layoutProfile.value); document.addEventListener('click', handleFabClick, true); });
onBeforeUnmount(() => { active = false; ++mediaSequence; ++storageSequence; ++gridGeneration; contentObserver?.disconnect(); dashboardGrid?.destroy(false); if (resizeFrame) cancelAnimationFrame(resizeFrame); document.removeEventListener('click', handleFabClick, true); });
</script>

<style scoped>
.dashboard-grid { margin: -8px; }
.dashboard-module { min-width: 0; }
.dashboard-grid > .dashboard-module > .grid-stack-item-content { overflow: visible; }
.dashboard-card { display: flex; flex-direction: column; container-type: inline-size; }
.dashboard-grid.is-editing .dashboard-card, .dashboard-module.is-manual-height .dashboard-card { height: 100%; min-height: 0; }
.dashboard-grid.is-editing .dashboard-card > :deep(.v-card-text), .dashboard-module.is-manual-height .dashboard-card > :deep(.v-card-text) { overflow: auto; min-height: 0; flex: 1 1 auto; }
.dashboard-grid :deep(.ui-resizable-handle) { z-index: 11; }
.dashboard-grid.is-editing :deep(.ui-resizable-se) { width: 24px; height: 24px; right: -4px; bottom: -4px; }
.dashboard-card-title { font-size: .92rem; font-weight: 650; }
.dashboard-card { border: var(--app-surface-border); border-radius: var(--app-surface-radius); box-shadow: var(--app-surface-shadow); }
.dashboard-drag-handle { display: inline-flex; align-items: center; justify-content: center; width: 28px; height: 28px; cursor: move; touch-action: none; }
.storage-content { position: static; min-height: 114px; padding-top: 0; }
.storage-number { color: rgba(var(--v-theme-on-surface), var(--v-high-emphasis-opacity)); font-size: clamp(1.5rem, 1.8vw, 1.75rem); font-weight: 700; line-height: 1.2; font-variant-numeric: tabular-nums; padding-right: 70px; }
.storage-icon { position: absolute; right: 1.35rem; top: 2.7rem; width: clamp(3.6rem, 18%, 4.5rem); filter: hue-rotate(225deg) saturate(.72); }
.storage-caption { padding-right: 4.75rem; font-size: .68rem; margin-top: .35rem; line-height: 1.5; overflow-wrap: anywhere; }
.dashboard-stat-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); min-height: 98px; align-items: center; }
.dashboard-stat-item { display: flex; gap: 12px; align-items: center; padding: 0 18px; min-width: 0; }
.dashboard-stat-item:first-child { padding-left: 0; }
.dashboard-stat-item + .dashboard-stat-item { border-left: 1px solid rgba(var(--v-border-color), var(--v-border-opacity)); }
.dashboard-stat-copy { display: flex; flex-direction: column; min-width: 0; overflow-wrap: anywhere; }
.dashboard-stat-label { color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity)); font-size: .72rem; }
.dashboard-stat-addition { color: var(--ui-success-text, rgb(var(--v-theme-success))); font-size: .72rem; line-height: 1.5; overflow-wrap: anywhere; }
.dashboard-number { color: rgba(var(--v-theme-on-surface), var(--v-high-emphasis-opacity)); font-size: 1.15rem; font-weight: 700; line-height: 1.4; font-variant-numeric: tabular-nums; }
.dashboard-poster-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(144px, 1fr)); gap: 16px; padding: 4px 0; }
.dashboard-library-row { display: grid; grid-template-columns: repeat(7, minmax(0, 1fr)); gap: 20px; padding: 4px 0 8px; }
.dashboard-media-card, .dashboard-library-card { min-width: 0; background: transparent; border: 0; box-shadow: none; }
.dashboard-library-card { border-radius: 0; overflow: visible; }
.dashboard-media-card { aspect-ratio: 2 / 3; border: var(--app-surface-border); border-radius: var(--app-surface-radius); box-shadow: var(--app-surface-shadow); overflow: hidden; }
.dashboard-poster-hover-area { width: 100%; height: 100%; }
.dashboard-poster-image { height: 100%; width: 100%; }
.dashboard-library-card :deep(.v-img) { border-radius: 9px; overflow: hidden; }
.dashboard-media-type { position: absolute; z-index: 1; top: 8px; left: 8px; font-weight: 700; }
.dashboard-poster-overlay { position: absolute; inset: 0; display: flex; flex-direction: column; justify-content: flex-end; padding: 8px max(12px, var(--app-theme-surface-radius)) max(20px, var(--app-theme-surface-radius)); background: linear-gradient(rgba(var(--v-theme-surface), .94) 0%, rgba(var(--v-theme-surface), .98) 100%); opacity: 0; transition: opacity .18s ease; pointer-events: none; }
.dashboard-poster-overlay.is-visible, .dashboard-media-card:focus-visible .dashboard-poster-overlay, .dashboard-poster-fallback .dashboard-poster-overlay { opacity: 1; }
@media (hover: none) { .dashboard-poster-overlay { opacity: 1; } }
@media (prefers-reduced-motion: reduce) { .dashboard-poster-overlay { transition: none; } }
.dashboard-library-count { position: absolute; top: 6px; right: 6px; min-width: 24px; height: 24px; padding: 0 5px; border-radius: 50%; display: flex; align-items: center; justify-content: center; background: rgb(var(--v-theme-success)); color: rgb(var(--v-theme-on-success)); font-size: .75rem; font-weight: 600; font-variant-numeric: tabular-nums; }
.dashboard-poster-title { display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; overflow: hidden; overflow-wrap: anywhere; margin: 0 0 4px; color: rgb(var(--v-theme-on-surface)) !important; font-size: 18px; font-weight: 700; line-height: 1.5; }
.dashboard-poster-subtitle { color: rgb(var(--v-theme-on-surface)) !important; font-size: 14px; font-weight: 600; line-height: 1.5; }
.dashboard-library-title { padding: 8px 4px 4px; color: rgb(var(--v-theme-on-surface)); text-align: center; font-size: .75rem; font-weight: 700; line-height: 1.5; }
.dashboard-settings-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 10px; }
.dashboard-setting-item { appearance: none; position: relative; overflow: hidden; display: flex; align-items: center; gap: 8px; min-height: 48px; border: 1px solid rgba(var(--v-theme-on-surface), .1); border-radius: 10px; background: rgba(var(--v-theme-on-surface), .04); padding: 10px 12px; font: inherit; cursor: pointer; transition: border-color .2s ease, background-color .2s ease, transform .2s ease; }
.dashboard-setting-label { flex: 1; color: rgba(var(--v-theme-on-surface), .72); text-align: start; font-size: .9rem; font-weight: 550; line-height: 1.35; transition: color .2s ease; }
.dashboard-setting-item::before { position: absolute; content: ''; background: rgb(var(--v-theme-primary)); width: 3px; inset-block: 0; inset-inline-start: 0; opacity: 0; transition: opacity .2s ease; }
.dashboard-setting-item.enabled { border-color: rgba(var(--v-theme-primary), .3); background: rgba(var(--v-theme-primary), .08); }
.dashboard-setting-item.enabled::before { opacity: 1; }
.dashboard-setting-item.enabled .dashboard-setting-label { color: rgb(var(--v-theme-on-surface)); }
.dashboard-setting-item:hover { border-color: rgba(var(--v-theme-primary), .32); background: rgba(var(--v-theme-primary), .06); }
.dashboard-setting-item:active { transform: scale(.99); }
.dashboard-setting-item:focus-visible { outline: 3px solid rgba(var(--v-theme-primary), .28); outline-offset: 2px; }
@media (max-width: 599px) { .dashboard-settings-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
@container (max-width: 680px) {
  .dashboard-stat-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 20px 8px; }
  .dashboard-stat-item { padding: 0 8px; gap: 10px; }
  .dashboard-stat-item:nth-child(odd) { border-left: 0; padding-left: 0; }
}
@container (max-width: 280px) {
  .dashboard-stat-grid { grid-template-columns: minmax(0, 1fr); }
  .dashboard-stat-item { border-left: 0; padding: 0; }
}
@container (max-width: 160px) {
  .dashboard-stat-item :deep(.v-avatar) { display: none; }
  .dashboard-stat-item { gap: 0; }
}
@container (max-width: 380px) {
  .storage-icon { display: none; }
  .storage-number, .storage-caption { padding-right: 0; }
  .storage-number { overflow-wrap: anywhere; }
}
@container (max-width: 180px) { .storage-number { font-size: 1.25rem; } }
@container (max-width: 1440px) { .dashboard-library-row { grid-template-columns: repeat(6, minmax(0, 1fr)); } }
@container (max-width: 1190px) { .dashboard-library-row { grid-template-columns: repeat(5, minmax(0, 1fr)); } }
@container (max-width: 840px) { .dashboard-library-row { grid-template-columns: repeat(4, minmax(0, 1fr)); } }
@container (max-width: 600px) { .dashboard-library-row { grid-template-columns: repeat(3, minmax(0, 1fr)); } }
@container (max-width: 480px) { .dashboard-library-row { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
@container (max-width: 280px) { .dashboard-library-row { grid-template-columns: minmax(0, 1fr); } }
.dashboard-art-fallback { height: 100%; width: 100%; display: flex; align-items: center; justify-content: center; background: rgba(var(--v-theme-on-surface), 0.08); }
@media (max-width: 740px) {
  .dashboard-poster-grid { grid-template-columns: repeat(auto-fill, minmax(144px, 1fr)); gap: 16px; }
  .dashboard-library-row { gap: 12px; }
}
</style>
