// MoviePilot-Frontend v3 browser theme integration (MIT, see third-party/MoviePilot-LICENSE).
import { watch } from 'vue'
import vuetify from './plugins/vuetify'
import {
  applyStoredThemeCustomizerAppearance,
  themeCustomizerSettings,
  THEME_CUSTOMIZER_STORAGE_KEY,
} from './composables/useThemeCustomizer'
import { applyDocumentThemeChrome, applyGlassThemePalette } from './utils/themePalette'
export { textColorForBackground } from './utils/colorContrast'

// Existing ETK business styles consume semantic tokens; values now come from Vuetify.
function syncBusinessColors() {
  const theme = vuetify.theme.current.value
  const colors = theme.colors
  const root = document.documentElement
  const color = name => colors[name] || colors['on-surface']
  const variables = {
    '--ui-page-bg': color('background'),
    '--ui-surface': color('surface'),
    '--ui-surface-muted': color('surface-variant'),
    '--ui-text': color('on-surface'),
    '--ui-text-muted': color('on-surface-variant'),
    '--ui-border': `rgba(${theme.dark ? '255,255,255' : '60,60,67'},.14)`,
    '--ui-border-soft': `rgba(${theme.dark ? '255,255,255' : '60,60,67'},.08)`,
    '--accent-color': color('primary'),
    '--ui-on-primary': color('on-primary'),
    '--ui-primary-text': color('primary'),
    '--ui-primary-solid': color('primary'),
    '--ui-success-color': color('success'),
    '--ui-success-text': color('success'),
    '--ui-warning-color': color('warning'),
    '--ui-warning-text': color('warning'),
    '--ui-error-color': color('error'),
    '--ui-error-text': color('error'),
    '--ui-info-color': color('info'),
    '--ui-info-text': color('info'),
    '--ui-radius': 'var(--app-surface-radius)',
    '--ui-radius-sm': 'var(--app-control-radius)',
    '--ui-overlay-radius': 'var(--app-overlay-radius)',
    '--ui-input-bg': color('surface'),
    '--ui-input-border': `rgba(${theme.dark ? '255,255,255' : '60,60,67'},.22)`,
    '--modal-solid-bg-color': color('surface'),
  }
  Object.entries(variables).forEach(([name, value]) => root.style.setProperty(name, value))
  root.classList.toggle('dark', theme.dark)
  root.classList.toggle('light', !theme.dark)
}

let disposeThemeRuntime

/** Call before app.mount; returns cleanup for HMR/application disposal. */
export function initializeThemeRuntime() {
  if (disposeThemeRuntime) return disposeThemeRuntime
  applyStoredThemeCustomizerAppearance(vuetify.theme)
  syncBusinessColors()
  const media = window.matchMedia?.('(prefers-color-scheme: dark)')
  const syncSystemTheme = () => {
    const settings = themeCustomizerSettings.value
    if (settings.theme !== 'auto') return
    applyGlassThemePalette(vuetify.theme, 'auto')
    applyDocumentThemeChrome('auto', {
      glassEnabled: settings.glassEnabled,
      primary: settings.primaryColor,
      persistLoaderColors: true,
    })
    syncBusinessColors()
  }
  const syncStorage = event => {
    if (event.key && event.key !== THEME_CUSTOMIZER_STORAGE_KEY && event.key !== 'theme') return
    applyStoredThemeCustomizerAppearance(vuetify.theme)
    syncBusinessColors()
  }
  media?.addEventListener('change', syncSystemTheme)
  window.addEventListener('storage', syncStorage)
  const stop = watch([themeCustomizerSettings, vuetify.theme.current], syncBusinessColors, { deep: true })
  disposeThemeRuntime = () => {
    stop()
    media?.removeEventListener('change', syncSystemTheme)
    window.removeEventListener('storage', syncStorage)
    disposeThemeRuntime = undefined
  }
  return disposeThemeRuntime
}
