<script setup>
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useMessage } from '@/composables/useFeedback';
import { preloadWallpaperImage, useWallpaper } from '@/composables/useWallpaper';
import { useAuthStore } from '@/stores/auth';
import { useEffectiveGlassSettings } from '@/composables/useThemeCustomizer';
import { loadGlassWallpaperToneProfile } from '@/utils/glassWallpaperTone';

const { settings, images, activeImage, previousImage, activeExposure, previousExposure, crossfading, setPresentation, refreshSource, cancelSourceRequest } = useWallpaper();
const auth = useAuthStore();
const glass = useEffectiveGlassSettings();
const materialExposure = computed(() => glass.value.glassAppearance === 'frosted' ? 0.82 : glass.value.glassAppearance === 'tinted' ? 0.85 : 0.86);
const layerStyle = exposure => ({ '--glass-wallpaper-brightness': String(materialExposure.value * exposure) });
const message = useMessage();
const motion = window.matchMedia('(prefers-reduced-motion: reduce)');
const reducedMotion = ref(motion.matches);
let generation = 0, rotationTimer, fadeTimer;

function clearTimers() {
  window.clearTimeout(rotationTimer);
  window.clearTimeout(fadeTimer);
}

function scheduleRotation(revision) {
  window.clearTimeout(rotationTimer);
  const dynamicApi = settings.mode === 'remote' && settings.source === 'api';
  if (revision !== generation || !settings.rotationInterval || reducedMotion.value || document.hidden || images.value.length < 2 && !dynamicApi) return;
  rotationTimer = window.setTimeout(async () => {
    if (dynamicApi) {
      await refreshSource();
      if (revision === generation) scheduleRotation(revision);
      return;
    }
    const next = (images.value.indexOf(activeImage.value) + 1) % images.value.length;
    void showImage(next, revision, true);
  }, settings.rotationInterval * 1000);
}

async function showImage(start, revision, rotating = false) {
  const candidates = [...images.value];
  for (let offset = 0; offset < candidates.length; offset++) {
    if (revision !== generation || rotating && (reducedMotion.value || document.hidden)) return;
    const source = candidates[(start + offset) % candidates.length];
    try { await preloadWallpaperImage(source); } catch { continue; }
    const profile = await loadGlassWallpaperToneProfile(source);
    if (revision !== generation || rotating && (reducedMotion.value || document.hidden)) return;
    if (source !== activeImage.value) {
      window.clearTimeout(fadeTimer);
      const previous = reducedMotion.value ? '' : activeImage.value;
      setPresentation({ previousImage: previous, previousExposure: activeExposure.value, activeImage: source, activeExposure: profile.exposure, crossfading: !!previous, transitionStartedAt: performance.now(), ready: true });
      if (previous) fadeTimer = window.setTimeout(() => {
        if (revision === generation) setPresentation({ previousImage: '', crossfading: false });
      }, 900);
    } else setPresentation({ activeExposure: profile.exposure, ready: true });
    scheduleRotation(revision);
    return;
  }
  if (revision === generation) {
    setPresentation({ ready: true });
    message.warning('壁纸加载失败，已保留当前背景，请检查图片后重新选择。');
  }
}

watch(images, () => {
  const revision = ++generation;
  clearTimers();
  setPresentation({ previousImage: '', crossfading: false, ready: false });
  if (!images.value.length) {
    setPresentation({ ...(settings.mode === 'remote' ? {} : { activeImage: '', transitionStartedAt: 0 }), ready: true });
    return;
  }
  void showImage(0, revision);
}, { immediate: true });
watch(() => settings.rotationInterval, () => scheduleRotation(generation));
watch(() => [settings.mode, settings.source, settings.customApi, auth.isLoggedIn], () => {
  if (settings.mode === 'remote') void refreshSource();
  else cancelSourceRequest();
}, { immediate: true });

function motionChanged(event) {
  reducedMotion.value = event.matches;
  if (event.matches) {
    clearTimers();
    if (settings.mode === 'remote' && settings.source === 'api') cancelSourceRequest();
    setPresentation({ previousImage: '', crossfading: false });
  } else scheduleRotation(generation);
}

function visibilityChanged() {
  if (document.hidden) {
    window.clearTimeout(rotationTimer);
    if (settings.mode === 'remote' && settings.source === 'api') cancelSourceRequest();
  }
  else scheduleRotation(generation);
}

