import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { ref, computed, watch, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import axios from 'axios';
import { useAuthStore } from '../src/stores/auth.js';
import { invalidateConfigCache, useConfig } from '../src/composables/useConfig.js';

const source = readFileSync(new URL('../src/composables/useTaskStatus.js', import.meta.url), 'utf8')
  .replace(/^import .*;\n/gm, '').replaceAll('export function', 'function');
const running = { task_id: 'running-a', is_running: true, state: 'running', progress: 42, message: '正在处理' };
const flush = async () => { await nextTick(); await new Promise(resolve => setImmediate(resolve)); await nextTick(); };

async function fixture() {
  setActivePinia(createPinia()); invalidateConfigCache();
  const store = useAuthStore(), originalAdapter = axios.defaults.adapter;
  const state = { forbidden: false, probe: 'login_required', probeFails: false, statusGate: null, probeGate: null, probeReads: 0 };
  const intervals = new Map(), stops = [], unmounts = [];
  axios.defaults.adapter = async config => {
    let data;
    if (config.url === '/api/auth/login') {
      const body = JSON.parse(config.data);
      if (body.username === 'B' && state.loginGate) await state.loginGate;
      data = { status: 'ok', user: { id: body.username, name: body.username, is_admin: true }, remember: true };
    } else if (config.url === '/api/config') data = { ai_model_name: 'keep-until-expiry' };
    else if (config.url === '/api/status') {
      const forbidden = state.forbidden;
      if (state.statusGate) await state.statusGate;
      if (forbidden) throw new axios.AxiosError('status forbidden', 'ERR_BAD_REQUEST', config, null, { status: 403, config, data: {} });
      data = running;
    } else if (config.url === '/api/auth/check_status') {
      state.probeReads++;
      if (state.probeHttpUrl) return axios.getAdapter('http')({ ...config, url: state.probeHttpUrl });
      if (state.probeGate) await state.probeGate;
      if (state.probeFails) throw new Error('probe disconnected');
      data = state.probe === 'logged_in' ? { status: 'logged_in', user: { id: 'A', name: 'A', is_admin: true } } : { status: state.probe };
    } else throw new Error(`Unexpected request ${config.url}`);
    return { config, data, status: 200, statusText: 'OK', headers: {} };
  };
  await store.login({ username: 'A', password: 'fixture-only' });
  const config = useConfig(); await config.fetchConfigData();
  const context = vm.createContext({ axios, ref, computed, AbortController, useAuthStore: () => store,
    watch: (...args) => { const stop = watch(...args); stops.push(stop); return stop; }, onUnmounted: fn => unmounts.push(fn),
    setInterval: fn => { const id = intervals.size + 1; intervals.set(id, fn); return id; }, clearInterval: id => intervals.delete(id),
    window: { location: { reload() { throw new Error('no restart'); } } },
  });
  vm.runInContext(source + '\nthis.subscribe = useTaskStatus; this.refresh = refreshTaskStatus; this.status = backgroundTaskStatus;', context);
  context.subscribe(); await context.refresh(); await flush();
  return { store, config, state, context, intervals, restore: () => { unmounts.forEach(fn => fn()); stops.forEach(stop => stop()); axios.defaults.adapter = originalAdapter; invalidateConfigCache(); } };
}

test('an unresponsive auth probe releases the task poller without clearing a valid login', async () => {
  const f = await fixture(), sockets = new Set();
  let started, timer, reading;
  const requested = new Promise(resolve => { started = resolve; });
  const server = createServer(() => started());
  server.on('connection', socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)); });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    f.state.forbidden = true;
    f.state.probeHttpUrl = `http://127.0.0.1:${server.address().port}/api/auth/check_status`;
    let settled = false;
    reading = f.context.refresh().then(() => { settled = true; });
    await requested;
    await Promise.race([reading, new Promise(resolve => { timer = setTimeout(resolve, 11_500); })]);
    assert.equal(settled, true, 'the owned auth probe must not hold the status request indefinitely');
    assert.equal(f.store.isLoggedIn, true);
    assert.equal(f.context.status.value.task_id, 'running-a');
    f.state.forbidden = false; f.state.probeHttpUrl = null;
    await f.context.refresh(); await flush();
    assert.equal(f.context.status.value.is_running, true);
  } finally {
    clearTimeout(timer);
    for (const socket of sockets) socket.destroy();
    await new Promise(resolve => server.close(resolve));
    await reading;
    f.restore();
  }
});

test('an owned status 403 clears login and old task only after verified session expiry', async () => {
  const f = await fixture();
  try {
    f.state.forbidden = true;
    await f.context.refresh(); await flush();
    assert.equal(f.state.probeReads, 1);
    assert.equal(f.store.isLoggedIn, false);
    assert.equal(f.config.configModel.value, null);
    assert.equal(f.context.status.value.is_running, false);
    assert.equal(f.intervals.size, 0);
  } finally { f.restore(); }
});

for (const mode of ['valid session', 'probe failed']) {
  test(`${mode} after status 403 preserves login and does not invent an idle task`, async () => {
    const f = await fixture();
    try {
      f.state.forbidden = true; f.state.probe = 'logged_in'; f.state.probeFails = mode === 'probe failed';
      await f.context.refresh(); await flush();
      assert.equal(f.state.probeReads, 1);
      assert.equal(f.store.isLoggedIn, true);
      assert.equal(f.config.configModel.value.ai_model_name, 'keep-until-expiry');
      assert.equal(f.context.status.value.task_id, 'running-a');
      assert.equal(f.context.status.value.is_running, true);
      assert.equal(f.intervals.size, 1);
    } finally { f.restore(); }
  });
}

test('a late old status 403 cannot probe or clear a newer login', async () => {
  const f = await fixture(); let release;
  try {
    f.state.forbidden = true; f.state.statusGate = new Promise(resolve => { release = resolve; });
    const reading = f.context.refresh();
    await f.store.login({ username: 'B', password: 'fixture-only' });
    release(); await reading; await flush();
    assert.equal(f.state.probeReads, 0);
    assert.equal(f.store.isLoggedIn, true);
    assert.equal(f.store.user.id, 'B');
    assert.equal(f.context.status.value.task_id, 'running-a');
  } finally { release?.(); f.restore(); }
});

test('a late auth probe cannot clear a newer login after an owned status 403', async () => {
  const f = await fixture(); let release;
  try {
    f.state.forbidden = true; f.state.probeGate = new Promise(resolve => { release = resolve; });
    const reading = f.context.refresh(); await flush();
    assert.equal(f.state.probeReads, 1);
    await f.store.login({ username: 'B', password: 'fixture-only' });
    release(); await reading; await flush();
    assert.equal(f.store.isLoggedIn, true);
    assert.equal(f.store.user.id, 'B');
    assert.equal(f.context.status.value.is_running, true);
  } finally { release?.(); f.restore(); }
});

test('a status 403 cannot start a probe while the current owner is changing login', async () => {
  const f = await fixture(); let release;
  try {
    f.state.forbidden = true; f.state.loginGate = new Promise(resolve => { release = resolve; });
    const signingIn = f.store.login({ username: 'B', password: 'fixture-only' });
    await f.context.refresh(); await flush();
    assert.equal(f.state.probeReads, 0);
    assert.equal(f.store.isLoggedIn, true);
    assert.equal(f.store.user.id, 'A');
    assert.equal(f.context.status.value.is_running, true);
    release(); await signingIn;
    assert.equal(f.store.user.id, 'B');
  } finally { release?.(); f.restore(); }
});
