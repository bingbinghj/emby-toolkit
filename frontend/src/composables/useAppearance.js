// Compatibility entry for ETK callers; MoviePilot's shared theme state is authoritative.
import { computed } from 'vue'
import {
  getDefaultGlassCustomizerSettings,
  normalizeThemeCustomizerSettings,
  persistPartialThemeCustomizerSettings,
  readThemeCustomizerSettings,
  themeCustomizerPrimaryColors,
  themeCustomizerSettings,
  THEME_CUSTOMIZER_STORAGE_KEY,
} from './useThemeCustomizer'
import { normalizeThemePreference } from '../utils/themePalette'
import { persistThemeSettings } from '../utils/themeSettingsPersistence'

export const MAX_WALLPAPER_LENGTH = 2 * 1024 * 1024
export const primaryColors = themeCustomizerPrimaryColors
export const appearanceDefaults = Object.freeze({
  ...getDefaultGlassCustomizerSettings(),
  theme: 'auto',
  primaryColor: themeCustomizerPrimaryColors[0].value,
  radius: 'default',
  layout: 'vertical',
  glassEnabled: true,
})
export const normalizeThemeMode = normalizeThemePreference
export const normalizeAppearance = normalizeThemeCustomizerSettings
export const readSettings = readThemeCustomizerSettings

export function normalizeWallpaper(input) {
  if (typeof input !== 'string') return ''
  const value = input.trim()
  if (value.length > MAX_WALLPAPER_LENGTH) return ''
  if (/^data:image\/(?:jpeg|png|webp);base64,[A-Za-z0-9+/]+={0,2}$/i.test(value)) return value
  try {
    const url = new URL(value)
    return ['http:', 'https:'].includes(url.protocol) ? url.href : ''
  } catch { return '' }
}

function readLegacyWallpaper() {
  try { return JSON.parse(localStorage.getItem(THEME_CUSTOMIZER_STORAGE_KEY) || '{}').wallpaper || '' }
  catch { return '' }
}

const wallpaper = computed(() => {
  // Observe the shared settings so existing callers refresh after a legacy write.
  void themeCustomizerSettings.value
  return readLegacyWallpaper()
})

function setWallpaper(input) {
  const value = normalizeWallpaper(input)
  if (!value) throw new Error('请选择有效图片，或输入 HTTP / HTTPS 图片地址。')
  persistThemeSettings(THEME_CUSTOMIZER_STORAGE_KEY, {
    ...themeCustomizerSettings.value,
    wallpaper: value,
    wallpaperMode: 'custom',
  })
  return persistPartialThemeCustomizerSettings({})
}

export function useAppearance() {
  return {
    settings: themeCustomizerSettings,
    readSettings,
    updateSettings: persistPartialThemeCustomizerSettings,
    resetSettings: () => persistPartialThemeCustomizerSettings(appearanceDefaults),
    wallpaper,
    setWallpaper,
  }
}
