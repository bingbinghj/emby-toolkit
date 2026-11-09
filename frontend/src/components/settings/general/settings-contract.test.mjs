import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import { File } from 'node:buffer';

const source = fs.readFileSync(new URL('../GeneralSettingsPage.vue', import.meta.url), 'utf8')
  .match(/<script\b[^>]*>([\s\S]*?)<\/script>/)[1]
  .replace(/^import(?:[\s\S]*?from\s+)?['"][^'"]+['"];?[ \t]*(?:\r?\n|$)/gm, '');
const plain = value => JSON.parse(JSON.stringify(value));
function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
}
function fixture(overrides = {}) {
  const requests = [], messages = [], intervals = new Map(), timeouts = new Map(), disposals = [];
  const model = {
    emby_user_id: 'bad-id', ai_api_key: 'unrelated-secret',
    delay_between_items_sec: 0.5, media_info_retry_interval: 30, media_info_retry_attempts: 3,
    subtitle_chinese_default_language: 'chs', min_score_for_review: 6, max_actors_to_process: 40,
    max_episode_actors_to_process: 5, extract_thumb: true, actor_role_add_prefix: true,
    remove_actors_without_avatars: false, keyword_to_tags: true, studio_to_chinese: true,
    monitor_paths: ['/media/a'], local_strm_root: '/media', p115_app_id: 'fixture',
  };
  const toast = Object.fromEntries(['success', 'error', 'warning', 'info', 'loading'].map(kind => [kind, text => {
    messages.push({ kind, text }); return { destroy() {} };
  }]));
  let timerId = 0;
  const axios = {
    async post(url, body, options) {
      requests.push({ method: 'POST', url, body, options });
      if (overrides.post) return overrides.post(url, body, options);
      return { data: { success: true, tables: ['emby_users', 'emby_users_extended'] } };
    },
    async get(url, options) {
      requests.push({ method: 'GET', url, options });
      if (overrides.get) return overrides.get(url, options);
      return { data: { code: 200, data: [] } };
    },
    async delete(url) { requests.push({ method: 'DELETE', url }); return { data: { success: true } }; },
  };
  const context = vm.createContext({
    console, FormData, JSON, Set, Map,
    ref: value => ({ value }), reactive: value => value,
    computed: get => ({ get value() { return get(); } }), watch: () => () => {},
    onMounted() {}, onUnmounted: callback => disposals.push(callback), nextTick: async () => {},
    setInterval: callback => { const id = ++timerId; intervals.set(id, callback); return id; },
    clearInterval: id => intervals.delete(id),
    setTimeout: callback => { const id = ++timerId; timeouts.set(id, callback); return id; },
    clearTimeout: id => timeouts.delete(id),
    window: { innerWidth: 1200, location: { origin: 'https://etk.test' }, screen: { width: 1200, height: 800 }, open: overrides.open || (() => null), removeEventListener() {} },
    useDisplay: () => ({ smAndDown: { value: false } }),
    useMessage: () => toast, useDialog: () => ({ warning() {}, success() {}, error() {} }),
    useConfig: () => ({
      configModel: { value: model }, loadingConfig: { value: false }, savingConfig: { value: false }, configError: { value: '' },
      handleSaveConfig: async body => { requests.push({ method: 'SAVE', body }); return true; },
    }), axios,
  });
  vm.runInContext(source + '\n' + `globalThis.page = {
    saveCard, formRef, cardFields, configModel, handleImportFileSelection, fileToImport,
    tablesToImport, handleImportSelectionChange, startImportProcess, openLocalFolderSelector,
    confirmLocalFolder, currentLocalPath, currentP115Cid, currentP115Path, confirmP115Folder,
    openQrcodeModal, closeQrcodeModal, qrcodeStatus, showQrcodeModal, componentIsMounted,
    startCookiePolling, closeCookieModal, cookieQrcodeSessionId, cookieQrcodeStatus, cookieAppType, showCookieModal,
    fetchLocalFolders, fetchP115Folders, showLocalFolderModal, showP115FolderModal, loadingLocalFolders, loadingP115Folders, currentLocalTargetField,
    openP115FolderSelector, startWebAuth, isWebAuthing, p115Info
  };`, context);
  context.page.componentIsMounted.value = true;
  context.page.formRef.value = { validate: async () => ({ valid: overrides.valid ?? true }) };
  return { page: context.page, requests, messages, intervals, timeouts, disposals };
}

