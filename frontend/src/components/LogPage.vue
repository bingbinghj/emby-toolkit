<template>
  <NotificationCenter :task="backgroundTaskStatus" standalone />
</template>

<script setup>
import NotificationCenter from './NotificationCenter.vue';
import { useTaskStatus } from '@/composables/useTaskStatus';
const { backgroundTaskStatus } = useTaskStatus();
</script>
