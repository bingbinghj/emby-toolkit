import test from 'node:test';
import assert from 'node:assert/strict';
import { createPinia, setActivePinia } from 'pinia';
import axios from 'axios';
import { useAuthStore } from '../src/stores/auth.js';
import { invalidateConfigCache, useConfig } from '../src/composables/useConfig.js';

function fixture() {
  setActivePinia(createPinia()); invalidateConfigCache();
  const originalAdapter = axios.defaults.adapter, store = useAuthStore();
  const state = { denied: null, accountDenied: false, account: { id: 'A', name: 'A', profile_image_tag: 'tag-a' }, accountReads: [], deferAccount: false };
  const response = (config, data) => ({ config, data, status: 200, statusText: 'OK', headers: {} });
  axios.defaults.adapter = config => {
    if (config.url === '/api/auth/login') {
      if (state.denied) return Promise.reject(new axios.AxiosError('login rejected', 'ERR_BAD_REQUEST', config, null,
        { config, status: state.denied, data: { error: 'sign-in rejected' }, headers: state.denied === 403 ? { 'set-cookie': 'etk_session=; Max-Age=0' } : {} }));
      const body = JSON.parse(config.data);
      return Promise.resolve(response(config, { status: 'ok', remember: true, user: { id: body.username, name: body.username, is_admin: true } }));
    }
    if (config.url === '/api/config') return Promise.resolve(response(config, { ai_model_name: 'A saved settings' }));
    if (config.url === '/api/portal/account-info') {
      if (state.accountDenied) return Promise.reject(new axios.AxiosError('resource rejected', 'ERR_BAD_REQUEST', config, null, { config, status: 403, data: {} }));
      if (!state.deferAccount) return Promise.resolve(response(config, state.account));
      return new Promise(resolve => state.accountReads.push(data => resolve(response(config, data))));
    }
    throw new Error(`Unexpected request: ${config.url}`);
  };
  return { state, store, restore: () => { axios.defaults.adapter = originalAdapter; invalidateConfigCache(); } };
}

for (const status of [403, 401, 500]) {
  test(`current login ${status} follows its session-clearing protocol`, async () => {
    const f = fixture();
    try {
      await f.store.login({ username: 'A', password: 'fixture-only' });
      await f.store.fetchAccountInfo();
      const config = useConfig(); await config.fetchConfigData();
      f.state.denied = status;
      await assert.rejects(f.store.login({ username: 'B', password: 'fixture-only' }));
      assert.equal(f.store.isLoggedIn, status !== 403);
      assert.equal(f.store.isAdmin, status !== 403);
      assert.equal(f.store.user.id ?? null, status === 403 ? null : 'A');
      assert.equal(config.configModel.value !== null, status !== 403);
      if (status === 403) {
        assert.equal(f.store.remember, false);
        assert.equal(f.store.accountInfo, null);
        assert.equal(f.store.systemStatus, 'login_required');
      }
    } finally { f.restore(); }
  });
}

for (const account of [{ id: 'B', profile_image_tag: 'tag-b' }, { name: 'unknown', profile_image_tag: 'tag-b' }]) {
  test(`account-info refuses an unconfirmed identity ${account.id ?? 'missing'}`, async () => {
    const f = fixture();
    try {
      await f.store.login({ username: 'A', password: 'fixture-only' });
      f.state.account = account;
      await assert.rejects(f.store.fetchAccountInfo(), /用户资料.*确认/);
      assert.equal(f.store.accountInfo, null);
      assert.equal(f.store.avatarUrl, null);
      assert.equal(f.store.user.id, 'A');
    } finally { f.restore(); }
  });
}

test('confirmed account-info retains the matching owner and avatar', async () => {
  const f = fixture();
  try {
    await f.store.login({ username: 'A', password: 'fixture-only' });
    const info = await f.store.fetchAccountInfo();
    assert.equal(info.id, 'A');
    assert.equal(f.store.accountInfo.id, 'A');
    assert.match(f.store.avatarUrl, /Users\/A\/Images\/Primary\?tag=tag-a$/);
  } finally { f.restore(); }
});

test('a resource 403 does not apply the login-specific session clearing rule', async () => {
  const f = fixture();
  try {
    await f.store.login({ username: 'A', password: 'fixture-only' });
    const config = useConfig(); await config.fetchConfigData();
    f.state.accountDenied = true;
    await assert.rejects(f.store.fetchAccountInfo());
    assert.equal(f.store.isLoggedIn, true);
    assert.equal(f.store.user.id, 'A');
    assert.notEqual(config.configModel.value, null);
  } finally { f.restore(); }
});

test('an old account-info promise cannot return the old account after a new login', async () => {
  const f = fixture();
  try {
    await f.store.login({ username: 'A', password: 'fixture-only' });
    f.state.deferAccount = true;
    const older = f.store.fetchAccountInfo();
    await f.store.login({ username: 'B', password: 'fixture-only' });
    f.state.accountReads[0]({ id: 'A', profile_image_tag: 'tag-a' });
    assert.equal(await older, null);
    assert.equal(f.store.accountInfo, null);
    assert.equal(f.store.user.id, 'B');
  } finally { f.restore(); }
});

test('a credential transition does not reuse an earlier account-info request', async () => {
  const f = fixture();
  try {
    await f.store.login({ username: 'A', password: 'fixture-only' });
    f.state.deferAccount = true;
    const older = f.store.fetchAccountInfo();
    f.state.denied = 401;
    await assert.rejects(f.store.login({ username: 'B', password: 'fixture-only' }));
    const newer = f.store.fetchAccountInfo();
    assert.equal(f.state.accountReads.length, 2);
    f.state.accountReads[1]({ id: 'A', profile_image_tag: 'fresh-tag-a' });
    await newer;
    f.state.accountReads[0]({ id: 'A', profile_image_tag: 'old-tag-a' });
    assert.equal(await older, null);
    assert.match(f.store.avatarUrl, /fresh-tag-a$/);
  } finally {
    for (const resolve of f.state.accountReads) resolve({ id: 'A' });
    f.restore();
  }
});
