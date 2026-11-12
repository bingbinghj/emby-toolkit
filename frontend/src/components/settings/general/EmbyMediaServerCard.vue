<script setup>
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import axios from 'axios';
import embyLogo from '@/assets/mp-emby.png';
import { formatDashboardCount } from '@/utils/dashboard';

// MoviePilot v3 MediaServerCard 的摘要布局；配置仍使用 ETK 的单 Emby 服务。
const props = defineProps({
  ready: { type: Boolean, default: false },
  disabled: { type: Boolean, default: false },
  refreshKey: { type: Number, default: 0 },
});
const emit = defineEmits(['edit']);
const statistics = ref(null), error = ref('');
let requestVersion = 0;
const counts = computed(() => [
  { icon: 'mdi-movie-roll', title: '电影', value: formatDashboardCount(statistics.value?.movie_count?.count) },
  { icon: 'mdi-television-box', title: '电视剧', value: formatDashboardCount(statistics.value?.tv_count?.count) },
]);
watch(() => [props.ready, props.refreshKey], async () => {
  const version = ++requestVersion;
  statistics.value = null;
  error.value = '';
  if (!props.ready) return;
  try {
    const { data } = await axios.get('/api/dashboard/media');
    if (version !== requestVersion) return;
    if (!data?.statistics || typeof data.statistics !== 'object') throw new Error('媒体统计响应无效');
    statistics.value = data.statistics;
    error.value = counts.value.some(item => item.value === '—') ? '部分媒体统计尚不可用' : '';
  } catch (failed) {
    if (version === requestVersion) error.value = failed.response?.data?.error || failed.message || '媒体统计读取失败';
  }
}, { immediate: true });
onBeforeUnmount(() => { requestVersion += 1; });
function edit() { if (!props.disabled) emit('edit'); }
</script>

<template>
  <VCard variant="tonal" class="app-card-shell app-card-colorful emby-media-server-card"
    style="--app-card-accent-rgb:var(--v-theme-success)" role="button" tabindex="0"
    aria-label="编辑 Emby 媒体服务器" :aria-disabled="disabled" :disabled="disabled"
    @click="edit" @keydown.self.enter.prevent="edit" @keydown.self.space.prevent="edit">
    <VIcon class="media-server-edit" icon="mdi-pencil-outline" size="18" aria-hidden="true" />
    <VCardText class="app-card-summary app-card-summary--single-action">
      <div class="app-card-summary__content">
        <div class="app-card-summary__title text-h6">Emby</div>
        <div class="media-server-counts text-sm text-medium-emphasis" :title="error || '媒体统计'">
          <span v-for="item in counts" :key="item.title" :aria-label="`${item.title}数量 ${item.value}`" :title="`${item.title} ${item.value}`">
            <VIcon :icon="item.icon" size="18" class="me-1" />
            <span class="media-server-count">{{ item.value }}</span>
          </span>
        </div>
      </div>
      <div class="app-card-summary__media" aria-hidden="true">
        <VImg :src="embyLogo" contain class="app-card-summary__image" />
      </div>
    </VCardText>
  </VCard>
</template>

<style scoped>
.emby-media-server-card { cursor:pointer; min-width:0; }
.media-server-edit { position:absolute; top:12px; right:12px; color:var(--ui-text-muted); }
.media-server-counts { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:8px; min-height:24px; }
.media-server-counts > span { display:flex; align-items:center; min-width:0; }
.media-server-count { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
html[data-theme='glass'][data-glass-color-scheme='dark'] .app-card-summary { background-color:rgba(var(--v-theme-surface),0.2); }
html[data-theme='glass'][data-glass-color-scheme='dark'] .media-server-count { color:rgb(var(--v-theme-on-surface)); }
</style>
