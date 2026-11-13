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
  const requests = [], messages = [], intervals = new Map(), timeouts = new Map(), disposals = [], watchers = [];
  const model = {
    emby_user_id: 'bad-id', ai_api_key: 'unrelated-secret',
    delay_between_items_sec: 0.5, media_info_retry_interval: 30, media_info_retry_attempts: 3,
    subtitle_chinese_default_language: 'chs', min_score_for_review: 6, max_actors_to_process: 40,
    max_episode_actors_to_process: 5, extract_thumb: true, actor_role_add_prefix: true,
    media_edit_skip_track_differences_on_select_all: false,
    remove_actors_without_avatars: false, keyword_to_tags: true, studio_to_chinese: true,
    monitor_paths: ['/media/a'], local_strm_root: '/media', p115_app_id: 'fixture',
  };
  const configState = { value: model }, configError = { value: '' };
  let configOwner = 0;
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
    computed: get => ({ get value() { return get(); } }),
    watch: (value, callback) => { watchers.push({ value, callback }); return () => {}; },
    onMounted() {}, onUnmounted: callback => disposals.push(callback), nextTick: async () => {},
    setInterval: callback => { const id = ++timerId; intervals.set(id, callback); return id; },
    clearInterval: id => intervals.delete(id),
    setTimeout: callback => { const id = ++timerId; timeouts.set(id, callback); return id; },
    clearTimeout: id => timeouts.delete(id),
    window: { innerWidth: 1200, location: { origin: 'https://etk.test' }, screen: { width: 1200, height: 800 }, open: overrides.open || (() => null), removeEventListener() {} },
    useDisplay: () => ({ smAndDown: { value: false } }),
    useRoute: () => ({ query: { tab: 'system' } }),
    useSettingsSearch: () => ({ query: { value: '' }, results: { value: [] }, selected: { value: null }, notice: { value: '' }, showCard: () => true }),
    useMessage: () => toast, useDialog: () => ({ warning() {}, success() {}, error() {} }),
    useConfig: () => ({
      configModel: configState, loadingConfig: { value: false }, savingConfig: { value: false }, configError,
      handleSaveConfig: async body => { requests.push({ method: 'SAVE', body }); return overrides.save ? overrides.save(body) : true; },
      captureConfigRefresh: () => {
        const owner = configOwner;
        return {
          isCurrent: () => owner === configOwner,
          refresh: async () => {
            if (owner !== configOwner) return false;
            requests.push({ method: 'CONFIG_REFRESH' });
            const result = overrides.refresh ? await overrides.refresh() : true;
            if (owner === configOwner && result === false) configError.value = '配置重新读取失败';
            return result;
          },
        };
      },
    }), axios,
  });
  vm.runInContext(source + '\n' + `globalThis.page = {
    saveCard, formRef, cardFields, configModel, openMediaServerDialog, saveMediaServer, mediaServerFormRef,
    mediaServerDraft, mediaServerDialogOpen, handleImportFileSelection, fileToImport,
    tablesToImport, handleImportSelectionChange, startImportProcess, openLocalFolderSelector,
    confirmLocalFolder, currentLocalPath, currentP115Cid, currentP115Path, confirmP115Folder,
    openQrcodeModal, closeQrcodeModal, qrcodeStatus, showQrcodeModal, componentIsMounted,
    startCookiePolling, closeCookieModal, cookieQrcodeSessionId, cookieQrcodeStatus, cookieAppType, showCookieModal,
    fetchLocalFolders, fetchP115Folders, showLocalFolderModal, showP115FolderModal, loadingLocalFolders, loadingP115Folders, currentLocalTargetField,
    openP115FolderSelector, startWebAuth, isWebAuthing, p115Info, previewRenameFormats, renameFormatPreview,
    monitorExtensionOptions, check115Status, p115StatusError, loading115Info,
    openPromptModal, savePrompts, resetPrompts, promptModalVisible, loadingPrompts, savingPrompts, promptsModel,
    openStorageDialog, saveStorageSettings, storageFormRef, storageDraft, refreshAIModels, aiModelOptions, isFetchingAIModels,
    handleClearTables, tablesToClear, clearTablesModalVisible, isClearing, isImporting,
    closePromptModal: typeof closePromptModal === 'function' ? closePromptModal : () => { promptModalVisible.value = false; }
  };`, context);
  context.page.componentIsMounted.value = true;
  context.page.formRef.value = { validate: async () => ({ valid: overrides.valid ?? true }) };
  context.page.mediaServerFormRef.value = context.page.formRef.value;
  return { page: context.page, requests, messages, intervals, timeouts, disposals, watchers,
    invalidateConfigOwner: () => { configOwner++; configState.value = { newOwner: true }; } };
}