test('native settings card submits only its declared fields and leaves unrelated drafts out', async () => {
  const f = fixture();
  await f.page.saveCard('basic');
  const request = f.requests.find(item => item.method === 'SAVE');
  assert.deepEqual(Object.keys(request.body).sort(), plain(f.page.cardFields.basic).sort());
  assert.equal(request.body.delay_between_items_sec, 0.5);
  assert.equal(request.body.subtitle_chinese_default_language, 'chs');
  assert.equal(request.body.ai_api_key, undefined);
});

test('Vuetify invalid form result blocks only the Emby card', async () => {
  const f = fixture({ valid: false });
  await f.page.saveCard('emby');
  assert.equal(f.requests.length, 0);
  await f.page.saveCard('basic');
  assert.equal(f.requests.filter(item => item.method === 'SAVE').length, 1);
});

test('VFileInput File array reaches multipart preview and selected table import unchanged', async () => {
  const f = fixture();
  const file = new File(['compressed-fixture'], 'backup.json.gz', { type: 'application/gzip' });
  await f.page.handleImportFileSelection([file]);
  const preview = f.requests.find(item => item.url === '/api/database/preview-backup');
  assert.equal(preview.body.get('file').name, file.name);
  assert.equal(await preview.body.get('file').text(), 'compressed-fixture');
  assert.equal(f.page.fileToImport.value, file);
  f.page.tablesToImport.value = ['emby_users_extended'];
  f.page.handleImportSelectionChange(['emby_users_extended']);
  assert.deepEqual(plain(f.page.tablesToImport.value).sort(), ['emby_users', 'emby_users_extended']);
  f.page.startImportProcess();
  await Promise.resolve();
  const imported = f.requests.find(item => item.url === '/api/database/import');
  assert.equal(imported.body.get('file').name, file.name);
  assert.deepEqual(imported.body.get('tables').split(',').sort(), ['emby_users', 'emby_users_extended']);
});

test('directory selection keeps local path arrays and cloud CID/path values', async () => {
  const f = fixture({ get: async (url, options) => ({ data: { code: 200, data: [], current_path: url === '/api/p115/directories' ? '媒体库/电影' : options.params.path, current_cid: options.params.cid } }) });
  f.page.openLocalFolderSelector('monitor_paths', true);
  await f.page.fetchLocalFolders('/media/b');
  f.page.confirmLocalFolder();
  f.page.confirmLocalFolder();
  assert.deepEqual(plain(f.page.configModel.value.monitor_paths), ['/media/a', '/media/b']);
  f.page.openLocalFolderSelector('local_strm_root');
  await f.page.fetchLocalFolders('/new-media');
  f.page.confirmLocalFolder();
  assert.equal(f.page.configModel.value.local_strm_root, '/new-media');
  f.page.showP115FolderModal.value = true;
  await f.page.fetchP115Folders('123');
  f.page.confirmP115Folder();
  assert.equal(f.page.configModel.value.p115_fingerprint_media_root_cid, '123');
  assert.equal(f.page.configModel.value.p115_fingerprint_media_root_path, '媒体库/电影');
});

test('closing the native QR dialog discards a late request and prevents polling', async () => {
  const gate = deferred();
  const f = fixture({ post: () => gate.promise });
  const opening = f.page.openQrcodeModal();
  f.page.closeQrcodeModal();
  gate.resolve({ data: { success: true, data: { qrcode: 'late-qr' } } });
  await opening;
  assert.equal(f.page.showQrcodeModal.value, false);
  assert.equal(f.page.qrcodeStatus.value, 'idle');
  assert.equal(f.intervals.size, 0);
});

test('unmount stops an active native QR poll', async () => {
  const f = fixture({ post: async () => ({ data: { success: true, data: { qrcode: 'qr' } } }) });
  await f.page.openQrcodeModal();
  assert.equal(f.intervals.size, 1);
  for (const dispose of f.disposals) dispose();
  assert.equal(f.intervals.size, 0);
});

function cookieSession(f, session, client) {
  f.page.cookieQrcodeSessionId.value = session;
  f.page.cookieQrcodeStatus.value = 'waiting';
  f.page.cookieAppType.value = client;
  f.page.showCookieModal.value = true;
  f.page.startCookiePolling(session, client);
}

for (const outcome of ['success', 'client-mismatch', 'error']) {
  test(`old Cookie ${outcome} response cannot update or stop the new session`, async () => {
    const gate = deferred();
    const f = fixture({ get: () => gate.promise });
    cookieSession(f, 'old', 'alipaymini');
    const oldRequest = [...f.intervals.values()][0]();
    f.page.closeCookieModal();
    cookieSession(f, 'new', '115ios');
    gate.resolve({ data: { status: outcome === 'client-mismatch' ? 'waiting' : outcome, client_type: outcome === 'client-mismatch' ? 'web' : 'alipaymini' } });
    await oldRequest;
    assert.equal(f.page.cookieQrcodeStatus.value, 'waiting');
    assert.equal(f.page.cookieQrcodeSessionId.value, 'new');
    assert.equal(f.intervals.size, 1);
    assert.equal(f.timeouts.size, 0);
    assert.equal(f.messages.length, 0);
  });
}

