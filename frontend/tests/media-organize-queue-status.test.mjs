import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../src/components/MediaOrganizeQueueDialog.vue', import.meta.url), 'utf8');
const start = source.indexOf('const executionState = ');
const end = source.indexOf('\nconst taskTitle = ', start);
const context = vm.createContext({});
vm.runInContext(source.slice(start, end) + '\nthis.presentation = statusPresentation; this.held = progressHeld;', context);

test('queue status follows the original job state while terminal queue results retain precedence', () => {
  for (const [state, title] of [['paused', '已暂停'], ['pausing', '暂停中'], ['stopping', '停止中']]) {
    const task = { status: 'running', job_state: state, progress: 37 };
    assert.equal(context.presentation(task).title, title);
    assert.equal(context.held(task), true);
    assert.equal(task.progress, 37);
  }
  assert.equal(context.presentation({ status: 'running', pause_requested: true }).title, '暂停中');
  assert.equal(context.presentation({ status: 'running', pause_requested: true, stop_requested: true }).title, '停止中');
  assert.equal(context.presentation({ status: 'completed', job_state: 'paused', pause_requested: true }).title, '已完成');
  assert.equal(context.presentation({ status: 'running', job_state: 'running' }).title, '正在处理');
  assert.equal(context.held({ status: 'running', job_state: 'running' }), false);
});
