import { test, expect } from './fixtures.js';

const runningTask = {
  task_id: 'task-A', is_running: true, state: 'running', current_action: '任务A',
  progress: 20, progress_known: true, message: '处理中', history: [], logs: [],
};

async function openStopConfirmation(page) {
  await page.getByRole('button', { name: '查看当前任务', exact: true }).click();
  await page.getByRole('button', { name: '停止', exact: true }).click();
  await expect(page.getByText(/确定停止“任务A”吗/)).toBeVisible();
}

for (const ended of [false, true]) {
  test(`topbar stop confirmation ignores a task that ${ended ? 'ended' : 'was replaced'}`, async ({ page, api }) => {
    api.setTask(runningTask);
    api.respond('POST', '/api/trigger_stop_task', { message: '停止请求已发送' });
    await page.goto('/user-management');
    await openStopConfirmation(page);
    api.setTask(ended
      ? { is_running: false, state: 'idle', message: '等待任务', history: [], logs: [] }
      : { ...runningTask, task_id: 'task-B', current_action: '任务B' });
    const current = page.getByRole('button', { name: '查看当前任务', exact: true });
    if (ended) await expect(current).toHaveCount(0);
    else await expect(current).toContainText('任务B');
    await expect(page.getByText(/确定停止“任务A”吗/)).toBeVisible();
    await page.getByRole('button', { name: '停止任务', exact: true }).click();
    await expect(page.getByText('当前任务已变化，未执行停止操作。', { exact: true })).toBeVisible();
    expect(api.requests.filter(request => request.path === '/api/trigger_stop_task')).toEqual([]);
  });
}

test('topbar confirmed stop submits only the task named in its confirmation', async ({ page, api }) => {
  api.setTask(runningTask);
  api.respond('POST', '/api/trigger_stop_task', { message: '停止请求已发送' });
  await page.goto('/user-management');
  await openStopConfirmation(page);
  await page.getByRole('button', { name: '停止任务', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.path === '/api/trigger_stop_task')).toBe(true);
  expect(api.requests.find(request => request.path === '/api/trigger_stop_task').body).toEqual({ task_id: 'task-A' });
});

async function openMenuEditor(page) {
  await page.getByRole('button', { name: 'UI Test 的用户菜单', exact: true }).click();
  await page.getByText('自定义导航', { exact: true }).click();
  return page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存', exact: true }) });
}

test('failed menu read preserves fallback navigation and blocks save until a successful null read', async ({ page, api }) => {
  api.respond('GET', '/api/system/menu_config', { error: '菜单配置暂时无法读取' }, 503);
  await page.goto('/user-management');
  await page.locator('.v-navigation-drawer').getByText('仪表盘', { exact: true }).click();
  await expect(page).toHaveURL(/\/dashboard$/);
  const editor = await openMenuEditor(page);
  const save = editor.getByRole('button', { name: '保存', exact: true });
  await expect(editor.getByText('菜单配置暂时无法读取', { exact: true })).toBeVisible();
  await expect(save).toBeDisabled();
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/system/menu_config')).toEqual([]);
  api.respond('GET', '/api/system/menu_config', null);
  await editor.getByRole('button', { name: '重试读取', exact: true }).click();
  await expect(save).toBeEnabled();
  await save.click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/system/menu_config')).toBe(true);
  const saved = api.requests.find(request => request.method === 'POST' && request.path === '/api/system/menu_config').body;
  expect(saved.map(group => group.label)).toEqual(['发现', '整理', '系统']);
});

test('explicit reset is allowed after a failed menu read and unlocks saving the confirmed defaults', async ({ page, api }) => {
  api.respond('GET', '/api/system/menu_config', { error: '菜单配置暂时无法读取' }, 503);
  await page.goto('/user-management');
  const editor = await openMenuEditor(page);
  await expect(editor.getByRole('button', { name: '保存', exact: true })).toBeDisabled();
  await editor.getByRole('button', { name: '恢复默认', exact: true }).click();
  const confirmation = page.locator('.v-overlay--active').filter({ has: page.getByText('确定恢复菜单名称、顺序及显示状态吗？', { exact: true }) });
  await confirmation.getByRole('button', { name: '恢复默认', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/system/menu_config/reset')).toBe(true);
  const save = editor.getByRole('button', { name: '保存', exact: true });
  await expect(save).toBeEnabled();
  await expect(editor.getByRole('button', { name: '重试读取', exact: true })).toHaveCount(0);
  await save.click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/system/menu_config')).toBe(true);
});