onMounted(() => {
  motion.addEventListener('change', motionChanged);
  document.addEventListener('visibilitychange', visibilityChanged);
});
onBeforeUnmount(() => {
  generation++;
  cancelSourceRequest();
  clearTimers();
  motion.removeEventListener('change', motionChanged);
  document.removeEventListener('visibilitychange', visibilityChanged);
});
</script>

<template>
  <div class="wallpaper-background" aria-hidden="true">
    <div v-if="previousImage" class="wallpaper-layer wallpaper-layer-previous" :style="layerStyle(previousExposure)">
      <img :src="previousImage" alt="" class="wallpaper-image" />
    </div>
    <div v-if="activeImage" :key="activeImage" class="wallpaper-layer wallpaper-layer-active" :class="{ 'wallpaper-layer-enter': crossfading }" :style="layerStyle(activeExposure)">
      <img :src="activeImage" alt="" class="wallpaper-image" />
    </div>
  </div>
</template>

<style scoped>
.wallpaper-background { position: fixed; inset: 0; z-index: -1; pointer-events: none; overflow: hidden; background: rgb(var(--v-theme-background)); }
.wallpaper-layer { position: absolute; inset: 0; }
.wallpaper-layer::after { position: absolute; inset: 0; content: ''; background: linear-gradient(rgba(0, 0, 0, .3), rgba(0, 0, 0, .6)); }
.wallpaper-image { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; }
.wallpaper-layer-active { --wallpaper-layer-opacity: 1; opacity: var(--wallpaper-layer-opacity); }
.wallpaper-layer-enter { animation: wallpaper-enter 900ms ease-out both; }
:global(html[data-theme='glass'] .wallpaper-layer) { filter: brightness(var(--glass-wallpaper-brightness, .6192)) saturate(.95) contrast(1.02); }
:global(html[data-theme='glass'] .wallpaper-layer-active) { --wallpaper-layer-opacity: .94; }
:global(html[data-theme='glass'] .wallpaper-layer::after) { background: radial-gradient(circle at 50% 18%, transparent 24%, rgba(var(--glass-wallpaper-shade-rgb, 6, 10, 19), .12) 100%), linear-gradient(rgba(var(--glass-wallpaper-shade-rgb, 6, 10, 19), .1), rgba(var(--glass-wallpaper-shade-rgb, 6, 10, 19), .3)); }
:global(html[data-theme='glass'][data-glass-appearance='tinted'] .wallpaper-layer) { filter: brightness(var(--glass-wallpaper-brightness, .612)) saturate(.97) contrast(1.02); }
:global(html[data-theme='glass'][data-glass-appearance='tinted'] .wallpaper-layer-active) { --wallpaper-layer-opacity: .93; }
:global(html[data-theme='glass'][data-glass-appearance='tinted'] .wallpaper-layer::after) { background: radial-gradient(circle at 50% 18%, transparent 22%, rgba(var(--glass-wallpaper-shade-rgb, 6, 10, 19), .14) 100%), linear-gradient(rgba(var(--glass-wallpaper-shade-rgb, 6, 10, 19), .1), rgba(var(--glass-wallpaper-shade-rgb, 6, 10, 19), .32)), rgba(var(--glass-material-accent-rgb), .03); }
:global(html[data-theme='glass'][data-glass-appearance='frosted'] .wallpaper-layer) { filter: brightness(var(--glass-wallpaper-brightness, .5904)) saturate(.9); }
:global(html[data-theme='glass'][data-glass-appearance='frosted'] .wallpaper-layer-active) { --wallpaper-layer-opacity: .92; }
:global(html[data-theme='glass'][data-glass-appearance='frosted'] .wallpaper-layer::after) { background: linear-gradient(rgba(var(--glass-wallpaper-shade-rgb, 6, 10, 19), .24), rgba(var(--glass-wallpaper-shade-rgb, 6, 10, 19), .48)), rgba(var(--glass-material-rgb, 28, 28, 30), .08); }
:global(html[data-glass-color-scheme='light'] .wallpaper-layer) { visibility: hidden; }
@keyframes wallpaper-enter { from { opacity: 0; } to { opacity: var(--wallpaper-layer-opacity); } }
@media (prefers-reduced-motion: reduce) { .wallpaper-layer-enter { animation: none; } }
</style>
