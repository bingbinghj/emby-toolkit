import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { searchSettings, settingsResultRoute } from '../src/utils/settingsSearch.js';
const catalog = JSON.parse(readFileSync(new URL('../src/assets/settings-search.json', import.meta.url)));

test('settings search matches current labels, hints and categories across both settings routes', () => {
  assert.equal(searchSettings(catalog, '电影重命名格式')[0].field, 'movie_rename_format');
  assert.ok(searchSettings(catalog, '数据管理').some(entry => entry.field === 'data_management'));
  assert.ok(searchSettings(catalog, '校准ID计数器').some(entry => entry.field === 'data_management'));
  assert.ok(searchSettings(catalog, 'metadata').some(entry => entry.field === 'emby_metadata_path'));
  assert.ok(searchSettings(catalog, '数据源').some(entry => entry.field === 'tmdb_api_key'));
  assert.ok(searchSettings(catalog, 'BOT token').some(entry => entry.field === 'telegram_bot_token'));
  assert.ok(searchSettings(catalog, '仅管理员').some(entry => entry.field === 'notification_scope'));
  assert.ok(searchSettings(catalog, '整理失败').some(entry => entry.field === 'telegram_notify_types'));
  assert.ok(searchSettings(catalog, 'Telegram').some(entry => entry.field === 'telegram_enabled'));
});

test('empty and unmatched queries have no results and all query words must match', () => {
  assert.deepEqual(searchSettings(catalog, '  '), []);
  assert.deepEqual(searchSettings(catalog, '不存在设置xyz'), []);
  assert.ok(searchSettings(catalog, 'LLM 模型').some(entry => entry.field === 'ai_model_name'));
  assert.deepEqual(searchSettings(catalog, 'LLM 未存在xyz'), []);
});

test('result navigation selects the settings category without carrying settings values', () => {
  for (const entry of catalog) {
    const route = settingsResultRoute(entry);
    assert.deepEqual(route, entry.route === 'notification' ? { name: 'settings-notifications' }
      : { name: 'settings-general', query: entry.category === 'system' ? {} : { tab: entry.category } });
    assert.doesNotMatch(JSON.stringify(route), /token|password|secret/);
  }
});


test('settings search owner preserves same-user drafts and clears them on logout or account changes', () => {
  const source = readFileSync(new URL('../src/composables/useSettingsSearch.js', import.meta.url), 'utf8');
  const functions = ['setSettingsSearchOwner', 'readPersonalNotificationDraft', 'savePersonalNotificationDraft'].map(name => source.match(new RegExp(`^export function ${name}\\([\\s\\S]*?^}`, 'm'))[0].replace('export ', '')).join('\n');
  const context = vm.createContext({ query: { value: '' }, selected: { value: null }, personalNotificationDraft: { value: null }, settingsOwnerId: null });
  vm.runInContext(functions + '\nthis.change = setSettingsSearchOwner; this.read = readPersonalNotificationDraft; this.save = savePersonalNotificationDraft;', context);
  context.change('user-a');
  context.query.value = '个人 Chat ID';
  context.selected.value = { field: 'telegram_chat_id' };
  context.save('user-a', 'unsaved-a-receiver', 'saved-a-receiver');
  context.change('user-a');
  assert.equal(context.read('user-a').chatId, 'unsaved-a-receiver');
  assert.equal(context.query.value, '个人 Chat ID');
  context.change(null);
  assert.equal(context.read('user-a'), null);
  assert.equal(context.query.value, '');
  assert.equal(context.selected.value, null);
  context.change('user-b');
  assert.equal(context.read('user-b'), null);
  context.save('user-a', 'late-a-receiver', 'saved-a-receiver');
  assert.equal(context.read('user-b'), null, 'an old departing page cannot save its receiver under the new owner');
  context.save('user-b', 'unsaved-b-receiver', 'saved-b-receiver');
  assert.equal(context.read('user-b').chatId, 'unsaved-b-receiver');
  assert.equal(context.read('user-a'), null);
  context.change('user-c');
  assert.equal(context.read('user-c'), null);
});


test('auth owner observation remains active after leaving settings and removes old account drafts', () => {
  const source = readFileSync(new URL('../src/composables/useSettingsSearch.js', import.meta.url), 'utf8')
    .replace(/^import .*;$/gm, '').replace(/^export /gm, '');
  const subscribers = [], disposals = [];
  const auth = { isLoggedIn: true, user: { id: 'user-a' }, $subscribe: (callback, options) => subscribers.push({ callback, options }) };
  const context = vm.createContext({
    ref: value => ({ value }), computed: get => ({ get value() { return get(); } }),
    watch() {}, onBeforeUnmount: callback => disposals.push(callback), nextTick: async () => {},
    useAuthStore: () => auth, useRoute: () => ({ query: {}, fullPath: '/setting' }), useRouter: () => ({ push: async () => {} }),
    useContextSearch() {}, catalog: [], searchSettings: () => [], settingsResultRoute: () => ({}),
    document: { querySelectorAll: () => [] },
  });
  vm.runInContext(source + '\nthis.setup = useSettingsSearch; this.save = savePersonalNotificationDraft; this.read = readPersonalNotificationDraft;', context);
  context.setup('general');
  context.save('user-a', 'a-receiver-draft', 'a-original');
  for (const dispose of disposals) dispose();
  assert.equal(subscribers.length, 1);
  assert.equal(subscribers[0].options.detached, true);
  auth.isLoggedIn = false;
  subscribers[0].callback({}, auth);
  assert.equal(context.read('user-a'), null);
  auth.isLoggedIn = true;
  auth.user = { id: 'user-b' };
  subscribers[0].callback({}, auth);
  context.setup('notification');
  assert.equal(subscribers.length, 1, 'category navigation does not duplicate the global auth observer');
  assert.equal(context.read('user-b'), null);
  context.save('user-a', 'late-old-receiver', 'a-original');
  assert.equal(context.read('user-b'), null);
});