const settle = () => new Promise(resolve => setImmediate(resolve));
const configRefreshes = f => f.requests.filter(request => request.method === 'CONFIG_REFRESH');

test('completed restoration rereads only the configuration table actually restored', async () => {
  for (const restored of [true, false]) {
    const f = fixture({ post: async () => ({ status: 200, data: {
      data_restored: true, restored_tables: restored ? ['app_settings'] : ['translation_cache'], message: '数据已恢复',
    } }) });
    f.page.fileToImport.value = new File(['{}'], 'backup.json');
    f.page.tablesToImport.value = ['app_settings'];
    await f.page.startImportProcess();
    await settle();
    assert.equal(configRefreshes(f).length, restored ? 1 : 0);
  }
});

test('configuration read failure after restoration remains a warning about committed data', async () => {
  const f = fixture({ post: async () => ({ status: 200, data: {
    data_restored: true, restored_tables: ['app_settings'], message: '数据已恢复',
  } }), refresh: async () => false });
  f.page.fileToImport.value = new File(['{}'], 'backup.json');
  f.page.tablesToImport.value = ['app_settings'];
  await f.page.startImportProcess();
  await settle();
  assert.equal(configRefreshes(f).length, 1);
  assert.equal(f.messages.some(message => message.kind === 'error'), false);
  assert.ok(f.messages.some(message => message.kind === 'warning' && /数据已恢复.*配置.*读取失败/.test(message.text)));
});

test('server post-commit restore warnings are not replaced by success', async () => {
  const f = fixture({ post: async () => ({ status: 200, data: {
    data_restored: true, restored_tables: ['app_settings'], config_reloaded: false,
    warnings: 1, warning_details: ['数据已恢复，运行配置重载失败'], message: '数据库恢复有提醒',
  } }) });
  f.page.fileToImport.value = new File(['{}'], 'backup.json');
  f.page.tablesToImport.value = ['app_settings'];
  await f.page.startImportProcess();
  await settle();
  assert.equal(f.messages.some(message => message.kind === 'success'), false);
  assert.ok(f.messages.some(message => message.kind === 'warning' && /运行配置重载失败/.test(message.text)));
});

test('a completed old restore does not fetch for a new owner or clear a newer import draft', async () => {
  const pending = deferred();
  const f = fixture({ post: () => pending.promise });
  f.page.fileToImport.value = new File(['old'], 'old.json');
  f.page.tablesToImport.value = ['app_settings'];
  const running = f.page.startImportProcess();
  f.invalidateConfigOwner();
  const nextFile = new File(['new'], 'new.json');
  f.page.fileToImport.value = nextFile;
  pending.resolve({ status: 200, data: { data_restored: true, restored_tables: ['app_settings'], message: '旧恢复完成' } });
  await running;
  await settle();
  assert.equal(configRefreshes(f).length, 0);
  assert.equal(f.page.fileToImport.value, nextFile);
  assert.equal(f.messages.some(message => ['success', 'error', 'warning'].includes(message.kind)), false);
});

test('partial table clearing refreshes configuration only when app_settings was actually cleared', async () => {
  for (const cleared of [['app_settings'], ['translation_cache']]) {
    const f = fixture({ post: async () => { throw { response: { status: 500, data: {
      error: '后续表清空失败', cleared_tables: cleared, config_reloaded: cleared[0] === 'app_settings' ? true : null,
    } } }; } });
    f.page.tablesToClear.value = ['app_settings', 'translation_cache'];
    f.page.clearTablesModalVisible.value = true;
    await f.page.handleClearTables();
    assert.equal(configRefreshes(f).length, cleared.includes('app_settings') ? 1 : 0);
    assert.deepEqual(plain(f.page.tablesToClear.value), ['app_settings', 'translation_cache'].filter(name => !cleared.includes(name)));
    assert.ok(f.messages.some(message => message.kind === 'error' && /后续表清空失败/.test(message.text)));
  }
});

