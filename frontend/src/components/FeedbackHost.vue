<template>
  <VDialog v-if="current" :model-value="true" :persistent="current.busy" max-width="560"
    :fullscreen="smAndDown" @update:model-value="value => !value && dismissDialog(current)">
    <VCard @keydown.tab="keepDialogFocus">
      <VCardItem>
        <template #prepend><VIcon :icon="icons[current.type]" :color="current.type" /></template>
        <VCardTitle>{{ current.options.title || '请确认' }}</VCardTitle>
        <template #append><VBtn icon="mdi-close" variant="text" color="default" :disabled="current.busy"
          aria-label="关闭确认" @click="dismissDialog(current)" /></template>
      </VCardItem>
      <VCardText class="feedback-content"><DialogContent :content="current.options.content" /></VCardText>
      <VCardActions>
        <VSpacer />
        <VBtn v-if="current.options.negativeText || current.options.onPositiveClick" variant="text" color="default"
          :disabled="current.busy" @click="answerDialog(current, false)">{{ current.options.negativeText || '取消' }}</VBtn>
        <VBtn :color="current.type" :loading="current.busy" :disabled="current.busy" variant="flat" @click="answerDialog(current, true)">
          {{ current.options.positiveText || (current.options.onPositiveClick ? '确认' : '知道了') }}
        </VBtn>
      </VCardActions>
    </VCard>
  </VDialog>
</template>

<script setup>
import { computed, defineComponent, h, isVNode } from 'vue'
import { useDisplay } from 'vuetify'
import { feedbackDialogs, answerDialog, dismissDialog } from '../composables/useFeedback'

const { smAndDown } = useDisplay()
const current = computed(() => feedbackDialogs[0])
const icons = { info: 'mdi-information-outline', success: 'mdi-check-circle-outline', warning: 'mdi-alert-outline', error: 'mdi-alert-circle-outline' }
function keepDialogFocus(event) {
  const controls = [...event.currentTarget.querySelectorAll('button, a[href], input, select, textarea, [tabindex]')]
    .filter(element => element.tabIndex >= 0 && !element.matches(':disabled') && element.getClientRects().length)
  const first = controls[0], last = controls.at(-1)
  if (document.activeElement === (event.shiftKey ? first : last)) {
    event.preventDefault()
    ;(event.shiftKey ? last : first)?.focus()
  }
}
const DialogContent = defineComponent({
  props: ['content'],
  setup(props) {
    return () => {
      const content = typeof props.content === 'function' ? props.content() : props.content
      return isVNode(content) ? content : h('div', String(content ?? ''))
    }
  },
})
</script>

<style scoped>
.feedback-content { white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