test('Cookie delayed success close cannot close a newly opened session', async () => {
  const f = fixture({ get: async () => ({ data: { status: 'success', client_type: 'alipaymini' } }) });
  cookieSession(f, 'old', 'alipaymini');
  await [...f.intervals.values()][0]();
  assert.equal(f.page.cookieQrcodeStatus.value, 'success');
  f.page.closeCookieModal();
  cookieSession(f, 'new', '115ios');
  for (const callback of f.timeouts.values()) callback();
  assert.equal(f.page.showCookieModal.value, true);
  assert.equal(f.page.cookieQrcodeStatus.value, 'waiting');
  assert.equal(f.intervals.size, 1);
});

test('local directory confirmation uses the last successfully loaded path after out-of-order replies', async () => {
  const pending = [];
  const f = fixture({ get: (url, options) => new Promise(resolve => pending.push({ url, options, resolve })) });
  f.page.currentLocalTargetField.value = 'local_strm_root';
  f.page.showLocalFolderModal.value = true;
  const first = f.page.fetchLocalFolders('/first');
  const latest = f.page.fetchLocalFolders('/second');
  pending[1].resolve({ data: { code: 200, current_path: '/second', data: [] } });
  await latest;
  pending[0].resolve({ data: { code: 200, current_path: '/first', data: [] } });
  await first;
  f.page.confirmLocalFolder();
  assert.equal(f.page.configModel.value.local_strm_root, '/second');
});

test('cloud directory confirmation preserves the last CID/path pair after out-of-order replies', async () => {
  const pending = [];
  const f = fixture({ get: (url, options) => new Promise(resolve => pending.push({ url, options, resolve })) });
  f.page.showP115FolderModal.value = true;
  const first = f.page.fetchP115Folders('first');
  const latest = f.page.fetchP115Folders('second');
  pending[1].resolve({ data: { code: 200, current_cid: 'second', current_path: '媒体库/second', data: [] } });
  await latest;
  pending[0].resolve({ data: { code: 200, current_cid: 'first', current_path: '媒体库/first', data: [] } });
  await first;
  f.page.confirmP115Folder();
  assert.equal(f.page.configModel.value.p115_fingerprint_media_root_cid, 'second');
  assert.equal(f.page.configModel.value.p115_fingerprint_media_root_path, '媒体库/second');
});

test('directory loading or failed reads cannot confirm unverified local paths or cloud IDs', async () => {
  const pending = [];
  const f = fixture({ get: () => new Promise(resolve => pending.push(resolve)) });
  f.page.openLocalFolderSelector('local_strm_root');
  f.page.currentLocalPath.value = '/unverified';
  f.page.confirmLocalFolder();
  assert.equal(f.page.configModel.value.local_strm_root, '/media');
  pending[0]({ data: { code: 403, message: 'denied' } });
  await Promise.resolve();
  await Promise.resolve();
  f.page.confirmLocalFolder();
  assert.equal(f.page.configModel.value.local_strm_root, '/media');
  f.page.showP115FolderModal.value = true;
  const cloud = f.page.fetchP115Folders('unverified');
  f.page.confirmP115Folder();
  assert.equal(f.page.configModel.value.p115_fingerprint_media_root_cid, undefined);
  pending[1]({ data: { code: 403, message: 'denied' } });
  await cloud;
  f.page.confirmP115Folder();
  assert.equal(f.page.configModel.value.p115_fingerprint_media_root_cid, undefined);
});

test('blocked web authorization popup preserves existing credentials and starts no success poll', () => {
  const f = fixture({ open: () => null, get: async () => ({ data: { data: { has_token: true } } }) });
  f.page.p115Info.value = { has_token: true, user_info: { user_name: '原账号' } };
  f.page.startWebAuth();
  assert.equal(f.page.isWebAuthing.value, false);
  assert.equal(f.intervals.size, 0);
  assert.equal(f.page.p115Info.value.has_token, true);
  assert.ok(f.messages.some(item => item.kind === 'warning'));
  assert.ok(!f.messages.some(item => item.kind === 'success'));
});

