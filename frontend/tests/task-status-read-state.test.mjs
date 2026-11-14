import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { computed, nextTick, reactive, ref, watch } from 'vue';

const source = readFileSync(new URL('../src/composables/useTaskStatus.js', import.meta.url), 'utf8')
  .replace(/^import .*;\n/gm, '').replaceAll('export function', 'function');
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const running = { task_id: 'confirmed-task', is_running: true, state: 'running', progress: 37, message: '正在处理' };

function fixture(t, get) {
  const auth = reactive({ isLoggedIn: true, isAdmin: true, user: { id: 'user-a' } });
  const unmounts = [], stops = [];
  let clock = 1000;
  const context = vm.createContext({
    axios: { get }, ref, computed, AbortController, Date: { now: () => clock },
    useAuthStore: () => auth, onUnmounted: callback => unmounts.push(callback),
    watch: (...args) => { const stop = watch(...args); stops.push(stop); return stop; },
    setInterval: () => 1, clearInterval() {}, window: { location: { reload() { throw new Error('unexpected reload'); } } },
  });
  vm.runInContext(source + '\nthis.subscribe = useTaskStatus; this.refresh = refreshTaskStatus; this.status = backgroundTaskStatus; this.readAccessor = typeof useTaskStatusReadState === "function" ? useTaskStatusReadState : () => ({ error: ref(""), lastUpdatedAt: ref(null) }); this.readState = this.readAccessor(); this.subscriberCount = () => subscribers;', context);
  context.subscribe();
  t.after(() => { unmounts.forEach(callback => callback()); stops.forEach(stop => stop()); });
  return { context, auth, unmount: () => unmounts.forEach(callback => callback()), time: value => { clock = value; } };
}

test('first failed status read exposes an unconfirmed error without creating a confirmed idle snapshot', async t => {
  const f = fixture(t, async () => { throw { response: { status: 503, data: { error: '状态服务暂时不可用' } } }; });
  await f.context.refresh();
  assert.equal(f.context.readState.error.value, '状态服务暂时不可用');
  assert.equal(f.context.readState.lastUpdatedAt.value, null);
  assert.equal(f.context.status.value.state, 'idle');
});

test('failed reads preserve the last confirmed running snapshot and a successful retry clears the error', async t => {
  let failure = false;
  const f = fixture(t, async () => {
    if (failure) throw new Error('连接中断');
    return { data: running };
  });
  await f.context.refresh();
  assert.equal(f.context.readState.lastUpdatedAt.value, 1000);
  failure = true; f.time(2000); await f.context.refresh();
  assert.equal(f.context.status.value.task_id, running.task_id);
  assert.equal(f.context.status.value.progress, 37);
  assert.equal(f.context.readState.error.value, '连接中断');
  assert.equal(f.context.readState.lastUpdatedAt.value, 1000);
  failure = false; f.time(3000); await f.context.refresh();
  assert.equal(f.context.readState.error.value, '');
  assert.equal(f.context.readState.lastUpdatedAt.value, 3000);
});

test('an obsolete failed response cannot overwrite a fresh status read after a control action', async t => {
  const old = deferred(); let reads = 0;
  const f = fixture(t, () => ++reads === 1 ? old.promise : Promise.resolve({ data: running }));
  const reading = f.context.refresh();
  f.time(2000); await f.context.refresh(true);
  old.reject(new Error('旧请求失联')); await reading;
  assert.equal(f.context.readState.error.value, '');
  assert.equal(f.context.readState.lastUpdatedAt.value, 2000);
  assert.equal(f.context.status.value.task_id, running.task_id);
});

test('a late failed read after a user change or the last unmount cannot publish an error', async t => {
  for (const boundary of ['user', 'unmount']) {
    const pending = deferred();
    const f = fixture(t, () => pending.promise);
    const reading = f.context.refresh();
    if (boundary === 'user') f.auth.user = { id: 'user-b' };
    else f.unmount();
    pending.reject(new Error('旧会话失联')); await reading;
    assert.equal(f.context.readState.error.value, '');
    assert.equal(f.context.readState.lastUpdatedAt.value, null);
  }
});

test('read state has no extra subscriptions and permission loss clears its old-session error', async t => {
  const f = fixture(t, async () => { throw new Error('旧会话读取失败'); });
  await f.context.refresh();
  const subscribers = f.context.subscriberCount();
  assert.equal(f.context.readAccessor().error, f.context.readState.error);
  assert.equal(f.context.subscriberCount(), subscribers);
  assert.equal(f.context.readState.error.value, '旧会话读取失败');
  f.auth.isLoggedIn = false; await nextTick();
  assert.equal(f.context.readState.error.value, '');
  assert.equal(f.context.readState.lastUpdatedAt.value, null);
});
