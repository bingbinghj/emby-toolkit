import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import assert from 'node:assert/strict';

const source = readFileSync(new URL('../src/components/settings/NotificationSettingsPage.vue', import.meta.url), 'utf8');
const plain = value => JSON.parse(JSON.stringify(value));
function fixture({ save = async () => true, post = async () => ({ data: { message: '已保存' } }), account = async () => ({ telegram_chat_id: '123' }) } = {}) {
  const calls = [], messages = [], opened = [], user = { id: 'test-user' };
  const context = vm.createContext({
    pageActive: true, loadedChatId: '', user, window: { open: url => opened.push(url) },
    authStore: { user, isLoggedIn: true, accountInfo: {}, fetchAccountInfo: account },
    configModel: { value: { telegram_bot_token: 'fixture-token', telegram_channel_id: '@fixture', telegram_notify_types: ['library_new'], telegram_enabled: true, notification_scope: { library_new: 'admin', recognize_fail: 'admin' }, notification_time: { start: '00:00', end: '23:59' }, unrelated: 'keep' } },
    channelDraft: { value: null }, telegramChatId: { value: '456' }, loading: { value: false }, accountError: { value: '' }, configError: { value: '' },
    isSavingChatId: { value: false }, isTestingTelegram: { value: false }, isFetchingBotLink: { value: false },
    canSave: { value: true }, canTest: { value: true }, busy: { value: false },
    handleSaveConfig: async payload => { calls.push({ path: '/api/config', payload: plain(payload) }); return save(payload); },
    axios: { post: async (path, payload) => { calls.push({ path, payload: plain(payload) }); return post(path, payload); } },
    message: { success: text => messages.push({ type: 'success', text }), error: text => messages.push({ type: 'error', text }) },
  });
  vm.runInContext(`const isCurrentAccount = user => pageActive && authStore.isLoggedIn && authStore.user === user;\n${['loadAccount', 'saveChatId', 'testTelegram', 'openBotChat'].map(name => source.match(new RegExp(`^const ${name} = async \\(\\) => \\{[\\s\\S]*?^};`, 'm'))?.[0] || (() => { throw new Error(`Missing handler ${name}`); })()).join('\n')}\nthis.load = loadAccount; this.save = saveChatId; this.test = testTelegram; this.openBot = openBotChat;`, context);
  return { context, calls, messages, opened };
}

test('notification save submits only notification fields then the selected personal chat ID', async () => {
  const f = fixture();
  await f.context.save();
  assert.deepEqual(f.calls, [
    { path: '/api/config', payload: { telegram_bot_token: 'fixture-token', telegram_channel_id: '@fixture', telegram_notify_types: ['library_new'], telegram_enabled: true, notification_scope: { library_new: 'admin', recognize_fail: 'admin' }, notification_time: { start: '00:00', end: '23:59' } } },
    { path: '/api/portal/telegram-chat-id', payload: { chat_id: '456' } },
  ]);
  assert.equal(f.context.authStore.accountInfo.telegram_chat_id, '456');
  assert.equal(f.messages.at(-1).type, 'success');
});

test('unread configuration and busy actions cannot submit notification writes', async () => {
  const f = fixture();
  f.context.canSave.value = false;
  await f.context.save();
  f.context.canSave.value = true;
  f.context.busy.value = true;
  await f.context.save();
  await f.context.test();
  assert.deepEqual(f.calls, []);
});

test('failed global configuration save does not write a personal chat ID or discard the draft', async () => {
  const f = fixture({ save: async () => false });
  f.context.configError.value = '配置写入失败';
  await f.context.save();
  assert.equal(f.calls.length, 1);
  assert.equal(f.context.telegramChatId.value, '456');
  assert.match(f.messages.at(-1).text, /配置写入失败/);
});

test('personal write failure reports that the global configuration already saved', async () => {
  const f = fixture({ post: async () => { throw { response: { data: { error: '个人ID写入失败' } } }; } });
  await f.context.save();
  assert.match(f.messages.at(-1).text, /全局配置已保存.*个人ID写入失败/);
  assert.equal(f.context.telegramChatId.value, '456');
});

