<!-- MoviePilot-Frontend v3 609ea05 PageContentTitle (MIT); see ../layouts/LICENSE.MoviePilot. -->
<script setup>
defineProps({ title: { type: String, required: true }, subtitle: String });
</script>

<template>
  <div class="mp-page-title">
    <h1 class="mp-page-title__heading"><span class="text-moviepilot">{{ title }}</span></h1>
    <p v-if="subtitle" class="text-body-2 text-medium-emphasis mt-1 mb-0">{{ subtitle }}</p>
  </div>
</template>

<style scoped>
.mp-page-title { min-width: 0; }
.mp-page-title__heading { margin: 0; font-size: 30px; line-height: 36px; font-weight: 700; overflow-wrap: anywhere; }
@media (max-width: 599px) { .mp-page-title__heading { font-size: 24px; line-height: 28px; } }
</style>
