<!-- MoviePilot DashboardRecentImports.vue (MIT), frontend/third-party/MoviePilot-LICENSE -->
<script setup>
import { onMounted, ref } from 'vue';
import axios from 'axios';
import OrganizePoster from './OrganizePoster.vue';

const items = ref(null), error = ref(''), loading = ref(false);
async function load() {
  loading.value = true;
  try {
    const { data } = await axios.get('/api/media_organize/history', { params: { page: 1, page_size: 10, status: 'admitted' } });
    if (!Array.isArray(data?.items)) throw new Error('整理记录响应无效');
    items.value = data.items;
    error.value = '';
  } catch (e) { error.value = e.response?.data?.error || e.message; }
  finally { loading.value = false; }
}
onMounted(load);
</script>
<template>
  <VCardText class="recent-import-list">
    <VProgressLinear v-if="loading" indeterminate />
    <VAlert v-if="error" type="warning" variant="tonal">{{ error }}<VBtn variant="text" @click="load">重试</VBtn></VAlert>
    <div v-for="row in items" :key="row.id" class="recent-import-row">
      <OrganizePoster :item="row" :title="row.series_name || row.name" :width="40" :height="54" />
      <div><strong>{{ row.series_name || row.name }}</strong><div class="text-caption">{{ row.item_type === 'Movie' ? '电影' : '电视剧' }}<template v-if="row.episode_number != null"> · 第 {{ row.episode_number }} 集</template></div></div>
      <time class="text-caption" :datetime="row.created_at">{{ new Date(row.created_at).toLocaleDateString('zh-CN') }}</time>
      <VIcon icon="mdi-check-circle" color="success" size="16" />
    </div>
    <p v-if="items?.length === 0">暂无整理入库记录</p>
  </VCardText>
</template>
<style scoped>
.recent-import-list { max-block-size: 300px; overflow-y: auto; }
.recent-import-row { display: grid; grid-template-columns: 40px minmax(0, 1fr) auto 16px; gap: 12px; align-items: center; padding-block: 6px; overflow-wrap: anywhere; }
.recent-import-row + .recent-import-row { border-block-start: 1px solid rgba(var(--v-border-color), var(--v-border-opacity)); }
</style>
