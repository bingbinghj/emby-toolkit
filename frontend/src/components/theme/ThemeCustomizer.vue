<script setup>
import ThemeCustomizer from '../ThemeCustomizer.vue'
defineOptions({ inheritAttrs: false })
defineEmits(['close'])
</script>

<template>
  <ThemeCustomizer v-bind="$attrs" @close="$emit('close')"><slot /></ThemeCustomizer>
</template>