test('successful configuration clearing preserves a newer table selection draft', async () => {
  const pending = deferred();
  const f = fixture({ post: () => pending.promise });
  f.page.tablesToClear.value = ['app_settings'];
  f.page.clearTablesModalVisible.value = true;
  const running = f.page.handleClearTables();
  f.page.tablesToClear.value = ['media_metadata'];
  pending.resolve({ status: 200, data: { cleared_tables: ['app_settings'], config_reloaded: true, message: '数据表已清空' } });
  await running;
  assert.equal(configRefreshes(f).length, 1);
  assert.deepEqual(plain(f.page.tablesToClear.value), ['media_metadata']);
  assert.equal(f.page.clearTablesModalVisible.value, true);
});

const promptValues = name => Object.fromEntries(['fast_mode', 'quality_mode', 'overview_translation',
  'title_translation', 'transliterate_mode', 'batch_joke_fallback'].map(key => [key, name + ':' + key]));
const promptPosts = f => f.requests.filter(request => request.method === 'POST' && request.url === '/api/ai/prompts');

test('a failed prompt read cannot overwrite custom prompts with initial empty values', async () => {
  const f = fixture({ get: async () => { throw new Error('提示词读取失败'); } });
  await f.page.openPromptModal();
  await f.page.savePrompts();
  assert.equal(promptPosts(f).length, 0);
  assert.equal(f.page.promptModalVisible.value, true);
  assert.equal(f.messages.some(message => message.kind === 'success'), false);
});

test('pending and invalid prompt reads cannot submit writes, and a successful retry allows explicit empty text', async () => {
  const pending = deferred();
  let reads = 0;
  const f = fixture({ get: () => ++reads === 1 ? pending.promise : Promise.resolve({ data: promptValues('authoritative') }) });
  const opening = f.page.openPromptModal();
  await f.page.savePrompts();
  assert.equal(promptPosts(f).length, 0);
  pending.resolve({ data: { message: 'invalid prompt response' } });
  await opening;
  await f.page.savePrompts();
  assert.equal(promptPosts(f).length, 0);
  await f.page.openPromptModal();
  f.page.promptsModel.value.fast_mode = '';
  await f.page.savePrompts();
  assert.equal(promptPosts(f).length, 1);
  assert.deepEqual(plain(promptPosts(f)[0].body), { ...promptValues('authoritative'), fast_mode: '' });
});

test('a delayed prompt read cannot overwrite a later reopened dialog', async () => {
  const pending = deferred();
  let reads = 0;
  const f = fixture({ get: () => ++reads === 1 ? pending.promise : Promise.resolve({ data: promptValues('new-dialog') }) });
  const opening = f.page.openPromptModal();
  f.page.closePromptModal();
  await f.page.openPromptModal();
  pending.resolve({ data: promptValues('old-dialog') });
  await opening;
  assert.deepEqual(plain(f.page.promptsModel.value), promptValues('new-dialog'));
  assert.equal(f.page.loadingPrompts.value, false);
  assert.equal(f.page.promptModalVisible.value, true);
});

test('a delayed prompt save cannot close a later reopened dialog', async () => {
  const pending = deferred();
  let reads = 0;
  const f = fixture({ get: async () => ({ data: promptValues(++reads === 1 ? 'old-dialog' : 'new-dialog') }),
    post: () => pending.promise });
  await f.page.openPromptModal();
  const saving = f.page.savePrompts();
  f.page.closePromptModal();
  await f.page.openPromptModal();
  pending.resolve({ data: { message: 'saved' } });
  await saving;
  assert.equal(f.page.promptModalVisible.value, true);
  assert.deepEqual(plain(f.page.promptsModel.value), promptValues('new-dialog'));
  assert.equal(f.messages.some(message => message.kind === 'success'), false);
  assert.equal(f.page.savingPrompts.value, false);
});

