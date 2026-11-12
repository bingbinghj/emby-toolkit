import { test, expect } from './fixtures.js';

const running = { task_id: 'credits-A', is_running: true, state: 'running', current_action: '提取电视剧片尾', last_action: '提取电视剧片尾', progress: 42, progress_known: true, message: '已处理第9集，正在读取第10集', logs: [] };
const controls = api => api.requests.filter(request => request.method === 'POST' && ['/api/tasks/pause', '/api/tasks/resume', '/api/trigger_stop_task'].includes(request.path));

test('task center provides real pause, resume and safe stop controls for the current task', async ({ page, api }) => {
  api.setTask(running);
  api.respond('POST', '/api/tasks/pause', () => { api.setTask({ ...running, state: 'paused', message: '已在安全停止点暂停' }); return { message: '已暂停任务' }; });
  api.respond('POST', '/api/tasks/resume', () => { api.setTask(running); return { message: '任务已继续' }; });
  api.respond('POST', '/api/trigger_stop_task', () => { api.setTask({ ...running, state: 'stopping', stop_requested: true }); return { message: '停止请求已发送' }; });
  await page.goto('/settings/scheduler');
  const card = page.getByTestId('scheduler-current-task');
  await expect(card).toContainText('提取电视剧片尾');
  await expect(card).toContainText('42%');
  await card.getByRole('button', { name: '暂停', exact: true }).click();
  await expect(card.getByText('已暂停', { exact: true })).toBeVisible();
  await card.getByRole('button', { name: '继续', exact: true }).click();
  await expect(card.getByText('运行中', { exact: true })).toBeVisible();
  await card.getByRole('button', { name: '停止', exact: true }).click();
  await expect(page.getByText(/确定停止“提取电视剧片尾”吗/)).toBeVisible();
  await page.getByRole('button', { name: '取消', exact: true }).click();
  expect(controls(api).map(request => request.path)).toEqual(['/api/tasks/pause', '/api/tasks/resume']);
  await card.getByRole('button', { name: '停止', exact: true }).click();
  await page.getByRole('button', { name: '停止任务', exact: true }).click();
  await expect(card.getByRole('button', { name: '停止中', exact: true })).toBeDisabled();
  expect(controls(api).map(request => ({ path: request.path, body: request.body }))).toEqual([
    { path: '/api/tasks/pause', body: { task_id: 'credits-A' } },
    { path: '/api/tasks/resume', body: { task_id: 'credits-A' } },
    { path: '/api/trigger_stop_task', body: { task_id: 'credits-A' } },
  ]);
});

for (const ended of [false, true]) {
  test(`task-center stop confirmation protects a task that ${ended ? 'ended' : 'was replaced'}`, async ({ page, api }) => {
    api.setTask(running);
    api.respond('POST', '/api/trigger_stop_task', { message: '停止请求已发送' });
    await page.goto('/settings/scheduler');
    const card = page.getByTestId('scheduler-current-task');
    await card.getByRole('button', { name: '停止', exact: true }).click();
    api.setTask(ended ? { ...running, is_running: false, state: 'completed', message: '已完成' }
      : { ...running, task_id: 'credits-B', current_action: '新任务', last_action: '新任务' });
    if (ended) await expect(card.getByRole('button', { name: '停止', exact: true })).toHaveCount(0);
    else await expect(card).toContainText('新任务');
    await page.getByRole('button', { name: '停止任务', exact: true }).click();
    await expect(page.getByText('当前任务已变化，未执行停止操作。', { exact: true })).toBeVisible();
    expect(controls(api)).toEqual([]);
  });
}

test('failed task control exposes the real backend error and keeps controls available', async ({ page, api }) => {
  api.setTask(running);
  api.respond('POST', '/api/tasks/pause', { error: '任务尚未到达可暂停阶段' }, 409);
  await page.goto('/settings/scheduler');
  const pause = page.getByTestId('scheduler-current-task').getByRole('button', { name: '暂停', exact: true });
  await pause.click();
  await expect(page.getByText('任务尚未到达可暂停阶段', { exact: true })).toBeVisible();
  await expect(pause).toBeEnabled();
  expect(controls(api)[0].body).toEqual({ task_id: 'credits-A' });
});


test('configuration read failure does not hide controls for the actual running task', async ({ page, api }) => {
  api.setTask(running);
  api.respond('GET', '/api/config', { error: '配置读取暂时失败' }, 503);
  api.respond('POST', '/api/tasks/pause', () => { api.setTask({ ...running, state: 'paused' }); return { message: '已暂停任务' }; });
  await page.goto('/settings/scheduler');
  await expect(page.getByText('配置读取暂时失败', { exact: true }).first()).toBeVisible();
  const card = page.getByTestId('scheduler-current-task');
  await expect(card).toBeVisible();
  await card.getByRole('button', { name: '暂停', exact: true }).click();
  await expect(card.getByText('已暂停', { exact: true })).toBeVisible();
  expect(controls(api)[0].body).toEqual({ task_id: 'credits-A' });
});
