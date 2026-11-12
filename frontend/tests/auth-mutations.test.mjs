import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createPinia, setActivePinia } from 'pinia';
import axios from 'axios';
import { useAuthStore } from '../src/stores/auth.js';

const result = { status: 'ok', user: { id: 'B', name: 'B', is_admin: true }, remember: true };
const loginSource = readFileSync(new URL('../src/components/Login.vue', import.meta.url), 'utf8');

function fixture({ cancel = true } = {}) {
  setActivePinia(createPinia());
  const store = useAuthStore(), requests = [];
  const original = axios.post;
  axios.post = (path, body, config) => new Promise((resolve, reject) => {
    const request = { path, body, signal: config?.signal, resolve, reject };
    if (cancel) request.signal?.addEventListener('abort', () => reject(new axios.CanceledError('superseded')), { once: true });
    requests.push(request);
  });
  return { store, requests, restore: () => { axios.post = original; } };
}

for (const [olderKind, newerKind] of [['login', 'logout'], ['logout', 'login'], ['login', 'login']]) {
  test(`new ${newerKind} cancels old ${olderKind} and only the applied mutation returns true`, async () => {
    const f = fixture();
    try {
      const older = f.store[olderKind]({ username: 'A', password: 'fixture-only' });
      const settledOld = older.then(value => value, error => error);
      const newer = f.store[newerKind]({ username: 'B', password: 'fixture-only' });
      if (!f.requests[0].signal?.aborted) f.requests[0].reject(new axios.CanceledError('superseded'));
      f.requests[1].resolve({ data: result });
      const newApplied = await newer;
      const oldApplied = await settledOld;
      assert.equal(oldApplied, false);
      assert.equal(newApplied, true);
      assert.equal(f.requests[0].signal?.aborted, true);
      assert.equal(f.requests[1].signal?.aborted, false);
      assert.equal(f.store.isLoggedIn, newerKind === 'login');
      assert.equal(f.store.user.id ?? null, newerKind === 'login' ? 'B' : null);
    } finally { f.restore(); }
  });
}

test('a late successful logout explicitly returns false after a newer login', async () => {
  const f = fixture({ cancel: false });
  try {
    const leaving = f.store.logout();
    const signingIn = f.store.login({ username: 'B', password: 'fixture-only' });
    f.requests[1].resolve({ data: result });
    assert.equal(await signingIn, true);
    f.requests[0].resolve({ data: { status: 'ok' } });
    assert.equal(await leaving, false);
    assert.equal(f.store.user.id, 'B');
  } finally { f.restore(); }
});

test('a superseded login cannot announce success or navigate from the actual Login handler', async () => {
  const f = fixture({ cancel: false });
  const routes = [], successes = [];
  const code = loginSource.match(/^async function handleLogin\(\) \{[\s\S]*?^}/m)[0];
  const handle = new Function('authStore', 'formRef', 'credentials', 'remember', 'loading', 'errorMessage', 'router', 'message', 'isPageActive', code + '\nreturn handleLogin;')(
    f.store, { value: { validate: async () => ({ valid: true }) } }, { value: { username: 'A', password: 'fixture-only' } }, { value: true },
    { value: false }, { value: '' }, { push: route => routes.push(route.name) }, { success: message => successes.push(message) }, true,
  );
  try {
    const older = handle();
    await Promise.resolve();
    const leaving = f.store.logout();
    f.requests[1].resolve({ data: { status: 'ok' } });
    await leaving;
    f.requests[0].resolve({ data: result });
    await older;
    assert.deepEqual(routes, []);
    assert.deepEqual(successes, []);
    assert.equal(f.store.isLoggedIn, false);
  } finally { f.restore(); }
});

test('the actual Axios adapter receives cancellation of a superseded auth request', async () => {
  setActivePinia(createPinia());
  const store = useAuthStore(), requests = [];
  const originalAdapter = axios.defaults.adapter;
  axios.defaults.adapter = config => new Promise((resolve, reject) => {
    requests.push({ config, resolve });
    config.signal?.addEventListener('abort', () => reject(new axios.CanceledError('superseded', config)), { once: true });
  });
  try {
    const older = store.logout();
    const oldResult = older.then(value => value, error => error);
    const signingIn = store.login({ username: 'B', password: 'fixture-only' });
    requests[1].resolve({ data: result, status: 200, statusText: 'OK', headers: {}, config: requests[1].config });
    assert.equal(await signingIn, true);
    assert.equal(await oldResult, false);
    assert.equal(requests[0].config.signal.aborted, true);
    assert.equal(requests[1].config.signal.aborted, false);
    assert.equal(store.user.id, 'B');
  } finally { axios.defaults.adapter = originalAdapter; }
});
