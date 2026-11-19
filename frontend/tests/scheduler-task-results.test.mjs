import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { computed, nextTick, reactive, ref, watch } from 'vue';
import { compileScript, compileTemplate, parse } from '@vue/compiler-sfc';

const source = readFileSync(new URL('../src/components/settings/SchedulerSettingsPage.vue', import.meta.url), 'utf8');
const shellSource = readFileSync(new URL('../src/MainLayout.vue', import.meta.url), 'utf8');
const script = source.match(/<script setup>([\s\S]*?)<\/script>/)[1].replace(/^import[^\n]+\n/gm, '');
const terminal = (taskId = 'original-task', state = 'failed') => ({
  task_id: taskId, state, is_running: false, last_action: '翻译演员', current_action: '空闲',
  message: '持久结果中的消息', result_status: state === 'failed' ? 500 : 207,
  result: { message: '批处理结束', failed: state === 'failed' ? 1 : 0, warnings: 1,
    failures: state === 'failed' ? [{ item_id: 'media-A', reason: '姓名译文缺失' }] : [],
    warning_details: ['图片暂未恢复'] },
});
const summary = task => Object.fromEntries(Object.entries(task).filter(([key]) => key !== 'result'));
const flush = async () => { await nextTick(); await new Promise(resolve => setImmediate(resolve)); await nextTick(); };
const clone = value => JSON.parse(JSON.stringify(value));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };

function fixture(t, { status = summary(terminal()), get = async () => ({ data: terminal() }) } = {}) {
  const calls = [], messages = [], stops = [], unmounts = [];
  const auth = reactive({ isLoggedIn: true, isAdmin: true, user: { id: 'admin-A' } });
  const background = ref(status);
  const config = { configModel: ref({ tg_menu_tasks: [], manual_task_order: [] }), loadingConfig: ref(false),
    configError: ref(''), handleSaveConfig: async () => true };
  const context = vm.createContext({
    ref, computed, nextTick, AbortController,
    watch: (...args) => { const stop = watch(...args); stops.push(stop); return stop; },
    onMounted: () => {}, onBeforeUnmount: callback => unmounts.push(callback),
    useDisplay: () => ({ mdAndUp: ref(true) }), useContextSearch() {},
    useConfig: () => config, useAuthStore: () => auth,
    useTaskStatus: () => ({ backgroundTaskStatus: background, isBackgroundTaskRunning: computed(() => background.value.is_running), refreshTaskStatus: async () => {} }),
    useMessage: () => Object.fromEntries(['success', 'warning', 'info', 'error'].map(type => [type, message => messages.push({ type, message })])),
    useDialog: () => ({}),
    axios: { get: (url, options) => { calls.push({ url, options }); return get(url, options); }, post: async () => ({ status: 200, data: {} }) },
    Sortable: { create: () => ({ destroy() {}, option() {} }) },
  });
  vm.runInContext(`${script}\nthis.api = {
    read: typeof readTaskResult === 'undefined' ? null : readTaskResult,
    open: typeof openTaskResult === 'undefined' ? null : openTaskResult,
    close: typeof closeTaskResult === 'undefined' ? null : closeTaskResult,
    result: typeof taskResult === 'undefined' ? null : taskResult,
    target: typeof taskResultTarget === 'undefined' ? null : taskResultTarget,
    loading: typeof taskResultLoading === 'undefined' ? null : taskResultLoading,
    error: typeof taskResultError === 'undefined' ? null : taskResultError,
    feedback: typeof taskResultFeedback === 'undefined' ? null : taskResultFeedback,
    showRunResult: showTaskRunResult, runFeedback: taskRunFeedback, triggering: isTriggeringTask,
    label: currentTaskStateLabel, paused: taskPaused, stopping: taskStopping,
    pausing: typeof taskPausing === 'undefined' ? null : taskPausing,
  };`, context);
  let disposed = false;
  const dispose = () => {
    if (disposed) return;
    disposed = true;
    unmounts.forEach(callback => callback()); stops.forEach(stop => stop());
  };
  t.after(dispose);
  return { api: context.api, background, auth, calls, messages, dispose };
}

