<script setup>
import { computed, ref, watch } from 'vue';
import { primaryImageUrl } from '@/utils/mediaImages';

const props = defineProps({ item: { type: Object, required: true }, title: String, width: Number, height: Number });
const candidates = computed(() => {
  const item = props.item;
  const series = item.item_type === 'Episode' && primaryImageUrl(item.series_id, item.series_image_tag);
  const file = primaryImageUrl(item.item_id, item.image_tag);
  // A receipt can predate both image tags. Emby can still serve Primary by ID.
  return (item.image_tag && !item.series_image_tag ? [file, series] : [series, file]).filter(Boolean);
});
const attempt = ref(0);
watch(() => candidates.value.join('|'), () => { attempt.value = 0; });
const src = computed(() => candidates.value[attempt.value]);
</script>

<template>
  <VAvatar rounded :width="width" :height="height" class="organize-poster">
    <VImg v-if="src" :key="src" :src="src" :alt="title" cover @error="attempt++" />
    <VIcon v-else :icon="item.item_type === 'Episode' ? 'mdi-television-classic' : 'mdi-movie-open-outline'" />
  </VAvatar>
</template>

<style scoped>
.organize-poster { flex: none; background: rgba(var(--v-theme-on-surface), 0.06); }
</style>
