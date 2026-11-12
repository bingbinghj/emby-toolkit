import test from 'node:test';
import assert from 'node:assert/strict';
import axios from 'axios';
import { nextTick } from 'vue';

const plain = value => JSON.parse(JSON.stringify(value));
let fixtureId = 0;

async function fixture(t, initial, save) {
  const originalGet = axios.get, originalPost = axios.post;
  t.after(() => { axios.get = originalGet; axios.post = originalPost; });
  const calls = [];
  axios.get = async path => {
    assert.equal(path, '/api/config');
    return { data: plain(initial) };
  };
  axios.post = async (path, payload) => {
    assert.equal(path, '/api/config');
    calls.push(plain(payload));
    return save(payload);
  };
  const url = new URL('../src/composables/useConfig.js', import.meta.url);
  url.searchParams.set('fixture', String(++fixtureId));
  const { useConfig, invalidateConfigCache } = await import(url.href);
  const store = useConfig();
  await nextTick();
  assert.equal(store.loadingConfig.value, false);
  assert.deepEqual(plain(store.configModel.value), initial);
  return { store, calls, useConfig, invalidateConfigCache };
}

test('an independent server draft merges canonical saved values only into submitted fields', async t => {
  const initial = {
    emby_server_url: 'http://old.invalid:8096',
    libraries_to_ignore: ['original-library'],
    ai_model_name: 'another-card-draft',
  };
  const draft = { emby_server_url: 'http://edited.invalid:8096/', libraries_to_ignore: ['original-library'] };
  const { store, calls } = await fixture(t, initial, async () => ({ data: { saved_fields: {
    emby_server_url: 'http://edited.invalid:8096', libraries_to_ignore: ['canonical-library'],
    ai_model_name: 'unrequested-server-value', server_only: 'unrequested-field',
  } } }));

  assert.equal(await store.handleSaveConfig(draft), true);
  assert.deepEqual(calls, [draft]);
  assert.deepEqual(plain(store.configModel.value), {
    ...initial, emby_server_url: 'http://edited.invalid:8096', libraries_to_ignore: ['canonical-library'],
  });
  assert.equal(Object.hasOwn(store.configModel.value, 'server_only'), false);
  assert.deepEqual(draft, { emby_server_url: 'http://edited.invalid:8096/', libraries_to_ignore: ['original-library'] });
  assert.equal(store.savingConfig.value, false);
});

test('a failed independent save leaves the global values and local draft available for retry', async t => {
  const initial = { emby_server_url: 'http://old.invalid:8096', libraries_to_ignore: ['keep-library'] };
  const draft = { emby_server_url: 'http://edited.invalid:8096', libraries_to_ignore: ['edited-library'] };
  const { store, calls } = await fixture(t, initial, async () => {
    throw { response: { data: { error: '服务器配置写入失败' } } };
  });

  assert.equal(await store.handleSaveConfig(draft), false);
  assert.deepEqual(calls, [draft]);
  assert.deepEqual(plain(store.configModel.value), initial);
  assert.deepEqual(draft, { emby_server_url: 'http://edited.invalid:8096', libraries_to_ignore: ['edited-library'] });
  assert.equal(store.configError.value, '服务器配置写入失败');
  assert.equal(store.savingConfig.value, false);
});

test('a successful reply cannot replace newer local text or in-place array edits', async t => {
  let release;
  const pending = new Promise(resolve => { release = resolve; });
  const initial = { emby_server_url: 'http://submitted.invalid:8096', libraries_to_ignore: ['original-library'] };
  const { store, calls } = await fixture(t, initial, () => pending);
  const saving = store.handleSaveConfig(plain(store.configModel.value));
  assert.equal(store.savingConfig.value, true);
  store.configModel.value.emby_server_url = 'http://newer-draft.invalid:8096';
  store.configModel.value.libraries_to_ignore.push('newer-library');
  release({ data: { saved_fields: {
    emby_server_url: 'http://canonical.invalid:8096', libraries_to_ignore: ['canonical-library'],
  } } });

  assert.equal(await saving, true);
  assert.deepEqual(calls, [initial]);
  assert.deepEqual(plain(store.configModel.value), {
    emby_server_url: 'http://newer-draft.invalid:8096', libraries_to_ignore: ['original-library', 'newer-library'],
  });
  assert.equal(store.savingConfig.value, false);
});

