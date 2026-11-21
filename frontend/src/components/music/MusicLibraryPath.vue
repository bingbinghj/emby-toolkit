<script setup>
import { computed } from 'vue';
const props = defineProps({ directory: String, loading: Boolean });
const emit = defineEmits(['navigate']);
const crumbs = computed(() => (props.directory || '').split('/').filter(Boolean).map((title, i, parts) => ({title, path:parts.slice(0,i+1).join('/')})));
</script>
<template>
  <VCard class="music-library-path mb-4" data-testid="music-library-path">
    <VBtn icon="mdi-arrow-up" variant="text" aria-label="上级目录" :disabled="!directory || loading" @click="emit('navigate', directory.split('/').slice(0,-1).join('/'))" />
    <div class="library-location">
      <nav class="file-breadcrumbs" aria-label="音乐目录"><VBtn variant="text" prepend-icon="mdi-home-outline" :disabled="loading" @click="emit('navigate', '')">音乐库</VBtn><template v-for="crumb in crumbs" :key="crumb.path"><VIcon icon="mdi-chevron-right" size="16" /><VBtn variant="text" :disabled="loading" @click="emit('navigate', crumb.path)">{{ crumb.title }}</VBtn></template></nav>
    </div>
  </VCard>
</template>
<style scoped>
.music-library-path{display:flex;align-items:center;gap:.5rem;padding:.5rem .75rem;min-inline-size:0;}.library-location{min-inline-size:0;flex:1;}.file-breadcrumbs{display:flex;align-items:center;flex-wrap:wrap;gap:0;}.file-breadcrumbs :deep(.v-btn){min-inline-size:0;max-inline-size:100%;height:auto;min-height:36px;padding-inline:.5rem;}.file-breadcrumbs :deep(.v-btn__content){white-space:normal;overflow-wrap:anywhere;}
</style>