test('a prompt read after page unmount cannot fill or announce an obsolete dialog', async () => {
  const pending = deferred();
  const f = fixture({ get: () => pending.promise });
  const opening = f.page.openPromptModal();
  for (const dispose of f.disposals) dispose();
  pending.resolve({ data: promptValues('obsolete') });
  await opening;
  assert.equal(f.page.promptsModel.value.fast_mode, '');
  assert.equal(f.messages.some(message => message.kind === 'success'), false);
});

test('replacing the loaded configuration invalidates an open prompt draft before it can be saved', async () => {
  const f = fixture({ get: async () => ({ data: promptValues('old-config') }) });
  await f.page.openPromptModal();
  const configWatcher = f.watchers.find(watcher => watcher.value === f.page.configModel);
  assert.ok(configWatcher, 'configuration replacement must invalidate the dialog');
  const previous = f.page.configModel.value;
  f.page.configModel.value = { ...plain(previous), ai_api_key: 'replacement-config' };
  configWatcher.callback(f.page.configModel.value, previous);
  await f.page.savePrompts();
  assert.equal(f.page.promptModalVisible.value, false);
  assert.equal(promptPosts(f).length, 0);
});

test('an explicit reset can recover a failed prompt read before an ordinary save', async () => {
  const values = promptValues('default');
  const f = fixture({ get: async () => { throw new Error('failed read'); },
    post: async url => ({ data: url === '/api/ai/prompts/reset' ? { prompts: values } : {} }) });
  await f.page.openPromptModal();
  await f.page.resetPrompts();
  await f.page.savePrompts();
  assert.deepEqual(plain(promptPosts(f)[0].body), values);
});

test('a late settings save cannot report failure after its page is unmounted', async () => {
  const pending = deferred();
  const f = fixture({ save: () => pending.promise });
  const saving = f.page.saveCard('basic');
  for (const dispose of f.disposals) dispose();
  pending.resolve(false);
  await saving;
  assert.equal(f.messages.length, 0);
});

for (const kind of ['server', 'storage']) {
  test('delayed ' + kind + ' validation cannot submit an obsolete owner draft', async () => {
    const pending = deferred();
    const f = fixture();
    if (kind === 'server') {
      f.page.openMediaServerDialog();
      f.page.mediaServerFormRef.value = { validate: () => pending.promise };
    } else {
      f.page.openStorageDialog();
      f.page.storageFormRef.value = { validate: () => pending.promise };
    }
    const saving = kind === 'server' ? f.page.saveMediaServer() : f.page.saveStorageSettings();
    f.page.configModel.value = { ...plain(f.page.configModel.value), ai_api_key: 'new-owner' };
    pending.resolve({ valid: true });
    await saving;
    assert.equal(f.requests.filter(request => request.method === 'SAVE').length, 0);
  });
}

test('an old AI model list cannot fill a new owner configuration', async () => {
  const pending = deferred();
  const f = fixture({ post: () => pending.promise });
  const initialOptions = plain(f.page.aiModelOptions.value);
  const reading = f.page.refreshAIModels();
  f.page.configModel.value = { ...plain(f.page.configModel.value), ai_api_key: 'new-owner', ai_model_name: '' };
  pending.resolve({ data: { models: ['old-owner-model'] } });
  await reading;
  assert.equal(f.page.configModel.value.ai_model_name, '');
  assert.deepEqual(plain(f.page.aiModelOptions.value), initialOptions);
  assert.equal(f.messages.length, 0);
});

