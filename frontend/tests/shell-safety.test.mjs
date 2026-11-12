import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { mergeNavigationConfig } from '../src/utils/navigationConfig.js';

const source = readFileSync(new URL('../src/MainLayout.vue', import.meta.url), 'utf8');
function definition(name, declaration = 'const') {
  const start = source.indexOf(declaration === 'const' ? `const ${name} = ` : `${declaration} ${name}(`);
  assert.ok(start >= 0, `${name} must be the actual page function`);
  const end = declaration === 'const' ? '\n};' : '\n}';
  return source.slice(start, source.indexOf(end, start) + end.length);
}

function taskFixture() {
  const requests = [], warnings = [], refreshes = [];
  let confirmation;
  const context = vm.createContext({
    props: { taskStatus: { task_id: 'task-A', current_action: '任务A', is_running: true } },
    taskControlBusy: { value: false }, taskStopping: { value: false },
    dialog: { warning: options => { confirmation = options; } },
    message: { info() {}, warning: text => warnings.push(text), error() {} },
    axios: { post: async (path, body) => { requests.push({ path, body }); return { data: {} }; } },
    refreshTaskStatus: async value => refreshes.push(value),
  });
  vm.runInContext(`${definition('controlTask', 'async function')}\n${definition('confirmStopTask', 'function')}\nthis.confirm = confirmStopTask;`, context);
  return { context, requests, warnings, refreshes, confirmation: () => confirmation };
}

for (const task of [{ task_id: 'task-B', is_running: true }, { task_id: 'task-A', is_running: false }]) {
  test(`stop confirmation does not control ${task.is_running ? 'a successor' : 'an ended'} task`, async () => {
    const f = taskFixture();
    f.context.confirm();
    assert.match(f.confirmation().content, /任务A/);
    f.context.props.taskStatus = task;
    await f.confirmation().onPositiveClick();
    assert.equal(f.requests.length, 0);
    assert.equal(f.warnings.length, 1);
  });
}

test('confirmed stop keeps the original task ID and refreshes status after control', async () => {
  const f = taskFixture();
  f.context.confirm();
  await f.confirmation().onPositiveClick();
  assert.equal(f.requests[0].path, '/api/trigger_stop_task');
  assert.equal(f.requests[0].body.task_id, 'task-A');
  assert.deepEqual(f.refreshes, [true]);
});

function menuFixture(read) {
  const requests = [];
  const base = [{ key: 'group-discovery', defaultLabel: '发现', children: [{ key: 'UserCenter', defaultLabel: '用户中心' }] }];
  const map = new Map([
    ['group-discovery', { ...base[0], isGroup: true }],
    ['UserCenter', { ...base[0].children[0], isGroup: false }],
  ]);
  const context = vm.createContext({
    mergeNavigationConfig,
    baseMenuOptions: { value: base }, baseMenuMap: { value: map },
    isMenuReady: { value: false }, isMenuLoading: { value: false }, menuConfigLoaded: { value: false },
    menuConfigError: { value: '' }, menuConfigTree: { value: [] }, menuRevision: { value: 0 }, isSavingMenu: { value: false },
    isMenuEditorVisible: { value: true }, nextTick: callback => callback(), console: { error() {} },
    message: { success() {}, error() {} },
    axios: { get: read, post: async (path, body) => { requests.push({ path, body }); return { data: {} }; } },
  });
  vm.runInContext(['syncMenuConfig', 'initMenuConfig', 'saveMenuConfig', 'resetMenuConfig'].map(name => definition(name)).join('\n')
    + '\nthis.load = initMenuConfig; this.save = saveMenuConfig; this.reset = resetMenuConfig;', context);
  return { context, requests };
}

test('pending menu read prevents saving fallback data; a null result confirms default settings', async () => {
  let complete;
  const read = new Promise(resolve => { complete = resolve; });
  const f = menuFixture(() => read);
  const loading = f.context.load();
  await f.context.save();
  assert.equal(f.requests.length, 0);
  complete({ data: null });
  await loading;
  await f.context.save();
  assert.equal(f.requests.length, 1);
  assert.equal(f.requests[0].body[0].children[0].key, 'UserCenter');
});

test('failed menu read leaves fallback navigation usable while blocking unconfirmed writes', async () => {
  const f = menuFixture(async () => { throw { response: { data: { error: '读取失败' } } }; });
  await f.context.load();
  assert.equal(f.context.isMenuReady.value, true);
  assert.equal(f.context.menuConfigTree.value[0].children[0].key, 'UserCenter');
  assert.equal(f.context.menuConfigError.value, '读取失败');
  await f.context.save();
  assert.equal(f.requests.length, 0);
});

test('malformed successful menu response cannot be saved as fallback defaults', async () => {
  const f = menuFixture(async () => ({ data: {} }));
  await f.context.load();
  await f.context.save();
  assert.equal(f.requests.length, 0);
  assert.match(f.context.menuConfigError.value, /格式无效/);
});

test('explicit successful reset after a read failure authorizes saving default settings', async () => {
  const f = menuFixture(async () => { throw new Error('network unavailable'); });
  await f.context.load();
  await f.context.reset();
  await f.context.save();
  assert.deepEqual(f.requests.map(request => request.path), ['/api/system/menu_config/reset', '/api/system/menu_config']);
  assert.equal(f.context.menuConfigError.value, '');
});
