import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../src/components/settings/SchedulerSettingsPage.vue', import.meta.url), 'utf8');
const tasks = [
  { key: 'extract-episode-credits', name: '提取片尾', category_name: '媒体信息', help: '匹配同季尾部声纹' },
  { key: 'backup-mediainfo', name: '备份媒体信息', category_name: '媒体信息', help: '只备份缓存' },
];
const definition = name => {
  const start = source.indexOf(`const ${name} = `);
  assert.ok(start >= 0);
  const lineEnd = source.indexOf('\n', start);
  if (source.slice(start, lineEnd).endsWith(';')) return source.slice(start, lineEnd);
  return source.slice(start, source.indexOf('\n};', start) + 4);
};
function fixture(status = {}) {
  const context = vm.createContext({
    computed: fn => ({ get value() { return fn(); } }),
    backgroundTaskStatus: { value: status }, isTriggeringTask: { value: null },
    availableTasksForManualRun: { value: tasks }, taskSearchQuery: { value: '' },
  });
  const statusStart = source.indexOf('const currentTaskStateLabel = computed(');
  const statusEnd = source.indexOf('));', statusStart) + 3;
  const filterStart = source.indexOf('const filteredManualTasks = computed(');
  const filterEnd = source.indexOf('});', filterStart) + 3;
  vm.runInContext(source.slice(statusStart, statusEnd) + '\n' + source.slice(filterStart, filterEnd)
    + '\n' + ['isCurrentTask', 'getTaskRowState', 'getTaskRowColor'].map(definition).join('\n')
    + '\nthis.current = isCurrentTask; this.state = getTaskRowState; this.color = getTaskRowColor; this.tasks = filteredManualTasks;', context);
  return context;
}

test('running row keeps its original task identity while current_action changes to a processing phase', () => {
  const f = fixture({ task_id: 'run-A', last_action: '提取片尾', current_action: '比较第九集声纹', state: 'running', is_running: true });
  assert.equal(f.current(tasks[0]), true);
  assert.equal(f.state(tasks[0]), '运行中');
  assert.equal(f.color(tasks[0]), 'success');
  assert.equal(f.current(tasks[1]), false);
  assert.equal(f.state(tasks[1]), '待执行');
});

for (const [state, label] of [['paused', '已暂停'], ['completed', '已完成'], ['completed_with_warnings', '已完成，有提醒'], ['failed', '失败']]) {
  test(`the latest matching task displays its real ${state} state`, () => {
    const f = fixture({ task_id: 'run-A', last_action: '提取片尾', current_action: '空闲', state });
    assert.equal(f.state(tasks[0]), label);
    assert.equal(f.state(tasks[1]), '待执行');
  });
}

test('a current phase that happens to contain another task name never assigns progress to that task', () => {
  const f = fixture({ task_id: 'run-A', last_action: '备份媒体信息', current_action: '完成提取片尾缓存', state: 'running' });
  assert.equal(f.current(tasks[0]), false);
  assert.equal(f.state(tasks[0]), '待执行');
});

test('single-episode extraction stays separate from the whole-library extraction row', () => {
  const f = fixture({ task_id: 'single-A', last_action: '提取当前集片尾', current_action: '提取片尾', state: 'running' });
  assert.equal(f.current(tasks[0]), false);
  assert.equal(f.state(tasks[0]), '待执行');
});

test('task submission is shown only for the task this page actually submitted', () => {
  const f = fixture();
  f.isTriggeringTask.value = 'extract-episode-credits';
  assert.equal(f.state(tasks[0]), '提交中');
  assert.equal(f.state(tasks[1]), '待执行');
  assert.equal(f.current(tasks[0]), false);
});

test('task filtering is shared by desktop rows and mobile cards without mutating the catalog', () => {
  const f = fixture();
  for (const [query, names] of [[' 声纹 ', ['提取片尾']], ['BACKUP-MEDIAINFO', ['备份媒体信息']], ['媒体信息', tasks.map(task => task.name)], ['不存在', []], ['', tasks.map(task => task.name)]]) {
    f.taskSearchQuery.value = query;
    assert.deepEqual(Array.from(f.tasks.value, task => task.name), names);
  }
  assert.equal(f.availableTasksForManualRun.value.length, 2);
});


test('rerunning the same completed task displays submitting instead of its previous terminal result', () => {
  const f = fixture({ task_id: 'previous-run', last_action: '提取片尾', current_action: '空闲', state: 'failed', is_running: false });
  f.isTriggeringTask.value = 'extract-episode-credits';
  assert.equal(f.state(tasks[0]), '提交中');
  assert.equal(f.color(tasks[0]), '');
  f.backgroundTaskStatus.value = { task_id: 'new-run', last_action: '提取片尾', current_action: '提取第九集', state: 'running', is_running: true };
  assert.equal(f.state(tasks[0]), '运行中');
  assert.equal(f.color(tasks[0]), 'success');
});
