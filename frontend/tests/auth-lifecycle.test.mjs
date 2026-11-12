import test from 'node:test';
import assert from 'node:assert/strict';
import { createPinia, setActivePinia } from 'pinia';
import axios from 'axios';
import { useAuthStore } from '../src/stores/auth.js';
import { invalidateConfigCache, useConfig } from '../src/composables/useConfig.js';

const login = { status: 'ok', remember: true, user: { id: 'account-b', name: 'B', is_admin: true } };

for (const timing of ['before login', 'during login']) {
  for (const oldResult of ['login_required', 'old account', 'request failure']) {
    test(`a status ${oldResult} started ${timing} cannot overwrite successful login`, async () => {
      setActivePinia(createPinia());
      const store = useAuthStore();
      const originalGet = axios.get, originalPost = axios.post;
      let releaseStatus, rejectStatus, releaseLogin;
      axios.get = () => new Promise((resolve, reject) => { releaseStatus = resolve; rejectStatus = reject; });
      axios.post = () => new Promise(resolve => { releaseLogin = resolve; });
      try {
        let status, signingIn;
        if (timing === 'before login') {
          status = store.checkAuthStatus();
          signingIn = store.login({ username: 'B', password: 'fixture-only' });
        } else {
          signingIn = store.login({ username: 'B', password: 'fixture-only' });
          status = store.checkAuthStatus();
        }
        releaseLogin({ data: login });
        await signingIn;
        if (oldResult === 'request failure') rejectStatus(new Error('old connection failed'));
        else releaseStatus({ data: oldResult === 'old account'
          ? { status: 'logged_in', user: { id: 'account-a', name: 'A', is_admin: true } }
          : { status: 'login_required' } });
        assert.equal(await status, 'logged_in');
        assert.equal(store.isLoggedIn, true);
        assert.equal(store.user.id, 'account-b');
        assert.equal(store.remember, true);
      } finally {
        axios.get = originalGet; axios.post = originalPost;
      }
    });
  }
}

test('a late logged-in status cannot restore a logged-out account', async () => {
  setActivePinia(createPinia());
  const store = useAuthStore();
  const originalGet = axios.get, originalPost = axios.post;
  let releaseStatus;
  axios.get = () => new Promise(resolve => { releaseStatus = resolve; });
  axios.post = async () => ({ data: login });
  try {
    await store.login({ username: 'B', password: 'fixture-only' });
    const status = store.checkAuthStatus();
    await store.logout();
    releaseStatus({ data: { status: 'logged_in', user: login.user } });
    assert.equal(await status, 'login_required');
    assert.equal(store.isLoggedIn, false);
    assert.deepEqual(store.user, {});
    assert.equal(store.remember, false);
  } finally {
    axios.get = originalGet; axios.post = originalPost;
  }
});

test('a newer status result keeps ownership after an older request completes', async () => {
  setActivePinia(createPinia());
  const store = useAuthStore();
  const originalGet = axios.get;
  const releases = [];
  axios.get = () => new Promise(resolve => releases.push(resolve));
  try {
    const older = store.checkAuthStatus(), newer = store.checkAuthStatus();
    releases[1]({ data: { status: 'logged_in', user: login.user, remember: true } });
    await newer;
    releases[0]({ data: { status: 'login_required' } });
    assert.equal(await older, 'logged_in');
    assert.equal(store.isLoggedIn, true);
    assert.equal(store.user.id, 'account-b');
  } finally { axios.get = originalGet; }
});

test('session changes clear cached settings while the same account keeps its draft', async () => {
  setActivePinia(createPinia());
  invalidateConfigCache();
  const store = useAuthStore();
  const originalGet = axios.get, originalPost = axios.post;
  let configReads = 0, serverValue = 'A';
  axios.get = async path => {
    if (path === '/api/config') { configReads++; return { data: { ai_model_name: serverValue } }; }
    return { data: { status: 'logged_in', user: login.user, remember: true } };
  };
  axios.post = async (path, body) => ({ data: path === '/api/auth/login'
    ? { ...login, user: { ...login.user, id: body.username === 'A' ? 'account-a' : 'account-b' } }
    : { status: 'ok' } });
  try {
    await store.login({ username: 'A', password: 'fixture-only' });
    const settings = useConfig();
    await settings.fetchConfigData();
    settings.configModel.value.ai_model_name = 'A unsaved draft';
    serverValue = 'B';
    await store.login({ username: 'B', password: 'fixture-only' });
    assert.equal(settings.configModel.value, null);
    await settings.fetchConfigData();
    assert.equal(configReads, 2);
    assert.equal(settings.configModel.value.ai_model_name, 'B');
    settings.configModel.value.ai_model_name = 'B unsaved draft';
    await store.checkAuthStatus();
    assert.equal(settings.configModel.value.ai_model_name, 'B unsaved draft');
    await store.logout();
    assert.equal(settings.configModel.value, null);
  } finally {
    axios.get = originalGet; axios.post = originalPost;
    invalidateConfigCache();
  }
});
