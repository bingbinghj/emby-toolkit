<template>
  <MainLayout v-if="!route.meta.public" :task-status="backgroundTaskStatus" />
  <div v-else class="fullscreen-container"><router-view /></div>
</template>

<script setup>
import { useRoute } from 'vue-router';
import MainLayout from './MainLayout.vue';
import { useTaskStatus } from './composables/useTaskStatus';
const route = useRoute();
const { backgroundTaskStatus } = useTaskStatus();
</script>
