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
    const { data } = await axios.get('/api/media_server/statistics');
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
  <VCard variant="flat" class="app-card-shell app-card-colorful mp-setting-card mp-setting-card--interactive emby-media-server-card"
    :class="{ 'mp-setting-card--inactive': !ready }"
    style="--app-card-accent-rgb:var(--v-theme-success)" role="button" tabindex="0"
    aria-label="编辑 Emby 媒体服务器" :aria-disabled="disabled" :disabled="disabled"
    @click="edit" @keydown.self.enter.prevent="edit" @keydown.self.space.prevent="edit">
    <div class="mp-setting-card__header">
      <div class="mp-setting-card__icon mp-setting-card__icon--logo" aria-hidden="true"><VImg :src="embyLogo" contain class="mp-setting-card__logo" /></div>
      <div class="mp-setting-card__heading">
        <div class="mp-setting-card__title-row"><span class="mp-setting-card__title">Emby</span></div>
        <div class="mp-setting-card__subtitle">
          <span class="mp-setting-card__subtitle-text">Emby</span>
          <span class="mp-setting-card__status" :class="ready ? 'mp-setting-card__status--active' : 'mp-setting-card__status--inactive'">
            <span class="mp-setting-card__status-dot" :class="ready ? 'mp-setting-card__status-dot--active' : 'mp-setting-card__status-dot--inactive'" />{{ ready ? '已配置' : '未配置' }}
          </span>
        </div>
      </div>
      <div class="mp-setting-card__actions" aria-hidden="true"><VIcon icon="mdi-pencil-outline" size="18" /></div>
    </div>
    <div class="mp-setting-card__dock">
      <div class="mp-setting-card__stats" :title="error || '媒体统计'">
        <span v-for="item in counts" :key="item.title" class="mp-setting-card__stat" :aria-label="`${item.title}数量 ${item.value}`" :title="`${item.title} ${item.value}`">
          <VIcon :icon="item.icon" size="14" /><span class="mp-setting-card__stat-value num">{{ item.value }}</span>
        </span>
      </div>
      <VIcon class="mp-setting-card__chevron" icon="mdi-chevron-right" size="16" />
    </div>
  </VCard>
</template>
