<template>
  <MainLayout v-if="!route.meta.public" :theme-mode="themeMode" :task-status="backgroundTaskStatus" @update:theme-mode="$emit('update:theme-mode', $event)" />
  <div v-else class="fullscreen-container"><router-view /></div>
</template>

<script setup>
import { useRoute } from 'vue-router';
import MainLayout from './MainLayout.vue';
import { useTaskStatus } from './composables/useTaskStatus';
defineProps({ themeMode: String });
defineEmits(['update:theme-mode']);
const route = useRoute();
const { backgroundTaskStatus } = useTaskStatus();
</script>
