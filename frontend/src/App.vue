<template>
  <VApp class="etk-app">
    <WallpaperBackground />
    <MoviePilotGlassRuntime>
      <AppContent />
      <FeedbackHost />
      <GlassSettingsDialog v-model="glassSettingsOpen" @after-leave="restoreGlassSettingsFocus" />
    </MoviePilotGlassRuntime>
  </VApp>
</template>

<script setup>
import { onBeforeUnmount, onMounted, ref } from 'vue'
import AppContent from './AppContent.vue'
import FeedbackHost from './components/FeedbackHost.vue'
import WallpaperBackground from './components/WallpaperBackground.vue'
import MoviePilotGlassRuntime from './components/theme/MoviePilotGlassRuntime.vue'
import GlassSettingsDialog from './components/theme/GlassSettingsDialog.vue'
import { GLASS_SETTINGS_OPEN_EVENT } from './composables/useThemeCustomizer'

const glassSettingsOpen = ref(false)
let glassSettingsTrigger
function openGlassSettings() {
  if (glassSettingsOpen.value) return
  glassSettingsTrigger = document.activeElement instanceof HTMLElement ? document.activeElement : undefined
  glassSettingsOpen.value = true
}
function restoreGlassSettingsFocus() {
  if (glassSettingsOpen.value || !glassSettingsTrigger?.isConnected) return
  const focused = document.activeElement
  if (focused === document.body || focused?.closest('.v-dialog')) glassSettingsTrigger.focus({ preventScroll: true })
  glassSettingsTrigger = undefined
}
onMounted(() => window.addEventListener(GLASS_SETTINGS_OPEN_EVENT, openGlassSettings))
onBeforeUnmount(() => window.removeEventListener(GLASS_SETTINGS_OPEN_EVENT, openGlassSettings))
</script>