for (const change of ['unmount', 'account']) {
  test(`a late global save cannot write a personal ID after ${change}`, async () => {
    let release;
    const f = fixture({ save: () => new Promise(resolve => { release = resolve; }) });
    const saving = f.context.save();
    if (change === 'unmount') f.context.pageActive = false;
    else f.context.authStore.user = { id: 'new-user' };
    release(true);
    await saving;
    assert.equal(f.calls.length, 1);
    assert.deepEqual(f.messages, []);
  });
}

test('a delayed personal save records submitted values without overwriting newer channel drafts', async () => {
  let release;
  const f = fixture({ post: () => new Promise(resolve => { release = resolve; }) });
  const saving = f.context.save();
  await new Promise(resolve => setTimeout(resolve, 0));
  f.context.telegramChatId.value = 'new-personal-draft';
  f.context.configModel.value.telegram_channel_id = '@new-draft';
  release({ data: { message: '已保存' } });
  await saving;
  assert.equal(f.context.telegramChatId.value, 'new-personal-draft');
  assert.equal(f.context.configModel.value.telegram_channel_id, '@new-draft');
  assert.equal(f.context.authStore.accountInfo.telegram_chat_id, '456');
  assert.equal(f.context.authStore.accountInfo.telegram_channel_id, '@fixture');
});

test('reloading account data preserves an edited personal chat ID', async () => {
  const f = fixture();
  f.context.loadedChatId = '456';
  f.context.telegramChatId.value = 'current-draft';
  await f.context.load();
  assert.equal(f.context.telegramChatId.value, 'current-draft');
  assert.equal(f.context.loadedChatId, '123');
});

test('test notification requires an explicit action and sends the chosen receiver', async () => {
  const f = fixture({ post: async () => ({ data: { success: true, message: '模拟测试成功' } }) });
  assert.deepEqual(f.calls, []);
  await f.context.test();
  assert.deepEqual(f.calls, [{ path: '/api/telegram/test', payload: { token: 'fixture-token', chat_id: '456' } }]);
  assert.equal(f.messages.at(-1).text, '模拟测试成功');
});


test('notification test uses dialog drafts and cannot silently substitute saved receivers', async () => {
  const f = fixture({ post: async () => ({ data: { success: true } }) });
  f.context.channelDraft.value = { telegram_bot_token: 'draft-token', telegram_channel_id: '@draft-channel', chat_id: '' };
  await f.context.test();
  assert.deepEqual(f.calls, [{ path: '/api/telegram/test', payload: { token: 'draft-token', chat_id: '@draft-channel' } }]);
});

test('notification save removes the nonfunctional intercept switch while preserving real scope and time', async () => {
  const f = fixture();
  f.context.configModel.value.telegram_notify_types = ['library_new', 'recognize_fail', 'intercept_notify'];
  f.context.configModel.value.notification_scope.library_new = 'all';
  f.context.configModel.value.notification_time = { start: '23:00', end: '06:00' };
  await f.context.save();
  assert.deepEqual(f.calls[0].payload.telegram_notify_types, ['library_new', 'recognize_fail']);
  assert.equal(f.calls[0].payload.notification_scope.library_new, 'all');
  assert.deepEqual(f.calls[0].payload.notification_time, { start: '23:00', end: '06:00' });
  assert.equal(Object.hasOwn(f.calls[0].payload, 'unrelated'), false);
});


test('opening a bot uses the unsaved channel token in a POST body and cannot fall back to the saved bot', async () => {
  const f = fixture({ post: async () => ({ data: { bot_username: 'draft_bot' } }) });
  f.context.channelDraft.value = { telegram_bot_token: 'draft-bot-token' };
  await f.context.openBot();
  assert.deepEqual(f.calls, [{ path: '/api/portal/telegram-bot-info', payload: { token: 'draft-bot-token' } }]);
  assert.deepEqual(f.opened, ['https://t.me/draft_bot']);
  f.context.channelDraft.value.telegram_bot_token = '';
  await f.context.openBot();
  assert.equal(f.calls.length, 1);
});
