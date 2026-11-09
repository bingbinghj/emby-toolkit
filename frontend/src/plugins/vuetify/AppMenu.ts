// Browser rendering adapted from MoviePilot-Frontend v3 609ea05 (MIT).
import { defineComponent, h } from 'vue'
import { VMenu } from 'vuetify/components/VMenu'

export default defineComponent({
  name: 'AppMenu',
  inheritAttrs: false,
  props: { modelValue: { type: Boolean, default: undefined } },
  emits: ['update:modelValue'],
  setup(props, { attrs, slots, emit }) {
    return () => h(VMenu, {
      ...attrs,
      // Omitting an absent model keeps Vuetify's activator-driven menu state.
      ...(props.modelValue === undefined ? {} : { modelValue: props.modelValue }),
      'onUpdate:modelValue': (value: boolean) => emit('update:modelValue', value),
    }, slots)
  },
})
