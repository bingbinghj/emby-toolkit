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
function formatSpaceBytes(value, fallback) {
  if (value === null) return fallback;
  if (value === 0) return '0 bytes';
  const units = ['bytes', 'KB', 'MB', 'GB', 'TB', 'PB', 'EB', 'ZB', 'YB'];
  const unit = Math.max(0, Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1));
  return `${Number.parseFloat((value / 1024 ** unit).toFixed(1))} ${units[unit]}`;
}
function edit() { if (!props.disabled) emit('edit'); }
</script>

<template>
  <VCard variant="flat" class="app-card-shell app-card-colorful mp-setting-card mp-setting-card--interactive p115-storage-card"
    style="--app-card-accent-rgb:255,180,0" role="button" tabindex="0"
    aria-label="编辑 115 存储" :aria-disabled="disabled" :disabled="disabled"
    @click="edit" @keydown.self.enter.prevent="edit" @keydown.self.space.prevent="edit">
    <div class="mp-setting-card__header">
      <div class="mp-setting-card__icon mp-setting-card__icon--logo" aria-hidden="true"><VImg :src="storageLogo" contain class="mp-setting-card__logo" /></div>
      <div class="mp-setting-card__heading">
        <div class="mp-setting-card__title-row"><span class="mp-setting-card__title">115 网盘</span></div>
        <div class="mp-setting-card__subtitle"><span class="mp-setting-card__subtitle-text num">{{ space ? `${formatSpaceBytes(space.used, space.usedText)} / ${formatSpaceBytes(space.total, space.totalText)}` : '空间信息未知' }}</span></div>
      </div>
      <div class="mp-setting-card__actions" aria-hidden="true"><VIcon icon="mdi-pencil-outline" size="18" /></div>
    </div>
    <div class="mp-setting-card__dock">
      <div v-if="space?.percent != null" class="mp-setting-card__progress" :title="`已使用 ${space.percent}%`">
        <div class="mp-setting-card__progress-track"><span class="mp-setting-card__progress-fill" :class="`mp-setting-card__progress-fill--${progressColor}`" :style="{inlineSize:`${Math.min(100, Math.max(0,space.percent))}%`}" /></div>
        <span class="mp-setting-card__progress-pct num">{{ space.percent }}%</span>
      </div>
      <span v-else class="mp-setting-card__muted">空间信息未知</span>
      <VIcon class="mp-setting-card__chevron" icon="mdi-chevron-right" size="16" />
    </div>
  </VCard>
</template>
