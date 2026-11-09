// MoviePilot-Frontend v3 609ea05 (MIT).
import { getGlassThemePalette } from '@/utils/themePalette'
import type { VuetifyOptions } from 'vuetify'

const theme: VuetifyOptions['theme'] = {
  defaultTheme: 'glass',
  themes: { glass: getGlassThemePalette(), plain: getGlassThemePalette() },
}
export default theme
