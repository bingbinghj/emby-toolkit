<!-- src/PublicLayout.vue -->
<template>
  <main class="public-layout"><slot><RouterView /></slot></main>
</template>

<script setup>
</script>

<style scoped>
.public-layout { position: relative; display: flex; align-items: center; justify-content: center; min-height: 100dvh; padding: 24px 16px; }
</style>