test('explicit refresh reads authoritative values while ordinary settings navigation keeps drafts', async t => {
  const f = await fixture(t, { ai_model_name: 'saved-model' }, async () => ({ data: {} }));
  f.store.configModel.value.ai_model_name = 'unsaved-draft';
  assert.equal(f.useConfig().configModel.value.ai_model_name, 'unsaved-draft');
  let reads = 0;
  axios.get = async () => { reads++; return { data: { ai_model_name: 'external-update' } }; };
  await f.store.fetchConfigData();
  assert.equal(reads, 1);
  assert.equal(f.store.configModel.value.ai_model_name, 'external-update');
});

test('failed explicit refresh leaves same-owner drafts shared until an explicit retry', async t => {
  const f = await fixture(t, { ai_model_name: 'saved-model' }, async () => ({ data: {} }));
  f.store.configModel.value.ai_model_name = 'unsaved-draft';
  let reads = 0;
  axios.get = async () => { reads++; throw new Error('暂时无法读取配置'); };
  assert.equal(await f.store.fetchConfigData(), false);
  const nextPage = f.useConfig();
  await nextTick();
  assert.equal(nextPage.configModel.value.ai_model_name, 'unsaved-draft');
  assert.equal(reads, 1);
  axios.get = async () => { reads++; return { data: { ai_model_name: 'fresh-model' } }; };
  assert.equal(await nextPage.fetchConfigData(), true);
  assert.equal(reads, 2);
  assert.equal(nextPage.configModel.value.ai_model_name, 'fresh-model');
});

test('invalidating an owner drops its configuration and allows a new authoritative read', async t => {
  const f = await fixture(t, { ai_api_key: 'owner-a-secret' }, async () => ({ data: {} }));
  f.store.configModel.value.ai_api_key = 'owner-a-draft';
  f.invalidateConfigCache();
  assert.equal(f.store.configModel.value, null);
  assert.equal(f.store.loadingConfig.value, false);
  assert.equal(f.store.savingConfig.value, false);
  assert.equal(f.store.configError.value, null);
  axios.get = async () => ({ data: { ai_api_key: 'owner-b-secret' } });
  const next = f.useConfig();
  await next.fetchConfigData();
  assert.deepEqual(plain(next.configModel.value), { ai_api_key: 'owner-b-secret' });
});

for (const outcome of ['success', 'failure']) {
  test('an obsolete configuration read cannot affect a new owner: ' + outcome, async t => {
    const f = await fixture(t, { ai_model_name: 'initial' }, async () => ({ data: {} }));
    let finishOld, finishNew;
    const oldRead = new Promise((resolve, reject) => { finishOld = outcome === 'success' ? resolve : reject; });
    const newRead = new Promise(resolve => { finishNew = resolve; });
    let reads = 0;
    axios.get = () => ++reads === 1 ? oldRead : newRead;
    const pending = f.store.fetchConfigData();
    await Promise.resolve();
    f.invalidateConfigCache();
    const next = f.useConfig();
    const fresh = next.fetchConfigData();
    await Promise.resolve();
    assert.equal(reads, 2, 'the new owner must read once; in-flight reads are shared');
    finishOld(outcome === 'success' ? { data: { ai_model_name: 'old-owner' } } : new Error('old-owner failure'));
    await pending;
    assert.equal(next.configModel.value, null);
    assert.equal(next.loadingConfig.value, true);
    assert.equal(next.configError.value, null);
    finishNew({ data: { ai_model_name: 'new-owner' } });
    await fresh;
    assert.equal(next.configModel.value.ai_model_name, 'new-owner');
    assert.equal(next.loadingConfig.value, false);
  });

  test('an obsolete configuration save cannot alter new owner data or saving state: ' + outcome, async t => {
    let finishOld, finishNew;
    const oldSave = new Promise((resolve, reject) => { finishOld = outcome === 'success' ? resolve : reject; });
    const newSave = new Promise(resolve => { finishNew = resolve; });
    let saves = 0;
    const f = await fixture(t, { ai_model_name: 'initial' }, () => ++saves === 1 ? oldSave : newSave);
    const pending = f.store.handleSaveConfig({ ai_model_name: 'old-owner-save' });
    f.invalidateConfigCache();
    const next = f.useConfig();
    await next.fetchConfigData();
    const fresh = next.handleSaveConfig({ ai_model_name: 'new-owner-save' });
    finishOld(outcome === 'success' ? { data: { saved_fields: { ai_model_name: 'old-owner-save' } } } : new Error('old-owner failure'));
    assert.equal(await pending, false);
    assert.equal(next.configModel.value.ai_model_name, 'initial');
    assert.equal(next.savingConfig.value, true);
    assert.equal(next.configError.value, null);
    finishNew({ data: { saved_fields: { ai_model_name: 'new-owner-save' } } });
    assert.equal(await fresh, true);
    assert.equal(next.configModel.value.ai_model_name, 'new-owner-save');
    assert.equal(next.savingConfig.value, false);
  });
}

