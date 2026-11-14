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

test('a hung initial menu read exposes its loading state, times out and can be retried', async ({ page, api }) => {
  let release;
  api.respond('GET', '/api/system/menu_config', () => new Promise(resolve => { release = resolve; }));
  await page.goto('/user-management');
  await expect(page.getByRole('button', { name: '正在读取导航', exact: true })).toBeVisible();
  await expect(page.locator('.v-navigation-drawer').getByText('仪表盘', { exact: true })).toHaveCount(0);
  const retry = page.getByRole('button', { name: '重试读取导航', exact: true });
  await expect(retry).toBeVisible({ timeout: 12000 });
  await expect(page.locator('.v-navigation-drawer').getByText('仪表盘', { exact: true })).toBeVisible();
  const editor = await openMenuEditor(page);
  await expect(editor.getByRole('button', { name: '保存', exact: true })).toBeDisabled();
  api.respond('GET', '/api/system/menu_config', null);
  await editor.getByRole('button', { name: '重试读取', exact: true }).click();
  await expect(editor.getByRole('button', { name: '保存', exact: true })).toBeEnabled();
  await expect(retry).toHaveCount(0);
  release(null);
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/system/menu_config')).toEqual([]);
});

for (const progressKnown of [false, true]) test(`paused topbar preserves ${progressKnown ? 'known' : 'unknown'} progress without animation and resumes the same task`, async ({ page, api }) => {
  const paused = { ...runningTask, state: 'paused', pause_requested: true,
    progress_known: progressKnown, phase: '仍在执行的旧阶段' };
  api.setTask(paused);
  api.respond('POST', '/api/tasks/resume', request => {
    api.setTask(runningTask);
    return { message: '任务已继续' };
  });
  await page.goto('/user-management');
  const current = page.getByRole('button', { name: '查看当前任务', exact: true });
  await expect(current).toContainText('任务A · 已暂停');
  await expect(current.locator('.v-progress-linear')).not.toHaveClass(/v-progress-linear--indeterminate/);
  if (progressKnown) await expect(current.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '20');
  await current.click();
  const menu = page.locator('.v-menu.v-overlay--active');
  await expect(menu.locator('.v-card-subtitle')).toHaveText('已暂停');
  const progress = menu.getByRole('progressbar');
  await expect(progress).toBeVisible();
  await expect(progress).not.toHaveClass(/v-progress-linear--indeterminate/);
  if (progressKnown) await expect(progress).toHaveAttribute('aria-valuenow', '20');
  const resume = menu.getByRole('button', { name: '继续', exact: true });
  await expect(resume).toBeEnabled();
  await resume.click();
  await expect(current).not.toContainText('已暂停');
  expect(api.requests.find(request => request.path === '/api/tasks/resume').body).toEqual({ task_id: 'task-A' });
});

test('topbar presents pausing and stopping intent before the next state snapshot', async ({ page, api }) => {
  api.setTask({ ...runningTask, state: 'running', pause_requested: true, progress_known: false });
  await page.goto('/user-management');
  const current = page.getByRole('button', { name: '查看当前任务', exact: true });
  await expect(current).toContainText('暂停中');
  await current.click();
  const menu = page.locator('.v-menu.v-overlay--active');
  await expect(menu.getByRole('button', { name: '暂停中', exact: true })).toBeDisabled();
  api.setTask({ ...runningTask, state: 'running', pause_requested: true, stop_requested: true, progress_known: false });
  await expect(current).toContainText('停止中');
  await expect(menu.locator('.v-card-subtitle')).toHaveText('停止中');
  await expect(menu.getByRole('button', { name: '停止中', exact: true })).toBeDisabled();
  await expect(current.locator('.v-progress-linear')).not.toHaveClass(/v-progress-linear--indeterminate/);
});

test('failed initial task-status read is visible and can be retried without submitting a task', async ({ page, api }) => {
  api.respond('GET', '/api/status', { error: '状态连接暂时不可用' }, 503);
  await page.goto('/user-management');
  const retry = page.getByRole('button', { name: '重试读取任务状态', exact: true });
  await expect(retry).toBeVisible();
  await expect(retry).toHaveAttribute('title', /尚未成功读取/);
  await expect(page.getByRole('button', { name: '查看当前任务', exact: true })).toHaveCount(0);
  api.respond('GET', '/api/status', runningTask);
  await retry.click();
  await expect(retry).toHaveCount(0);
  await expect(page.getByRole('button', { name: '查看当前任务', exact: true })).toContainText('任务A');
  expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
});

for (const width of [390, 1440]) test(`${width}px lost task-status connection marks the retained snapshot and permits a fresh retry`, async ({ page, api }) => {
  await page.setViewportSize({ width, height: 900 });
  api.setTask(runningTask);
  await page.goto('/user-management');
  const current = page.getByRole('button', { name: '查看当前任务', exact: true });
  await expect(current).toContainText('任务A');
  api.respond('GET', '/api/status', { error: '状态连接暂时不可用' }, 503);
  const retry = page.getByRole('button', { name: '重试读取任务状态', exact: true });
  await expect(retry).toBeVisible();
  await expect(retry).toHaveAttribute('title', /最后确认/);
  await expect(current).toContainText('任务A');
  await expect(current.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '20');
  const buttonBounds = await retry.boundingBox(), taskBounds = await current.boundingBox();
  expect(buttonBounds.x + buttonBounds.width).toBeLessThanOrEqual(taskBounds.x);
  api.respond('GET', '/api/status', { ...runningTask, task_id: 'task-B', current_action: '任务B' });
  await retry.click();
  await expect(retry).toHaveCount(0);
  await expect(current).toContainText('任务B');
  expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
});
