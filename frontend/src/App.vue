<template>
  <n-config-provider :theme="isDark ? darkTheme : undefined" :theme-overrides="theme" :locale="zhCN" :date-locale="dateZhCN">
    <n-message-provider placement="bottom-right">
      <n-dialog-provider>
        <AppContent :theme-mode="themeMode" @update:theme-mode="setThemeMode" />
      </n-dialog-provider>
    </n-message-provider>
  </n-config-provider>
</template>

<script setup>
import { ref, computed, watch, onUnmounted } from 'vue';
import { NConfigProvider, NMessageProvider, NDialogProvider, darkTheme, zhCN, dateZhCN } from 'naive-ui';
import AppContent from './AppContent.vue';
import { moviePilotTheme, themeOptions, themePalettes, resolveTheme } from './theme';

const systemTheme = window.matchMedia('(prefers-color-scheme: dark)');
const prefersDark = ref(systemTheme.matches);
const saved = localStorage.getItem('moviepilot-theme');
const legacy = localStorage.getItem('isDark');
const themeMode = ref(themeOptions.some(option => option.key === saved) ? saved : (legacy === null ? 'system' : legacy === 'true' ? 'dark' : 'light'));
const resolvedTheme = computed(() => resolveTheme(themeMode.value, prefersDark.value));
const isDark = computed(() => resolvedTheme.value !== 'light');
const theme = computed(() => moviePilotTheme(resolvedTheme.value));
function setThemeMode(value) {
  if (themeOptions.some(option => option.key === value)) {
    themeMode.value = value;
    localStorage.setItem('moviepilot-theme', value);
  }
}
const updateSystemTheme = event => { prefersDark.value = event.matches; };
systemTheme.addEventListener('change', updateSystemTheme);
onUnmounted(() => systemTheme.removeEventListener('change', updateSystemTheme));
watch(resolvedTheme, mode => {
  const root = document.documentElement;
  const palette = themePalettes[mode];
  root.dataset.theme = mode;
  root.classList.toggle('dark', isDark.value);
  root.classList.toggle('light', !isDark.value);
  root.style.colorScheme = isDark.value ? 'dark' : 'light';
  const variables = {
    '--ui-page-bg': palette.background, '--ui-surface': palette.surface, '--ui-surface-muted': palette.mutedSurface,
    '--ui-text': palette.text, '--ui-text-muted': palette.muted, '--ui-border': palette.border,
    '--ui-border-soft': palette.borderSoft, '--ui-border-strong': palette.inputBorder, '--ui-input-border': palette.inputBorder,
    '--ui-input-bg': palette.input, '--accent-color': palette.primary, '--accent-glow-color': `color-mix(in srgb, ${palette.primary} 14%, transparent)`,
    '--modal-solid-bg-color': palette.popup,
    '--ui-error-color': palette.error,
    '--ui-warning-color': palette.warning,
    '--ui-info-color': palette.info,
    '--ui-success-color': palette.success,
  };
  Object.entries(variables).forEach(([key, value]) => root.style.setProperty(key, value));
}, { immediate: true });
</script>