test('existing token is not a new web authorization until this popup reaches the saved callback', async () => {
  const popup = { closed: false, location: { origin: 'https://115.example', pathname: '/login' }, document: { title: '登录' } };
  const f = fixture({ open: () => popup, get: async () => ({ data: { data: { has_token: true, valid: true } } }) });
  f.page.p115Info.value = { has_token: true };
  f.page.startWebAuth();
  await [...f.intervals.values()][0]();
  assert.equal(f.page.isWebAuthing.value, true);
  assert.ok(!f.messages.some(item => item.kind === 'success'));
  assert.equal(f.requests.filter(item => item.url === '/api/p115/status').length, 0);
  popup.location = { origin: 'https://etk.test', pathname: '/api/p115/auto_save_auth' };
  popup.document.title = '授权成功';
  await [...f.intervals.values()][0]();
  assert.equal(f.page.isWebAuthing.value, false);
  assert.equal(f.intervals.size, 0);
  assert.ok(f.messages.some(item => item.kind === 'success'));
});

test('old Cookie request rejection cannot stop a reopened session', async () => {
  const gate = deferred();
  const f = fixture({ get: async () => { await gate.promise; throw new Error('old request failed'); } });
  cookieSession(f, 'old', 'alipaymini');
  const oldRequest = [...f.intervals.values()][0]();
  f.page.closeCookieModal();
  cookieSession(f, 'new', '115ios');
  gate.resolve();
  await oldRequest;
  assert.equal(f.page.cookieQrcodeStatus.value, 'waiting');
  assert.equal(f.intervals.size, 1);
  assert.equal(f.messages.length, 0);
});

test('editing a local path invalidates confirmation until that path has loaded', async () => {
  const f = fixture({ get: async (url, options) => ({ data: { code: 200, current_path: options.params.path, data: [] } }) });
  f.page.currentLocalTargetField.value = 'local_strm_root';
  f.page.showLocalFolderModal.value = true;
  await f.page.fetchLocalFolders('/loaded');
  f.page.currentLocalPath.value = '/typed';
  f.page.confirmLocalFolder();
  assert.equal(f.page.configModel.value.local_strm_root, '/media');
  await f.page.fetchLocalFolders('/typed');
  f.page.confirmLocalFolder();
  assert.equal(f.page.configModel.value.local_strm_root, '/typed');
});

test('closing directory dialogs discards pending replies without changing the new displayed location', async () => {
  const gate = deferred();
  const f = fixture({ get: () => gate.promise });
  f.page.currentLocalTargetField.value = 'local_strm_root';
  f.page.showLocalFolderModal.value = true;
  const loading = f.page.fetchLocalFolders('/old');
  f.page.showLocalFolderModal.value = false;
  f.page.currentLocalPath.value = '/new';
  gate.resolve({ data: { code: 200, current_path: '/old', data: [] } });
  await loading;
  assert.equal(f.page.currentLocalPath.value, '/new');
  assert.equal(f.page.configModel.value.local_strm_root, '/media');
});

test('closing an unconfirmed web popup preserves old credentials and cannot report success', async () => {
  const popup = { closed: true, location: { origin: 'https://115.example', pathname: '/login' }, document: { title: '' } };
  const f = fixture({ open: () => popup, get: async () => ({ data: { data: { has_token: true } } }) });
  f.page.p115Info.value = { has_token: true };
  f.page.startWebAuth();
  await [...f.intervals.values()][0]();
  assert.equal(f.page.isWebAuthing.value, false);
  assert.equal(f.page.p115Info.value.has_token, true);
  assert.equal(f.requests.length, 0);
  assert.ok(!f.messages.some(item => item.kind === 'success'));
});

test('a blocked popup exception follows the visible failure path without polling', () => {
  const f = fixture({ open: () => { throw new Error('sandbox blocked'); } });
  f.page.startWebAuth();
  assert.equal(f.page.isWebAuthing.value, false);
  assert.equal(f.intervals.size, 0);
  assert.ok(f.messages.some(item => item.kind === 'warning'));
});

test('confirmed web callback still waits for stored Token and does not erase old credentials', async () => {
  const popup = { closed: false, location: { origin: 'https://etk.test', pathname: '/api/p115/auto_save_auth' }, document: { title: '授权成功' } };
  const f = fixture({ open: () => popup, get: async () => ({ data: { data: { has_token: false } } }) });
  f.page.p115Info.value = { has_token: true, user_info: { user_name: '原账号' } };
  f.page.startWebAuth();
  await [...f.intervals.values()][0]();
  assert.equal(f.page.isWebAuthing.value, true);
  assert.equal(f.page.p115Info.value.has_token, true);
  assert.ok(!f.messages.some(item => item.kind === 'success'));
});