test('an older model read cannot replace a newer provider or clear its loading state', async () => {
  const first = deferred(), second = deferred();
  let reads = 0;
  const f = fixture({ post: () => ++reads === 1 ? first.promise : second.promise });
  const initialOptions = plain(f.page.aiModelOptions.value);
  const old = f.page.refreshAIModels();
  f.page.configModel.value.ai_api_key = 'new-provider-key';
  const fresh = f.page.refreshAIModels();
  first.resolve({ data: { models: ['old-provider-model'] } });
  await old;
  assert.equal(f.page.isFetchingAIModels.value, true);
  assert.deepEqual(plain(f.page.aiModelOptions.value), initialOptions);
  assert.equal(f.messages.length, 0);
  second.resolve({ data: { models: ['new-provider-model'] } });
  await fresh;
  assert.equal(f.page.isFetchingAIModels.value, false);
  assert.equal(f.page.configModel.value.ai_model_name, 'new-provider-model');
  assert.deepEqual(plain(f.page.aiModelOptions.value), [{ label: 'new-provider-model', value: 'new-provider-model' }]);
});

test('editing model connection parameters without another read prevents old automatic model selection', async () => {
  for (const [field, value] of [['ai_provider', 'gemini'], ['ai_base_url', 'https://new-provider.invalid'], ['ai_api_key', 'new-provider-key']]) {
    const pending = deferred();
    const f = fixture({ post: () => pending.promise });
    Object.assign(f.page.configModel.value, { ai_provider: 'openai', ai_base_url: 'https://old-provider.invalid', ai_model_name: '' });
    const initialOptions = plain(f.page.aiModelOptions.value);
    const reading = f.page.refreshAIModels();
    f.page.configModel.value[field] = value;
    pending.resolve({ data: { models: ['old-provider-model'] } });
    await reading;
    assert.equal(f.page.configModel.value.ai_model_name, '', field);
    assert.deepEqual(plain(f.page.aiModelOptions.value), initialOptions, field);
    assert.equal(f.messages.length, 0, field);
    assert.equal(f.page.isFetchingAIModels.value, false, field);
  }
});

test('native settings card submits only its declared fields and leaves unrelated drafts out', async () => {
  const f = fixture();
  await f.page.saveCard('basic');
  const request = f.requests.find(item => item.method === 'SAVE');
  assert.deepEqual(Object.keys(request.body).sort(), plain(f.page.cardFields.basic).sort());
  assert.equal(request.body.delay_between_items_sec, 0.5);
  assert.equal(request.body.subtitle_chinese_default_language, undefined);
  assert.equal(request.body.ai_api_key, undefined);
});

test('Vuetify invalid form result blocks only the Emby card', async () => {
  const f = fixture({ valid: false });
  f.page.openMediaServerDialog();
  await f.page.saveMediaServer();
  assert.equal(f.requests.length, 0);
  await f.page.saveCard('basic');
  assert.equal(f.requests.filter(item => item.method === 'SAVE').length, 1);
});

test('image language and SHA1 save independently while filesystem monitoring is disabled', async () => {
  const f = fixture();
  Object.assign(f.page.configModel.value, {
    monitor_enabled: false, tmdb_image_language_preference: 'original', monitor_sha1_pc_search: false,
    auto_extract_credits_on_ingest: true,
  });
  await f.page.saveCard('monitor');
  const monitor = f.requests.find(item => item.method === 'SAVE').body;
  assert.equal(monitor.monitor_enabled, false);
  assert.equal(monitor.tmdb_image_language_preference, undefined);
  assert.equal(monitor.monitor_sha1_pc_search, undefined);
  await f.page.saveCard('rename');
  const image = f.requests.filter(item => item.method === 'SAVE')[1].body;
  assert.equal(image.tmdb_image_language_preference, 'original');
  assert.equal(image.monitor_enabled, undefined);
  await f.page.saveCard('fingerprints');
  const fingerprint = f.requests.filter(item => item.method === 'SAVE')[2].body;
  assert.equal(fingerprint.monitor_sha1_pc_search, false);
  assert.equal(fingerprint.monitor_enabled, undefined);
  await f.page.saveCard('ingest');
  assert.equal(f.requests.filter(item => item.method === 'SAVE')[3].body.auto_extract_credits_on_ingest, true);
});

