import test from 'node:test';
import assert from 'node:assert/strict';
import axios from 'axios';
import { readFileSync } from 'node:fs';
import { shallowReactive } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { useAuthStore } from '../src/stores/auth.js';
import { feedbackDialogs } from '../src/composables/feedbackDialogs.js';

// Vite resolves the browser toast entry; use a local toast sink in Node while
// evaluating the production dialog handlers against the shared state.
const source = readFileSync(new URL('../src/composables/useFeedback.js', import.meta.url), 'utf8')
  .replace(/^import .*$/gm, '')
  .replace(/^export const feedbackDialogs = shallowReactive\(\[\]\)\s*$/m, '')
  .replace(/^export \{ feedbackDialogs \}\s*$/m, '')
  .replace(/^export /gm, '');
const toastCalls = [];
const toast = Object.assign(content => { toastCalls.push(content); return 'toast'; }, { dismiss() {}, update() {}, clear() {} });
const { answerDialog, useDialog } = new Function('shallowReactive', 'useToast', 'feedbackDialogs',
  source + '\nreturn { answerDialog, useDialog };')(shallowReactive, () => toast, feedbackDialogs);

const deferred = () => {
  let resolve, reject;
  const promise = new Promise((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
};

function fixture(t) {
  setActivePinia(createPinia());
  feedbackDialogs.splice(0);
  toastCalls.length = 0;
  t.after(() => feedbackDialogs.splice(0));
  const response = { id: 'A', name: 'A', is_admin: true };
  t.mock.method(axios, 'get', async () => ({ data: { status: 'logged_in', user: { ...response } } }));
  t.mock.method(axios, 'post', async (path, body) => ({ data: path.endsWith('/login')
    ? { user: { id: body.username, name: body.username, is_admin: true } }
    : { status: 'ok' } }));
  return { store: useAuthStore(), response, login: username => ({ username, password: 'fixture-only' }) };
}

test('a login transition removes an earlier account confirmation before the request completes', async t => {
  const f = fixture(t);
  await f.store.login(f.login('A'));
  let writes = 0;
  useDialog().warning({ title: 'A media', onPositiveClick: () => { writes++; } });
  const old = feedbackDialogs[0], signingIn = deferred();
  t.mock.method(axios, 'post', () => signingIn.promise);
  const pending = f.store.login(f.login('B'));
  assert.equal(feedbackDialogs.length, 0);
  await answerDialog(old, true);
  assert.equal(writes, 0);
  signingIn.resolve({ data: { user: { id: 'B', name: 'B', is_admin: true } } });
  await pending;
  assert.equal(f.store.user.id, 'B');
});

test('a confirmation from a logged-out account cannot issue its action', async t => {
  const f = fixture(t);
  await f.store.login(f.login('A'));
  let writes = 0;
  useDialog().warning({ onPositiveClick: () => { writes++; } });
  const old = feedbackDialogs[0];
  await f.store.logout();
  await answerDialog(old, true);
  assert.equal(feedbackDialogs.length, 0);
  assert.equal(writes, 0);
  assert.equal(f.store.isLoggedIn, false);
});

test('an ordinary status refresh preserves the same account confirmation', async t => {
  const f = fixture(t);
  await f.store.login(f.login('A'));
  let writes = 0;
  useDialog().warning({ onPositiveClick: () => { writes++; } });
  const current = feedbackDialogs[0];
  await f.store.checkAuthStatus();
  assert.equal(feedbackDialogs[0], current);
  await answerDialog(current, true);
  assert.equal(writes, 1);
  assert.equal(feedbackDialogs.length, 0);
});

test('a status-confirmed identity change removes the previous account dialog', async t => {
  const f = fixture(t);
  await f.store.login(f.login('A'));
  useDialog().warning({ title: 'Private A media', onPositiveClick: () => {} });
  f.response.id = 'B';
  await f.store.checkAuthStatus();
  assert.equal(f.store.user.id, 'B');
  assert.equal(feedbackDialogs.length, 0);
});

test('a revoked confirmation completion cannot call its close handler or remove a new dialog', async t => {
  const f = fixture(t);
  await f.store.login(f.login('A'));
  const pending = deferred();
  let closed = 0;
  useDialog().warning({ onPositiveClick: () => pending.promise, onClose: () => { closed++; } });
  const old = feedbackDialogs[0], answering = answerDialog(old, true);
  await f.store.login(f.login('B'));
  useDialog().info({ title: 'B confirmation', onPositiveClick: () => {} });
  const current = feedbackDialogs.at(-1);
  pending.resolve();
  await answering;
  assert.equal(closed, 0);
  assert.equal(feedbackDialogs.length, 1);
  assert.equal(feedbackDialogs[0], current);
  assert.equal(current.busy, false);
});

test('destroying a revoked dialog handle cannot execute its old close callback', async t => {
  const f = fixture(t);
  await f.store.login(f.login('A'));
  let closed = 0;
  const old = useDialog().info({ onClose: () => { closed++; } });
  await f.store.login(f.login('B'));
  old.destroy();
  assert.equal(closed, 0);
  assert.equal(feedbackDialogs.length, 0);
});

test('a same-account permission change revokes the earlier confirmation', async t => {
  const f = fixture(t);
  await f.store.login(f.login('A'));
  let writes = 0;
  useDialog().warning({ onPositiveClick: () => { writes++; } });
  const old = feedbackDialogs[0];
  f.response.is_admin = false;
  await f.store.checkAuthStatus();
  await answerDialog(old, true);
  assert.equal(f.store.isAdmin, false);
  assert.equal(feedbackDialogs.length, 0);
  assert.equal(writes, 0);
});

test('an expired session clears the dialog that would cover the login page', async t => {
  const f = fixture(t);
  await f.store.login(f.login('A'));
  useDialog().warning({ title: 'Earlier media', onPositiveClick: () => {} });
  t.mock.method(axios, 'get', async () => ({ data: { status: 'login_required' } }));
  await f.store.checkAuthStatus();
  assert.equal(f.store.isLoggedIn, false);
  assert.equal(feedbackDialogs.length, 0);
});

for (const revoked of [false, true]) {
  test(`a pending confirmation failure ${revoked ? 'does not publish into another account' : 'still reports its current error'}`, async t => {
    const f = fixture(t);
    await f.store.login(f.login('A'));
    const pending = deferred();
    useDialog().warning({ onPositiveClick: () => pending.promise });
    const old = feedbackDialogs[0], answering = answerDialog(old, true);
    if (revoked) await f.store.login(f.login('B'));
    pending.reject(new Error('Earlier operation failed'));
    await answering;
    assert.deepEqual(toastCalls, revoked ? [] : ['Earlier operation failed']);
    if (!revoked) assert.equal(old.busy, false);
  });
}
