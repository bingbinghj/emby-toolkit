import { shallowReactive } from 'vue'
import { useToast } from 'vue-toastification'

export const feedbackDialogs = shallowReactive([])
let nextDialogId = 0

function showMessage(type, content, options = {}) {
  const toast = useToast()
  const timeout = options.duration === 0 ? false : options.duration ?? (type === 'error' ? 6000 : 4000)
  const id = toast(content, { type: type === 'loading' ? 'info' : type, timeout, hideProgressBar: true })
  return {
    destroy: () => toast.dismiss(id),
    set content(value) { toast.update(id, { content: value }) },
  }
}

const message = Object.fromEntries(
  ['success', 'info', 'warning', 'error', 'loading'].map(type => [type, (content, options) => showMessage(type, content, options)]),
)
message.destroyAll = () => useToast().clear()

export function useMessage() { return message }

function showDialog(type, options) {
  const entry = shallowReactive({ id: ++nextDialogId, type, options, busy: false })
  feedbackDialogs.push(entry)
  return { destroy: () => dismissDialog(entry), update: changes => { entry.options = { ...entry.options, ...changes } } }
}

export function dismissDialog(entry) {
  if (entry.busy) return
  const index = feedbackDialogs.indexOf(entry)
  if (index >= 0) feedbackDialogs.splice(index, 1)
  entry.options.onClose?.()
}

export async function answerDialog(entry, positive) {
  if (entry.busy) return
  entry.busy = true
  try {
    const result = await (positive ? entry.options.onPositiveClick?.() : entry.options.onNegativeClick?.())
    entry.busy = false
    if (result !== false) dismissDialog(entry)
  } catch (error) {
    entry.busy = false
    message.error(error.response?.data?.error || error.message || '操作失败，请查看处理结果。')
  }
}

const dialog = Object.fromEntries(
  ['success', 'info', 'warning', 'error'].map(type => [type, options => showDialog(type, options)]),
)
dialog.create = options => showDialog(options.type || 'info', options)
export function useDialog() { return dialog }
