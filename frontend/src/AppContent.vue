<template>
  <div class="etk-app-content">
    <MainLayout v-if="!route.meta.public" :task-status="backgroundTaskStatus" />
    <PublicLayout v-else><RouterView /></PublicLayout>
  </div>
</template>

<script setup>
import { useRoute } from 'vue-router';
import MainLayout from './MainLayout.vue';
import PublicLayout from './PublicLayout.vue';
import { useTaskStatus } from './composables/useTaskStatus';
const route = useRoute();
const { backgroundTaskStatus } = useTaskStatus();
</script>

<style scoped>
.etk-app-content { position: relative; isolation: isolate; min-height: 100dvh; }
</style>
