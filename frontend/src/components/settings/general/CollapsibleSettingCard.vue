<script setup>
import { ref, useId } from 'vue';

const props = defineProps({
  title: { type: String, required: true },
  subtitle: { type: String, default: '' },
  icon: { type: String, required: true },
  defaultCollapsed: { type: Boolean, default: true },
});
const collapsed = ref(props.defaultCollapsed);
const contentId = useId();
</script>

<template>
  <VCard variant="outlined" class="collapsible-setting-card">
    <div class="collapsible-setting-card__header">
      <button type="button" class="collapsible-setting-card__toggle" :aria-label="title" :aria-expanded="!collapsed" :aria-controls="contentId" @click="collapsed = !collapsed">
        <VAvatar color="primary" variant="tonal" size="40"><VIcon :icon="icon" /></VAvatar>
        <div class="collapsible-setting-card__copy">
          <div class="text-subtitle-1 font-weight-medium">{{ title }}</div>
          <div class="text-body-2 text-medium-emphasis">{{ subtitle }}</div>
          <div v-if="$slots.status" class="collapsible-setting-card__status"><slot name="status" /></div>
        </div>
        <VIcon :icon="collapsed ? 'mdi-chevron-down' : 'mdi-chevron-up'" color="primary" aria-hidden="true" />
      </button>
      <div v-if="$slots.append" class="collapsible-setting-card__append"><slot name="append" /></div>
    </div>
    <VExpandTransition>
      <div v-show="!collapsed" :id="contentId" class="collapsible-setting-card__content">
        <slot />
        <VCardActions v-if="$slots.actions" class="justify-end"><slot name="actions" /></VCardActions>
      </div>
    </VExpandTransition>
  </VCard>
</template>

<style scoped>
.collapsible-setting-card { border-color: rgba(var(--v-theme-primary), 0.15); }
.collapsible-setting-card__header { display: flex; align-items: center; gap: 12px; padding: 12px 16px; }
.collapsible-setting-card__toggle { display: flex; align-items: center; flex: 1; min-width: 0; gap: 12px; padding: 4px 0; border: 0; background: transparent; color: inherit; font: inherit; text-align: start; cursor: pointer; }
.collapsible-setting-card__toggle:focus-visible { outline: 2px solid rgba(var(--v-theme-primary), 0.8); outline-offset: 3px; border-radius: 4px; }
.collapsible-setting-card__copy { flex: 1; min-width: 0; overflow-wrap: anywhere; }
.collapsible-setting-card__status { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin-block-start: 6px; }
.collapsible-setting-card__append { flex-shrink: 0; }
.collapsible-setting-card__content :deep(.v-card-text) { padding-block-start: 8px; }
@media (max-width: 599px) {
  .collapsible-setting-card__header { flex-wrap: wrap; }
  .collapsible-setting-card__toggle { flex-basis: 100%; align-items: flex-start; }
  .collapsible-setting-card__append { margin-inline-start: 52px; }
}
</style>
