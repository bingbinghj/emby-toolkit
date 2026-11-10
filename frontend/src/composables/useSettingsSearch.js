import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { useAuthStore } from '@/stores/auth';
import { useContextSearch } from './useContextSearch';
import catalog from '@/assets/settings-search.json';
import { searchSettings, settingsResultRoute } from '@/utils/settingsSearch';

const query = ref('');
const selected = ref(null);
// Preserve a personal receiver draft when following a search result to another settings tab.
const personalNotificationDraft = ref(null);
let settingsOwnerId = null;
const observedAuthStores = new WeakSet();

export function setSettingsSearchOwner(userId) {
  if (settingsOwnerId === userId) return;
  settingsOwnerId = userId;
  query.value = '';
  selected.value = null;
  personalNotificationDraft.value = null;
}
export function readPersonalNotificationDraft(userId) {
  return userId === settingsOwnerId && personalNotificationDraft.value?.userId === userId ? personalNotificationDraft.value : null;
}
export function savePersonalNotificationDraft(userId, chatId, loadedChatId) {
  if (!userId || userId !== settingsOwnerId) return;
  personalNotificationDraft.value = { userId, chatId, loadedChatId };
}
watch(query, () => { selected.value = null; });

export function useSettingsSearch(scope, options = {}) {
  const route = useRoute(), router = useRouter(), auth = useAuthStore();
  if (!observedAuthStores.has(auth)) {
    auth.$subscribe((_mutation, state) => setSettingsSearchOwner(state.isLoggedIn ? state.user?.id : null), { detached: true, flush: 'sync' });
    observedAuthStores.add(auth);
  }
  setSettingsSearchOwner(auth.isLoggedIn ? auth.user?.id : null);
  const notice = ref('');
  const results = computed(() => searchSettings(catalog, query.value));
  let active = true;
  const clearHighlight = () => document.querySelectorAll('.settings-search-highlight').forEach(element => element.classList.remove('settings-search-highlight'));
  async function locate() {
    clearHighlight();
    notice.value = '';
    const entry = selected.value;
    if (!entry || entry.route !== scope || options.ready?.value === false) return;
    if (scope === 'general' && (route.query.tab || 'system') !== entry.category) return;
    if (scope === 'notification' && ['notification_channel', 'telegram_enabled', 'telegram_bot_token', 'telegram_channel_id', 'telegram_chat_id'].includes(entry.field)) {
      if (!await options.openChannel?.()) {
        notice.value = '通知渠道尚不可编辑，请先核对页面中的读取错误。';
        return;
      }
    }
    await nextTick();
    if (!active || selected.value !== entry) return;
    let field = document.querySelector(`[data-field="${CSS.escape(entry.field)}"]`);
    const fallback = { ai_max_retries: ['ai_provider', '选择 OpenAI 后可设置失败重试次数。'], ai_translation_mode: ['ai_translate_actor_role', '启用翻译演员与角色后可设置翻译模式。'], p115_app_id: ['p115_auth_method', '选择自定义 AppID 扫码后可填写 OpenAPI AppID。'] }[entry.field];
    if (!field && fallback) {
      field = document.querySelector(`[data-field="${fallback[0]}"]`);
      notice.value = fallback[1];
    }
    if (!field) { notice.value = '此设置暂不可编辑，请先检查相关配置。'; return; }
    field.classList.add('settings-search-highlight');
    field.scrollIntoView({ behavior: 'smooth', block: 'center' });
    const control = field.matches('input,textarea,button') ? field : field.querySelector('input:not([type="hidden"]),textarea,button,[tabindex="0"]');
    if (!document.querySelector('.v-overlay--active .navbar-search-mobile')) control?.focus({ preventScroll: true });
  }
  async function select(entry) {
    if (!catalog.some(item => item.field === entry.field && item.route === entry.route)) return;
    selected.value = { ...entry };
    await router.push(settingsResultRoute(entry));
  }
  const showCard = title => !query.value.trim() || results.value.some(entry => entry.route === scope && entry.card === title);
  useContextSearch({ label: '搜索设置项', query, disabled: options.disabled, submit: () => results.value[0] && select(results.value[0]) });
  watch([selected, () => route.fullPath, () => options.ready?.value], locate, { flush: 'post', immediate: true });
  onBeforeUnmount(() => { active = false; clearHighlight(); });
  return { query, results, notice, selected, select, showCard };
}
