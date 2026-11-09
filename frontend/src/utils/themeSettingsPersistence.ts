/** Browser persistence for the ETK theme; no MoviePilot backend is required. */
export function persistThemeSettings(key: string, settings: object) {
  if (typeof localStorage === 'undefined') return

  let previous: object = {}
  try {
    const stored = JSON.parse(localStorage.getItem(key) || '{}')
    if (stored && typeof stored === 'object' && !Array.isArray(stored)) previous = stored
  } catch { /* A malformed legacy value must not prevent saving valid settings. */ }

  try {
    // Preserve legacy wallpaper preferences until the dedicated wallpaper UI migrates them.
    localStorage.setItem(key, JSON.stringify({ ...previous, ...settings }))
  } catch (error) {
    throw new Error('无法保存外观设置，浏览器存储空间可能已满或被禁用。', { cause: error })
  }
}
