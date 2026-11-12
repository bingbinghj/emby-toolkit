<script setup>
import { computed } from 'vue';
import storageLogo from '@/assets/mp-u115.png';
import { storageFrom115 } from '@/utils/dashboard';

const props = defineProps({
  info: { type: Object, default: null },
  disabled: { type: Boolean, default: false },
});
const emit = defineEmits(['edit']);
const space = computed(() => {
  try { return storageFrom115({ data: props.info }); }
  catch { return null; }
});
const progressColor = computed(() => space.value?.percent > 90 ? 'error' : space.value?.percent > 70 ? 'warning' : 'success');
function edit() { if (!props.disabled) emit('edit'); }
</script>

<template>
  <VCard variant="tonal" class="app-card-shell app-card-colorful p115-storage-card"
    style="--app-card-accent-rgb:255,180,0" role="button" tabindex="0"
    aria-label="编辑 115 存储" :aria-disabled="disabled" :disabled="disabled"
    @click="edit" @keydown.self.enter.prevent="edit" @keydown.self.space.prevent="edit">
    <VIcon class="storage-edit" icon="mdi-pencil-outline" size="18" aria-hidden="true" />
    <VCardText class="storage-summary d-flex justify-space-between align-center ga-3">
      <div class="storage-summary-content align-self-start">
        <h5 class="text-h6 mb-1">115 网盘</h5>
        <div class="text-sm storage-space">{{ space ? `${space.usedText} / ${space.totalText}` : '空间信息未知' }}</div>
      </div>
      <VImg :src="storageLogo" contain class="storage-logo mt-8" aria-hidden="true" />
    </VCardText>
    <VProgressLinear v-if="space?.percent !== null && space?.percent !== undefined" class="storage-progress"
      :model-value="space.percent" :bg-color="progressColor" :color="progressColor" />
  </VCard>
</template>

<style scoped>
.p115-storage-card { cursor:pointer; min-width:0; }
.storage-summary { min-height:124px; padding-bottom:24px; }
.storage-summary-content { flex:1; min-width:0; }
.storage-space { overflow-wrap:anywhere; }
.storage-logo { flex:0 0 3rem; max-width:3rem; min-width:3rem; }
.storage-edit { position:absolute; top:12px; right:12px; color:var(--ui-text-muted); }
.storage-progress { position:absolute; bottom:0; left:0; width:100%; }
html[data-theme='glass'][data-glass-color-scheme='dark'] .storage-summary { background-color:rgba(var(--v-theme-surface),0.2); }
</style>
