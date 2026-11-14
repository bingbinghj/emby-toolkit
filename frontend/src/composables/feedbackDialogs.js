import { shallowReactive } from 'vue'

export const feedbackDialogs = shallowReactive([])

export function resetFeedbackDialogs() {
  feedbackDialogs.splice(0)
}
