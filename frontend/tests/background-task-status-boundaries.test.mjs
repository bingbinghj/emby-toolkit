import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import axios, { AxiosError } from 'axios';
import { computed, reactive, ref, watch } from 'vue';

const load = path => readFileSync(new URL(path, import.meta.url), 'utf8')
  .replace(/^import .*;\n/gm, '').replaceAll('export function', 'function');
const statusSource = load('../src/composables/useTaskStatus.js');
const interceptorSource = load('../src/utils/backgroundTasks.js');
const deferred = () => {
  let resolve;
  const promise = new Promise(yes => { resolve = yes; });
  return { promise, resolve };
};
const flush = async () => { for (let step = 0; step < 6; step++) await new Promise(resolve => setImmediate(resolve)); };
const idle = { is_running: false, state: 'idle', current_action: '无', progress: 0, history: [] };
const running = { task_id: 'new-task', is_running: true, state: 'running', current_action: '新任务',
  progress: 42, progress_known: true, history: [] };
const completed = { ...running, is_running: false, state: 'completed', current_action: '空闲',
  progress: 100, result_status: 200, result: { message: '完成' } };

function fixture(t, delayedStatus) {
  const oldStatus = deferred(), terminal = deferred(), requests = [], stops = [], unmounts = [];
  const auth = reactive({ isLoggedIn: true, isAdmin: true });
  let server = { ...idle }, statusReads = 0, resultReads = 0, operation;
  const client = axios.create({ adapter: async config => {
    requests.push({ method: config.method, path: config.url });
    let data;
    if (config.method === 'post') {
      server = { ...running };
      return { data: { task_id: running.task_id }, status: 202, statusText: 'Accepted', headers: {}, config };
    }
    if (config.url === '/api/status') {
      data = { ...server };
      if (statusReads++ === delayedStatus) await oldStatus.promise;
    } else if (config.url === `/api/tasks/result/${running.task_id}`) {
      if (++resultReads === 2) await terminal.promise;
      data = { ...server };
    } else throw new Error(`Unexpected request ${config.url}`);
    return { data, status: 200, statusText: 'OK', headers: {}, config };
  } });
  const context = vm.createContext({
    axios: client, AxiosError, ref, computed, AbortController,
    watch: (...args) => { const stop = watch(...args); stops.push(stop); return stop; },
    useAuthStore: () => auth, onUnmounted: callback => unmounts.push(callback),
    setInterval: () => 1, clearInterval() {}, setTimeout: callback => queueMicrotask(callback),
    window: { location: { reload() { throw new Error('No restart occurred'); } } },
  });
  vm.runInContext(`${statusSource}\n${interceptorSource}\ninstallBackgroundTaskInterceptor();
    this.subscribe = useTaskStatus; this.refresh = refreshTaskStatus; this.latest = backgroundTaskStatus;`, context);
  context.subscribe();
  const bootstrap = context.refresh();
  t.after(async () => {
    server = { ...completed }; oldStatus.resolve(); terminal.resolve();
    await operation?.catch(() => {}); await flush();
    unmounts.forEach(callback => callback()); stops.forEach(stop => stop());
  });
  return {
    context, bootstrap, oldStatus, requests,
    statusReads: () => statusReads,
    async run() { operation = client.post('/api/tasks/run', {}); await flush(); assert.equal(resultReads, 2); },
    async finish() { server = { ...completed }; terminal.resolve(); const result = await operation; await flush(); return result; },
  };
}

test('accepted task invalidates a delayed status snapshot from before submission', async t => {
  const f = fixture(t, 0);
  await f.run();
  f.oldStatus.resolve(); await f.bootstrap; await flush();
  assert.equal(f.context.latest.value.task_id, running.task_id);
  assert.equal(f.context.latest.value.is_running, true);
  assert.equal(f.statusReads(), 2, 'acceptance must read a fresh global status after the obsolete one');
  const result = await f.finish();
  assert.equal(result.data.message, '完成');
  assert.equal(f.context.latest.value.is_running, false);
  assert.equal(f.requests.filter(request => request.method === 'post').length, 1);
});

test('completed task invalidates an in-flight running snapshot instead of resurrecting it', async t => {
  const f = fixture(t, 1);
  await f.bootstrap; await f.run();
  assert.equal((await f.finish()).data.message, '完成');
  f.oldStatus.resolve(); await flush();
  assert.equal(f.context.latest.value.state, 'completed');
  assert.equal(f.context.latest.value.is_running, false);
  assert.equal(f.statusReads(), 3, 'completion must reread after the stale running response');
  assert.equal(f.requests.filter(request => request.method === 'post').length, 1);
});

test('acceptance and completion sharing a blocked status read coalesce into one fresh terminal read', async t => {
  const f = fixture(t, 0);
  await f.run(); await f.finish();
  f.oldStatus.resolve(); await f.bootstrap; await flush();
  assert.equal(f.context.latest.value.state, 'completed');
  assert.equal(f.context.latest.value.is_running, false);
  assert.equal(f.statusReads(), 2);
  assert.equal(f.requests.filter(request => request.method === 'post').length, 1);
});