test('revisiting the task page reads the original terminal task and restores full failures and warnings', async t => {
  const f = fixture(t);
  await flush();
  assert.deepEqual(f.calls.map(call => call.url), ['/api/tasks/result/original-task']);
  assert.equal(f.api.result.value.task_id, 'original-task');
  assert.equal(f.api.result.value.state, 'failed');
  assert.equal(f.api.feedback.value.type, 'error');
  assert.match(f.api.feedback.value.message, /失败/);
  assert.deepEqual(clone(f.api.feedback.value.details), ['media-A：姓名译文缺失', '图片暂未恢复']);
  assert.equal(f.messages.length, 0, 'restoring a result must not replay completion toasts');
});

for (const [state, type] of [['completed', 'success'], ['completed_with_warnings', 'warning'], ['cancelled', 'warning']]) {
  test(`a restored ${state} task keeps its terminal meaning`, async t => {
    const task = terminal('saved-task', state);
    if (state === 'completed') { task.result_status = 200; task.result.warnings = 0; task.result.warning_details = []; }
    const f = fixture(t, { status: summary(task), get: async () => ({ data: task }) });
    await flush();
    assert.equal(f.api.result.value.state, state);
    assert.equal(f.api.feedback.value.type, type);
    assert.match(f.api.feedback.value.message, state === 'cancelled' ? /已停止/ : /已完成/);
    assert.equal(f.messages.length, 0);
  });
}

for (const state of ['idle', 'running', 'pausing', 'paused', 'stopping', 'interrupted']) {
  test(`the ${state} summary never causes a completion read`, async t => {
    const f = fixture(t, { status: { task_id: 'unfinished-task', state, is_running: false } });
    await flush();
    assert.equal(f.calls.length, 0);
    assert.equal(f.api.result.value, null);
    assert.equal(f.api.feedback.value, null);
  });
}

test('a busy task with a stale completed label is not treated as terminal', async t => {
  const f = fixture(t, { status: { ...summary(terminal()), state: 'completed', is_running: true } });
  await flush();
  assert.equal(f.calls.length, 0);
  assert.equal(f.api.feedback.value, null);
});

test('missing full results never fall back to the saved status summary and can be retried', async t => {
  let missing = true;
  const f = fixture(t, { get: async () => ({ data: missing ? summary(terminal()) : terminal() }) });
  await flush();
  assert.equal(f.api.result.value, null);
  assert.equal(f.api.feedback.value, null);
  assert.match(f.api.error.value, /详情缺失/);
  missing = false;
  await f.api.read();
  assert.equal(f.calls.length, 2);
  assert.equal(f.api.error.value, '');
  assert.equal(f.api.result.value.task_id, 'original-task');
});

test('a failed result request is visible and a same-component retry restores its details', async t => {
  let failed = true;
  const f = fixture(t, { get: async () => {
    if (failed) throw { response: { status: 404, data: { error: '任务结果不存在' } } };
    return { data: terminal() };
  } });
  await flush();
  assert.equal(f.api.loading.value, false);
  assert.equal(f.api.error.value, '任务结果不存在');
  assert.equal(f.api.feedback.value, null);
  failed = false;
  await f.api.read();
  assert.equal(f.api.feedback.value.type, 'error');
  assert.equal(f.api.error.value, '');
});

test('a result for another task ID is refused even if it arrived from the selected URL', async t => {
  const f = fixture(t, { get: async () => ({ data: terminal('unrelated-task') }) });
  await flush();
  assert.equal(f.api.result.value, null);
  assert.equal(f.api.feedback.value, null);
  assert.match(f.api.error.value, /所选任务不一致/);
});

test('a fresh running or paused response is never presented as a completed result', async t => {
  for (const state of ['running', 'paused']) {
    const f = fixture(t, { get: async () => ({ data: { ...terminal(), state, is_running: state === 'running' } }) });
    await flush();
    assert.equal(f.api.result.value, null);
    assert.equal(f.api.feedback.value, null);
    assert.match(f.api.error.value, /尚未结束/);
    f.dispose();
  }
});

