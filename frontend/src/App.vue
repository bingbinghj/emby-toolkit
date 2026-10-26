<template>
  <n-config-provider :theme="isDark ? darkTheme : undefined" :theme-overrides="theme" :locale="zhCN" :date-locale="dateZhCN">
    <div class="mp-wallpaper" aria-hidden="true">
      <img v-if="wallpaperUrl && !wallpaperFailed" :key="wallpaperUrl" :src="wallpaperUrl" alt="" @error="wallpaperFailed = true" />
    </div>
    <n-message-provider placement="bottom-right">
      <n-dialog-provider>
        <AppContent />
      </n-dialog-provider>
    </n-message-provider>
  </n-config-provider>
</template>

<script setup>
import { ref, computed, watch, onUnmounted } from 'vue';
import { NConfigProvider, NMessageProvider, NDialogProvider, darkTheme, zhCN, dateZhCN } from 'naive-ui';
import AppContent from './AppContent.vue';
import { moviePilotTheme, resolveTheme, applyDocumentTheme } from './theme';
import { useAppearance } from './composables/useAppearance';

const systemTheme = window.matchMedia('(prefers-color-scheme: dark)');
const prefersDark = ref(systemTheme.matches);
const { settings } = useAppearance();
const resolvedTheme = computed(() => resolveTheme(settings.theme, prefersDark.value));
const isDark = computed(() => resolvedTheme.value === 'dark');
const theme = computed(() => moviePilotTheme(resolvedTheme.value, prefersDark.value, settings));
const wallpaperUrl = computed(() => settings.wallpaperMode === 'custom' ? settings.wallpaper : '');
const wallpaperFailed = ref(false);
watch(wallpaperUrl, () => { wallpaperFailed.value = false; });
const updateSystemTheme = event => { prefersDark.value = event.matches; };
systemTheme.addEventListener('change', updateSystemTheme);
onUnmounted(() => systemTheme.removeEventListener('change', updateSystemTheme));
watch([settings, prefersDark], () => applyDocumentTheme(settings, prefersDark.value), { immediate: true, deep: true });
</script>
