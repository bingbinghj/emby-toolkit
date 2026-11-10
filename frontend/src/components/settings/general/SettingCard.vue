<script setup>
defineProps({
  title: { type: String, required: true },
  subtitle: { type: String, default: '' },
});
</script>

<template>
  <VCard class="setting-card">
    <VCardItem>
      <VCardTitle>{{ title }}</VCardTitle>
      <VCardSubtitle>{{ subtitle }}</VCardSubtitle>
      <div v-if="$slots.status" class="setting-card__status"><slot name="status" /></div>
      <template v-if="$slots.append" #append><slot name="append" /></template>
    </VCardItem>
    <div class="setting-card__content">
        <slot />
        <VCardActions v-if="$slots.actions" class="justify-end"><slot name="actions" /></VCardActions>
    </div>
  </VCard>
</template>

<style scoped>
.setting-card { min-width: 0; }
.setting-card :deep(.v-card-title), .setting-card :deep(.v-card-subtitle) { white-space: normal; }
.setting-card__status { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin-block-start: 6px; }
.setting-card__content :deep(.v-card-text) { padding-block-start: 8px; }
@media (max-width: 599px) {
  .setting-card :deep(.v-card-item) { align-items: start; flex-wrap: wrap; gap: 12px; }
  .setting-card :deep(.v-card-item__append) { padding-inline-start: 0; }
}
</style>