test('late reads from the former current task cannot overwrite or unlock the replacement task result', async t => {
  const old = deferred(), replacement = deferred();
  const f = fixture(t, { get: url => url.endsWith('original-task') ? old.promise : replacement.promise });
  f.background.value = summary(terminal('replacement-task', 'completed_with_warnings'));
  await flush();
  assert.equal(f.calls[0].options.signal.aborted, true);
  assert.equal(f.calls[1].url, '/api/tasks/result/replacement-task');
  old.resolve({ data: terminal() });
  await flush();
  assert.equal(f.api.loading.value, true);
  assert.equal(f.api.result.value, null);
  replacement.resolve({ data: terminal('replacement-task', 'completed_with_warnings') });
  await flush();
  assert.equal(f.api.result.value.task_id, 'replacement-task');
  assert.equal(f.api.loading.value, false);
});

test('result selection uses its original ID and switching tasks prevents late errors from replacing the new result', async t => {
  const first = deferred(), second = deferred();
  const histories = [summary(terminal('history/A')), summary(terminal('history-B', 'completed_with_warnings'))];
  const f = fixture(t, { status: { task_id: 'busy-task', state: 'running', is_running: true, history: histories },
    get: url => url.endsWith('history%2FA') ? first.promise : second.promise });
  const firstRead = f.api.open(histories[0]);
  const secondRead = f.api.open(histories[1]);
  assert.equal(f.calls[0].url, '/api/tasks/result/history%2FA');
  assert.equal(f.calls[0].options.signal.aborted, true);
  first.reject(new Error('旧历史请求失败'));
  await firstRead;
  assert.equal(f.api.error.value, '');
  assert.equal(f.api.loading.value, true);
  f.background.value = { task_id: 'another-busy-task', state: 'running', is_running: true, history: histories };
  second.resolve({ data: terminal('history-B', 'completed_with_warnings') });
  await secondRead;
  assert.equal(f.api.result.value.task_id, 'history-B', 'a new background task must not change the explicitly selected history');
  assert.equal(f.api.feedback.value.type, 'warning');
});

test('closing and reopening the same task rereads its persisted result in the same viewer', async t => {
  let reads = 0;
  const task = summary(terminal('history-task'));
  const f = fixture(t, { status: { state: 'idle', is_running: false, history: [task] }, get: async () => {
    const data = terminal('history-task'); data.result.warning_details = [`提醒 ${++reads}`]; return { data };
  } });
  await f.api.open(task);
  assert.ok(f.api.feedback.value.details.includes('提醒 1'));
  f.api.close();
  await f.api.open(task);
  assert.equal(reads, 2);
  assert.ok(f.api.feedback.value.details.includes('提醒 2'));
  assert.ok(!f.api.feedback.value.details.includes('提醒 1'));
});

test('selecting the latest history keeps that selected ID when a new current task starts', async t => {
  const pending = deferred(), task = summary(terminal());
  let reads = 0;
  const f = fixture(t, { status: { ...task, history: [task] }, get: () => ++reads === 1 ? Promise.resolve({ data: terminal() }) : pending.promise });
  await flush();
  const selected = f.api.open(task);
  f.background.value = { task_id: 'new-current-task', state: 'running', is_running: true, history: [task] };
  assert.equal(f.calls[1].options.signal.aborted, false);
  pending.resolve({ data: terminal() });
  await selected;
  assert.equal(f.api.target.value.task_id, 'original-task');
  assert.equal(f.api.result.value.task_id, 'original-task');
  assert.equal(f.api.loading.value, false);
});

test('viewing the latest history during a new local submission completes its result read', async t => {
  const task = summary(terminal());
  const f = fixture(t, { status: { ...task, history: [task] } });
  await flush();
  f.api.triggering.value = 'backup-mediainfo';
  await f.api.open(task);
  assert.equal(f.api.loading.value, false);
  assert.equal(f.api.result.value.task_id, 'original-task');
  assert.equal(f.api.feedback.value.type, 'error');
});

for (const operation of ['close', 'dispose']) {
  test(`${operation} aborts the read and refuses a late successful result`, async t => {
    const pending = deferred();
    const f = fixture(t, { get: () => pending.promise });
    operation === 'close' ? f.api.close() : f.dispose();
    assert.equal(f.calls[0].options.signal.aborted, true);
    pending.resolve({ data: terminal() });
    await flush();
    assert.equal(f.api.result.value, null);
    assert.equal(f.api.error.value, '');
    assert.equal(f.api.loading.value, false);
  });
}

