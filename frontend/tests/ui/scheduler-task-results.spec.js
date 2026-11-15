import { test, expect } from './fixtures.js';

const result = {
  task_id: 'saved-task-A', state: 'failed', is_running: false, last_action: '翻译演员', current_action: '空闲',
  progress: 100, progress_known: true, message: '演员翻译失败，有图片提醒', result_status: 500, logs: [],
  result: { failed: 1, failures: [{ item_id: 'media-A', reason: '姓名译文缺失' }], warnings: 1, warning_details: ['图片暂未恢复'] },
};
const summary = ({ result: _result, ...task }) => task;
const saved = summary(result);
const setup = api => {
  api.setTask({ ...saved, history: [saved] });
  api.respond('GET', '/api/tasks/result/saved-task-A', result);
};
const resultReads = api => api.requests.filter(request => request.method === 'GET' && request.path === '/api/tasks/result/saved-task-A').length;
const historyResult = page => page.getByTestId('scheduler-task-history').locator('[data-task-id="saved-task-A"]').getByRole('button', { name: '查看结果', exact: true });

test('reentering task center opens persisted failures and warnings from the original history task', async ({ page, api }) => {
  setup(api);
  await page.goto('/settings/scheduler');
  await expect(page.getByTestId('scheduler-current-task')).toHaveCount(0);
  await expect(page.getByText('Telegram 快捷菜单', { exact: true })).toBeVisible();
  await historyResult(page).click();
  let dialog = page.getByRole('dialog');
  await expect(dialog.getByTestId('scheduler-task-result')).toContainText('失败');
  await expect(dialog.getByTestId('scheduler-task-result')).toContainText('media-A：姓名译文缺失');
  await expect(dialog.getByTestId('scheduler-task-result')).toContainText('图片暂未恢复');
  await dialog.getByRole('button', { name: '关闭任务结果', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  const before = resultReads(api);
  await page.goto('/dashboard');
  await page.goto('/settings/scheduler');
  await expect(page.getByTestId('scheduler-current-task')).toHaveCount(0);
  await historyResult(page).click();
  dialog = page.getByRole('dialog');
  await expect(dialog.getByTestId('scheduler-task-result')).toContainText('media-A：姓名译文缺失');
  await expect(dialog.getByTestId('scheduler-task-result')).toContainText('图片暂未恢复');
  await expect.poll(() => resultReads(api)).toBeGreaterThan(before);
  await dialog.getByRole('button', { name: '关闭任务结果', exact: true }).click();
  await expect(dialog).toHaveCount(0);
});

test('the latest history result remains selected while a new current task starts', async ({ page, api }) => {
  setup(api);
  await page.goto('/settings/scheduler');
  await expect.poll(() => resultReads(api)).toBeGreaterThan(0);
  const pending = api.defer('GET', '/api/tasks/result/saved-task-A');
  await historyResult(page).click();
  await pending.started;
  api.setTask({ task_id: 'new-task-B', state: 'running', is_running: true, last_action: '备份媒体信息', current_action: '读取新媒体',
    progress: 20, progress_known: true, message: '新任务处理中', history: [saved], logs: [] });
  await expect(page.getByTestId('scheduler-current-task')).toHaveCount(0);
  await expect(page.getByRole('button', { name: '查看当前任务', exact: true })).toContainText('读取新媒体');
  pending.release(result);
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('翻译演员');
  await expect(dialog.getByTestId('scheduler-task-result')).toContainText('media-A：姓名译文缺失');
  await expect(dialog.getByTestId('scheduler-task-result')).toContainText('图片暂未恢复');
});

test('a result read failure can be retried inside the same history result dialog', async ({ page, api }) => {
  setup(api);
  api.respond('GET', '/api/tasks/result/saved-task-A', { error: '任务结果暂时无法读取' }, 503);
  await page.goto('/settings/scheduler');
  await expect(page.getByTestId('scheduler-current-task')).toHaveCount(0);
  await historyResult(page).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('任务结果暂时无法读取');
  await expect(dialog.getByTestId('scheduler-task-result')).toHaveCount(0);
  api.respond('GET', '/api/tasks/result/saved-task-A', result);
  await dialog.getByRole('button', { name: '重新读取结果', exact: true }).click();
  await expect(dialog.getByTestId('scheduler-task-result')).toContainText('media-A：姓名译文缺失');
  await expect(dialog.getByTestId('scheduler-task-result')).toContainText('图片暂未恢复');
  await expect(dialog.getByText('任务结果暂时无法读取', { exact: true })).toHaveCount(0);
});

test('pause and stop flags retain known progress and hold unknown progress in the row and shared task menu', async ({ page, api }) => {
  api.respond('GET', '/api/tasks/available', [{ key: 'backup-mediainfo', name: '备份媒体信息', category_name: '媒体信息', category: 'media-info' }]);
  const paused = { task_id: 'paused-task', is_running: true, state: 'paused', pause_requested: true, last_action: '备份媒体信息',
    current_action: '备份媒体信息', progress: 42, progress_known: false, message: '暂停验证', history: [], logs: [] };
  api.setTask(paused);
  await page.goto('/settings/scheduler');
  await expect(page.getByTestId('scheduler-current-task')).toHaveCount(0);
  await page.getByRole('button', { name: '查看当前任务', exact: true }).click();
  const current = page.locator('.v-menu.v-overlay--active'), row = page.locator('[data-task-key="backup-mediainfo"]');
  const currentProgress = current.getByRole('progressbar'), rowProgress = row.getByRole('progressbar');
  await expect(current.getByRole('button', { name: '继续', exact: true })).toBeEnabled();
  await expect(row).toContainText('已暂停');
  await expect(currentProgress).toHaveCount(1);
  await expect(rowProgress).toHaveCount(1);
  await expect(currentProgress).not.toHaveClass(/v-progress-linear--indeterminate/);
  await expect(rowProgress).not.toHaveClass(/v-progress-linear--indeterminate/);
  api.setTask({ ...paused, progress_known: true });
  await expect(current).toContainText('42%');
  await expect(row).toContainText('42%');
  await expect(currentProgress).toHaveAttribute('aria-valuenow', '42');
  await expect(rowProgress).toHaveAttribute('aria-valuenow', '42');
  api.setTask({ ...paused, stop_requested: true });
  await expect(current.getByRole('button', { name: '停止中', exact: true })).toBeDisabled();
  await expect(current.getByRole('button', { name: '继续', exact: true })).toBeDisabled();
  await expect(row).toContainText('停止中');
  await expect(currentProgress).not.toHaveClass(/v-progress-linear--indeterminate/);
  await expect(rowProgress).not.toHaveClass(/v-progress-linear--indeterminate/);
  api.setTask({ ...paused, state: 'running', pause_requested: true });
  await expect(current.getByRole('button', { name: '暂停中', exact: true })).toBeDisabled();
  await expect(row).toContainText('暂停中');
});
