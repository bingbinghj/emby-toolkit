<script setup>
import { onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useMessage } from '@/composables/useFeedback';
import { preloadWallpaperImage, useWallpaper } from '@/composables/useWallpaper';
import { useAuthStore } from '@/stores/auth';

const { settings, images, activeImage, previousImage, crossfading, setPresentation, refreshSource, cancelSourceRequest } = useWallpaper();
const auth = useAuthStore();
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
    if (revision !== generation || rotating && (reducedMotion.value || document.hidden)) return;
    if (source !== activeImage.value) {
      window.clearTimeout(fadeTimer);
      const previous = reducedMotion.value ? '' : activeImage.value;
      setPresentation({ previousImage: previous, activeImage: source, crossfading: !!previous, transitionStartedAt: performance.now(), ready: true });
      if (previous) fadeTimer = window.setTimeout(() => {
        if (revision === generation) setPresentation({ previousImage: '', crossfading: false });
      }, 900);
    } else setPresentation({ ready: true });
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
    <img v-if="previousImage" :src="previousImage" alt="" class="wallpaper-image" />
    <img v-if="activeImage" :key="activeImage" :src="activeImage" alt="" class="wallpaper-image" :class="{ 'wallpaper-image-enter': crossfading }" />
  </div>
</template>

<style scoped>
.wallpaper-background { position: fixed; inset: 0; z-index: -1; pointer-events: none; overflow: hidden; background: rgb(var(--v-theme-background)); }
.wallpaper-image { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; }
.wallpaper-image-enter { animation: wallpaper-enter 900ms ease-out both; }
@keyframes wallpaper-enter { from { opacity: 0; } to { opacity: 1; } }
@media (prefers-reduced-motion: reduce) { .wallpaper-image-enter { animation: none; } }
</style>
