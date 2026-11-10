<script setup>
import { settingsCategoryNames } from '@/utils/settingsSearch';
defineProps({ query: { type: String, default: '' }, results: { type: Array, default: () => [] }, notice: { type: String, default: '' } });
defineEmits(['select']);
</script>

<template>
  <VCard v-if="query.trim()" class="settings-search-results mb-6" aria-label="设置搜索结果">
    <VCardTitle class="text-subtitle-1" role="status">{{ results.length ? `找到 ${results.length} 项设置` : '没有匹配的设置项' }}</VCardTitle>
    <VList v-if="results.length" class="settings-search-list">
      <VListItem v-for="entry in results" :key="`${entry.route}:${entry.field}`" :title="entry.label" :subtitle="`${settingsCategoryNames[entry.category]} / ${entry.card}`" :aria-label="`打开设置：${entry.label}`" tag="button" @click="$emit('select', entry)">
        <template #append><VIcon icon="mdi-chevron-right" /></template>
      </VListItem>
    </VList>
    <VCardText v-else>尝试搜索设置名称、说明或分类。</VCardText>
    <VAlert v-if="notice" type="info" variant="tonal" class="ma-4" role="status">{{ notice }}</VAlert>
  </VCard>
</template>

<style scoped>
.settings-search-list { max-height: 360px; overflow-y: auto; }
.settings-search-list :deep(.v-list-item) { width: 100%; text-align: start; }
.settings-search-list :deep(.v-list-item-title), .settings-search-list :deep(.v-list-item-subtitle) { white-space: normal; overflow-wrap: anywhere; }
.settings-search-list :deep(.v-list-item-subtitle) { opacity: 1; color: var(--ui-text-muted); }
</style>

<style>
.settings-search-highlight { outline: 2px solid rgb(var(--v-theme-primary)); outline-offset: 4px; border-radius: 4px; scroll-margin-block-start: 96px; }
</style>
