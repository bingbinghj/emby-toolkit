// MoviePilot-Frontend v3 609ea05 (MIT), with explicit Vue imports.
import { h } from 'vue'
import { Icon } from '@iconify/vue'
import { aliases } from 'vuetify/iconsets/mdi'

export const icons = {
  defaultSet: 'iconify',
  aliases: {
    ...aliases,
    success: 'mdi-check-circle-outline',
    info: 'mdi-information-outline',
    warning: 'mdi-alert-outline',
    error: 'mdi-alert-circle-outline',
  },
  sets: { iconify: { component: (props: Record<string, unknown>) => h(Icon, props) } },
}
