<script setup>
defineProps({
  title: { type: String, required: true },
  subtitle: { type: String, default: '' },
  actionsStart: { type: Boolean, default: false },
});
</script>

<template>
  <VCard class="setting-card">
    <VCardItem>
      <VCardTitle>{{ title }}</VCardTitle>
      <VCardSubtitle>{{ subtitle }}</VCardSubtitle>
      <template v-if="$slots.append" #append><slot name="append" /></template>
    </VCardItem>
    <div class="setting-card__content">
        <slot />
        <VCardActions v-if="$slots.actions" :class="actionsStart ? 'justify-start' : 'justify-end'"><slot name="actions" /></VCardActions>
    </div>
  </VCard>
</template>

<style scoped>
.setting-card { min-width: 0; }
.setting-card :deep(.v-card-title), .setting-card :deep(.v-card-subtitle) { white-space: normal; }
.setting-card__content :deep(.v-card-text) { padding-block-start: 8px; }
@media (max-width: 599px) {
  .setting-card :deep(.v-card-item) { align-items: start; flex-wrap: wrap; gap: 12px; }
  .setting-card :deep(.v-card-item__append) { padding-inline-start: 0; }
}
</style>
