import { test, expect } from './fixtures.js';

test('media save retries a temporary result-read failure without submitting the save twice', async ({ page, api }) => {
  const savePath = '/api/media_info/edit/101';
  const taskId = 'media-save-after-temporary-read-failure';
  const resultPath = `/api/tasks/result/${taskId}`;
  const result = { message: '媒体信息回读验证完成', metadata_refresh: { state: 'verified' }, warnings: 0 };
  const task = { task_id: taskId, is_running: false, state: 'completed',
    current_action: '空闲', last_action: '保存媒体信息', message: '任务已完成',
    progress: 100, progress_known: true, result_status: 200, result };

  api.respond('POST', savePath, () => {
    api.setTask({ ...task, is_running: true, state: 'running', progress: 25,
      current_action: '保存媒体信息', message: '正在保存', result: null });
    return { task_id: taskId, message: '保存已提交' };
  }, 202);
  api.respond('GET', resultPath, () => {
    api.respond('GET', resultPath, () => { api.setTask(task); return task; });
    return { error: '模拟临时任务读取失败' };
  }, 503);

  await page.goto('/edit-media/101');
  await page.getByTestId('media-info-open').click();
  const editor = page.locator('.v-overlay--active').filter({ has: page.getByTestId('media-info-save') });
  await expect(editor.getByTestId('media-info-save')).toBeEnabled();
  await editor.getByTestId('media-info-save').click();

  await expect(page.getByText(result.message, { exact: true })).toBeVisible();
  await expect(editor).not.toBeVisible();
  expect(api.requests.filter(request => request.method === 'POST' && request.path === savePath)).toHaveLength(1);
  expect(api.requests.filter(request => request.method === 'GET' && request.path === resultPath)).toHaveLength(2);
});