test('removed external playback URL cannot be submitted by media server settings', async () => {
  const f = fixture();
  f.page.configModel.value.emby_public_url = 'https://legacy.example.invalid';
  f.page.openMediaServerDialog();
  await f.page.saveMediaServer();
  assert.equal(f.requests.find(item => item.method === 'SAVE').body.emby_public_url, undefined);
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


test('recent-admission preview displays both real examples without altering the saved templates', async () => {
  const f = fixture({ post: async () => ({ data: { movie: '最新电影 (2026)/最新电影.mkv', tv: '最新剧集 (2025)/S02E09.mp4', movie_sample: { id: 'new-movie', title: '最新电影' }, tv_sample: { id: 'new-episode', title: '最新剧集' }, errors: {} } }) });
  f.page.configModel.value.movie_rename_format = '{{title}}.mkv';
  f.page.configModel.value.tv_rename_format = '{{title}}.mp4';
  await f.page.previewRenameFormats();
  assert.equal(f.page.renameFormatPreview.value.movie_sample.id, 'new-movie');
  assert.equal(f.page.renameFormatPreview.value.tv_sample.id, 'new-episode');
  assert.equal(f.page.configModel.value.movie_rename_format, '{{title}}.mkv');
  assert.deepEqual(plain(f.requests[0].body), { movie_format: '{{title}}.mkv', tv_format: '{{title}}.mp4' });
});

test('out-of-order rename previews cannot replace the newest examples or errors', async () => {
  const pending = [];
  const f = fixture({ post: () => new Promise(resolve => pending.push(resolve)) });
  const first = f.page.previewRenameFormats();
  const latest = f.page.previewRenameFormats();
  pending[1]({ data: { movie: 'new.mkv', movie_sample: { title: '最近电影' }, errors: { tv: 'Emby 查询失败' } } });
  await latest;
  pending[0]({ data: { movie: 'old.mkv', movie_sample: { title: '过去电影' }, errors: {} } });
  await first;
  assert.equal(f.page.renameFormatPreview.value.movie, 'new.mkv');
  assert.equal(f.page.renameFormatPreview.value.errors.tv, 'Emby 查询失败');
});

test('failed and empty recent reads clear previous examples without altering format drafts', async () => {
  let failed = false;
  const f = fixture({ post: async () => {
    if (failed) throw new Error('Emby unavailable');
    return { data: { movie: '', tv: '', movie_sample: null, tv_sample: null, errors: {} } };
  } });
  f.page.renameFormatPreview.value = { movie: 'previous.mkv', movie_sample: { title: '旧示例' } };
  await f.page.previewRenameFormats();
  assert.equal(f.page.renameFormatPreview.value.movie, '');
  assert.equal(f.page.renameFormatPreview.value.movie_sample, null);
  failed = true;
  await f.page.previewRenameFormats();
  assert.equal(f.page.renameFormatPreview.value.movie, '');
  assert.ok(f.page.renameFormatPreview.value.errors.movie);
  assert.ok(f.page.renameFormatPreview.value.errors.tv);
});

test('an unmounted settings page rejects pending preview responses', async () => {
  const pending = deferred();
  const f = fixture({ post: () => pending.promise });
  const request = f.page.previewRenameFormats();
  for (const dispose of f.disposals) dispose();
  pending.resolve({ data: { movie: 'late.mkv', movie_sample: { title: '迟到示例' } } });
  await request;
  assert.equal(f.page.renameFormatPreview.value.movie, '');
});

test('legacy scoring configuration is excluded from current basic settings saves', async () => {
  const f = fixture();
  await f.page.saveCard('basic');
  const payload = f.requests.find(request => request.method === 'SAVE').body;
  assert.equal(payload.min_score_for_review, undefined);
  assert.equal(f.page.configModel.value.min_score_for_review, 6);
});

test('115 read failures clear stale account details and expose the read error', async () => {
  const f = fixture({ get: async () => { throw new Error('115 unreachable'); } });
  f.page.p115Info.value = { user_info: { user_name: '旧账号' }, has_token: true };
  await f.page.check115Status();
  assert.equal(f.page.p115Info.value, null);
  assert.equal(f.page.p115StatusError.value, '状态读取失败');
  assert.equal(f.page.loading115Info.value, false);
  assert.ok(f.messages.some(item => item.kind === 'error'));
});
