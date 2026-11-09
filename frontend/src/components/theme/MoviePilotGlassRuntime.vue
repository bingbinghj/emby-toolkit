<!-- Browser integration of MoviePilot-Frontend v3 609ea05 glass runtime (MIT). -->
<script setup lang="ts">
import { computed, defineAsyncComponent, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useTheme } from 'vuetify'
import { useEffectiveGlassSettings } from '@/composables/useThemeCustomizer'
import { useWallpaper } from '@/composables/useWallpaper'
import { usePagePresentationMotion } from '@/composables/usePagePresentationMotion'
import {
  provideGlassFixedShellBackplate,
} from '@/composables/useGlassFixedShellBackplate'
import { getLoginGlassOpticalSettings } from '@/utils/loginPresentation'
import { normalizeThemeMaterialAccent } from '@/utils/glassColor'

const GlassOpticalLayer = defineAsyncComponent(() => import('./GlassOpticalLayer.vue'))
const route = useRoute()
const router = useRouter()
const theme = useTheme()
const glass = useEffectiveGlassSettings()
const wallpaper = useWallpaper()
const motion = usePagePresentationMotion()
const routeReady = ref(false)
const enabled = computed(() => theme.global.name.value === 'glass')
const backdropEnabled = computed(() => enabled.value && theme.global.current.value.dark)
const optical = computed(() => {
  const settings = glass.value
  const preference = {
    appearance: settings.glassAppearance,
    deformationStrength: settings.glassDeformationStrength,
    flowStrength: settings.glassFlowStrength,
    preset: settings.glassPreset,
    reflectionStrength: settings.glassReflectionStrength,
    transmissionStrength: settings.glassTransmissionStrength,
    translationStrength: settings.glassTranslationStrength,
    transparencyStrength: settings.glassTransparencyStrength,
  }
  return route.path === '/login'
    ? getLoginGlassOpticalSettings(preference)
    : { ...preference, quality: settings.glassQuality }
})
const activeImage = computed(() => backdropEnabled.value ? wallpaper.activeImage.value : '')
const previousImage = computed(() => backdropEnabled.value ? wallpaper.previousImage.value : '')
const shouldRender = computed(() => enabled.value && optical.value.quality !== 'css' && Boolean(activeImage.value))
const tintColor = computed(() => normalizeThemeMaterialAccent(theme.global.current.value.colors.primary)?.hex ?? '#8D51F9')
const layers = computed(() => activeImage.value ? [
  { key: 'front' as const, role: 'active' as const, url: activeImage.value, src: activeImage.value, style: {} },
  { key: 'back' as const, role: previousImage.value ? 'previous' as const : 'standby' as const,
    url: previousImage.value, src: previousImage.value, style: {} },
] : [])
provideGlassFixedShellBackplate({ layers, transitionDurationMs: 900 })

watch(() => route.fullPath, path => motion.start(path), { flush: 'post' })
onMounted(() => {
  void router.isReady().catch(error => console.warn('[Glass] Route initialization failed', error)).finally(() => {
    routeReady.value = true
    motion.start(route.fullPath)
  })
})
onBeforeUnmount(() => motion.cancel())
</script>

<template>
  <GlassOpticalLayer
    v-if="shouldRender"
    :appearance="optical.appearance"
    :deformation-strength="optical.deformationStrength"
    :flow-strength="optical.flowStrength"
    :reflection-strength="optical.reflectionStrength"
    :transmission-strength="optical.transmissionStrength"
    :translation-strength="optical.translationStrength"
    :transparency-strength="optical.transparencyStrength"
    :dynamics-mode="glass.glassDynamicsMode"
    :quality="optical.quality === 'high' ? 'high' : 'balanced'"
    :route-key="route.fullPath"
    :presentation-ready="routeReady && wallpaper.ready.value"
    :tint-color="tintColor"
    :transition-duration="900"
    :transition-started-at="wallpaper.transitionStartedAt.value"
    :wallpaper-url="activeImage"
    :previous-wallpaper-url="previousImage"
  />
  <slot />
</template>
