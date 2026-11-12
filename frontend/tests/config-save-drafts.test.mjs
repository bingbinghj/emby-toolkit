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
  const { useConfig } = await import(url.href);
  const store = useConfig();
  await nextTick();
  assert.equal(store.loadingConfig.value, false);
  assert.deepEqual(plain(store.configModel.value), initial);
  return { store, calls };
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
