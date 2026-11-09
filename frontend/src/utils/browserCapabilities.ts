import { computed, readonly, ref } from 'vue'
import { useDisplay } from 'vuetify'

// ETK is a browser/PWA application; MoviePilot's native host bridge is not used.
export const nativeChromeActive = readonly(ref(false))
const displayEnvironment = ref('browser')

const displayQueries = [
  ['window-controls-overlay', '(display-mode: window-controls-overlay)'],
  ['standalone', '(display-mode: standalone)'],
  ['fullscreen', '(display-mode: fullscreen)'],
  ['minimal-ui', '(display-mode: minimal-ui)'],
] as const

if (typeof window !== 'undefined' && typeof window.matchMedia === 'function') {
  const queries = displayQueries.map(([name, query]) => ({ name, media: window.matchMedia(query) }))
  const update = () => {
    displayEnvironment.value = queries.find(({ media }) => media.matches)?.name
      ?? ((navigator as Navigator & { standalone?: boolean }).standalone ? 'standalone' : 'browser')
  }
  queries.forEach(({ media }) => media.addEventListener?.('change', update))
  update()
}

let savedUIMode = 'auto'
try {
  const saved = localStorage.getItem('ui-mode')
  if (saved === 'app' || saved === 'desktop') savedUIMode = saved
} catch { /* A restricted browser still has an automatic layout. */ }
const uiMode = ref(savedUIMode)

function isMobilePlatform() {
  if (typeof navigator === 'undefined') return false
  const browser = navigator as Navigator & { userAgentData?: { mobile?: boolean } }
  return browser.userAgentData?.mobile === true
    || /Android|webOS|iPhone|iPad|iPod|BlackBerry|IEMobile|Opera Mini|CriOS/i.test(browser.userAgent)
    || (/Macintosh/i.test(browser.userAgent) && browser.maxTouchPoints > 1)
}

export function usePWA() {
  const display = useDisplay()
  const appMode = computed(() => uiMode.value === 'app'
    || (uiMode.value !== 'desktop' && isMobilePlatform() && display.mdAndDown.value))
  return {
    appMode,
    uiMode: readonly(uiMode),
    setUIMode: (value: 'auto' | 'desktop' | 'app') => {
      localStorage.setItem('ui-mode', value)
      uiMode.value = value
    },
    displayEnvironment: readonly(displayEnvironment),
    isStandaloneMode: computed(() => ['standalone', 'fullscreen', 'minimal-ui'].includes(displayEnvironment.value)),
    isWindowControlsOverlayMode: computed(() => displayEnvironment.value === 'window-controls-overlay'),
  }
}