for (const [name, change] of [
  ['another user', auth => { auth.user = { id: 'admin-B' }; }],
  ['a new session of the same user', auth => { auth.user = { id: 'admin-A' }; }],
  ['a changed user ID', auth => { auth.user.id = 'admin-B'; }],
  ['logout', auth => { auth.isLoggedIn = false; }],
  ['loss of administrator access', auth => { auth.isAdmin = false; }],
]) {
  test(`${name} invalidates the original result read`, async t => {
    const pending = deferred();
    const f = fixture(t, { get: () => pending.promise });
    change(f.auth);
    assert.equal(f.calls[0].options.signal.aborted, true);
    pending.resolve({ data: terminal() });
    await flush();
    assert.equal(f.api.result.value, null);
    assert.equal(f.api.target.value, null);
    assert.equal(f.api.error.value, '');
    assert.equal(f.api.loading.value, false);
    assert.equal(f.messages.length, 0);
  });
}

test('restoration honors HTTP 207 result_status even without a warning count', async t => {
  const task = terminal('warning-task', 'completed');
  task.result.warnings = 0; task.result.warning_details = [];
  const f = fixture(t, { status: summary(task), get: async () => ({ status: 200, data: task }) });
  await flush();
  assert.equal(f.api.feedback.value.type, 'warning');
});

test('the original per-request HTTP 207 feedback contract is preserved', t => {
  const f = fixture(t, { status: { state: 'idle', is_running: false } });
  f.api.showRunResult({ status: 207, data: { message: '部分完成', warning_details: ['原响应提醒'] } });
  assert.equal(f.api.runFeedback.value.type, 'warning');
  assert.deepEqual(clone(f.api.runFeedback.value.details), ['原响应提醒']);
  assert.deepEqual(f.messages, [{ type: 'warning', message: '部分完成' }]);
});

const taskBinding = (expression, f) => vm.runInNewContext(expression, {
  backgroundTaskStatus: f.background.value, taskPaused: f.api.paused.value,
  taskStopping: f.api.stopping.value, taskPausing: f.api.pausing?.value || false,
});
for (const [name, flags, label, disabled, indeterminate] of [
  ['requested pause', { state: 'running', pause_requested: true }, '暂停中', true, true],
  ['held pause', { state: 'paused', pause_requested: true }, '已暂停', false, false],
  ['stop before pause', { state: 'paused', pause_requested: true, stop_requested: true }, '停止中', true, false],
  ['stopping', { state: 'stopping' }, '停止中', true, false],
]) {
  test(`control flags show ${name} with the correct button and held progress`, t => {
    const f = fixture(t, { status: { task_id: 'flag-task', is_running: true, progress: 42, progress_known: false, ...flags } });
    assert.equal(f.api.label.value, label);
    const pause = shellSource.match(/<VBtn\b[^>]*:prepend-icon="taskPaused[^>]*:disabled="([^"]+)"/)[1];
    assert.equal(taskBinding(pause, f), disabled);
    const bars = [...source.matchAll(/<VProgressLinear\b[^>]*:indeterminate="([^"]+)"/g)];
    assert.equal(bars.length, 1, 'task cards share one progress control');
    for (const bar of bars) assert.equal(taskBinding(bar[1], f), indeterminate);
    f.background.value.progress_known = true;
    for (const bar of bars) {
      assert.equal(taskBinding(bar[1], f), false);
      assert.equal(taskBinding(bar[0].match(/:model-value="([^"]+)"/)[1], f), 42);
    }
  });
}

test('control flags never replace a cancelled terminal label with stopping', t => {
  const task = { ...terminal('cancelled-task', 'cancelled'), stop_requested: true, pause_requested: true };
  const f = fixture(t, { status: summary(task), get: async () => ({ data: task }) });
  assert.equal(f.api.label.value, '已停止');
});

test('the result controls compile as one Vue component', () => {
  const parsed = parse(source, { filename: 'SchedulerSettingsPage.vue' });
  assert.deepEqual(parsed.errors, []);
  const script = compileScript(parsed.descriptor, { id: 'scheduler-results' });
  const template = compileTemplate({ source: parsed.descriptor.template.content, filename: 'SchedulerSettingsPage.vue',
    id: 'scheduler-results', compilerOptions: { bindingMetadata: script.bindings } });
  assert.deepEqual(template.errors, []);
});