test('configuration refresh after a mutation adopts restored fields and preserves newer drafts', async t => {
  const f = await fixture(t, { unchanged: 'before', text: 'before', paths: ['/before'], removed: 'old' }, async () => ({ data: {} }));
  const change = f.store.captureConfigRefresh();
  f.store.configModel.value.text = 'new-draft';
  f.store.configModel.value.paths.push('/new-draft');
  let release;
  axios.get = () => new Promise(resolve => { release = resolve; });
  const reading = change.refresh();
  f.store.configModel.value.extra = 'draft-during-read';
  release({ data: { unchanged: 'restored', text: 'restored', paths: ['/restored'], serverOnly: 'new' } });
  assert.equal(await reading, true);
  assert.deepEqual(plain(f.store.configModel.value), {
    unchanged: 'restored', text: 'new-draft', paths: ['/before', '/new-draft'],
    extra: 'draft-during-read', serverOnly: 'new',
  });
});

test('mutation refresh starts a fresh read instead of sharing a pre-commit read', async t => {
  const f = await fixture(t, { mode: 'before' }, async () => ({ data: {} }));
  const change = f.store.captureConfigRefresh();
  let finishOld, finishNew;
  const old = new Promise(resolve => { finishOld = resolve; });
  const fresh = new Promise(resolve => { finishNew = resolve; });
  let reads = 0;
  axios.get = () => ++reads === 1 ? old : fresh;
  const obsolete = f.store.fetchConfigData();
  const reloading = change.refresh();
  assert.equal(reads, 2);
  finishOld({ data: { mode: 'pre-commit' } });
  assert.equal(await obsolete, false);
  assert.equal(f.store.configModel.value.mode, 'before');
  assert.equal(f.store.loadingConfig.value, true);
  finishNew({ data: { mode: 'restored' } });
  assert.equal(await reloading, true);
  assert.equal(f.store.configModel.value.mode, 'restored');
  assert.equal(f.store.loadingConfig.value, false);
});

test('an old mutation cannot reread or alter a new owner configuration', async t => {
  const f = await fixture(t, { mode: 'owner-a' }, async () => ({ data: {} }));
  const change = f.store.captureConfigRefresh();
  f.invalidateConfigCache();
  let reads = 0;
  axios.get = async () => { reads++; return { data: { mode: 'owner-b' } }; };
  const next = f.useConfig();
  await next.fetchConfigData();
  assert.equal(change.isCurrent(), false);
  assert.equal(await change.refresh(), false);
  assert.equal(reads, 1);
  assert.equal(next.configModel.value.mode, 'owner-b');
});

test('failed post-mutation configuration reads keep drafts and can be retried', async t => {
  const f = await fixture(t, { unchanged: 'before', text: 'before' }, async () => ({ data: {} }));
  const change = f.store.captureConfigRefresh();
  f.store.configModel.value.text = 'new-draft';
  axios.get = async () => { throw new Error('configuration temporarily unavailable'); };
  assert.equal(await change.refresh(), false);
  assert.equal(change.isCurrent(), true);
  assert.deepEqual(plain(f.store.configModel.value), { unchanged: 'before', text: 'new-draft' });
  axios.get = async () => ({ data: { unchanged: 'restored', text: 'restored' } });
  assert.equal(await change.refresh(), true);
  assert.deepEqual(plain(f.store.configModel.value), { unchanged: 'restored', text: 'new-draft' });
});
