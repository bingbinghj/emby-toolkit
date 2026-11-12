import { test, expect } from './fixtures.js';

const tasks = [
  { key: 'extract-episode-credits', name: '提取片尾', category: 'media-info', category_name: '媒体信息', help: '仅处理电视剧分集，保留片头、章节图片和音轨' },
  { key: 'backup-mediainfo', name: '备份媒体信息', category: 'media-info', category_name: '媒体信息', help: '只备份缓存，不删除文件' },
];
const active = { task_id: 'full-library-credits', is_running: true, state: 'running', last_action: '提取片尾', current_action: '比较第九集声纹', message: '已处理第九集，读取第十集', progress_known: true, progress: 55, logs: [] };

test('desktop tasks use separate MP summary cards with a drag handle and primary execute button', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  api.respond('GET', '/api/tasks/available', tasks);
  await page.goto('/settings/scheduler');
  const table = page.getByTestId('scheduler-desktop-tasks');
  await expect(table).toBeVisible();
  await expect(page.getByTestId('scheduler-mobile-tasks')).toBeHidden();
  await expect(table.locator('.scheduler-category-card')).toHaveCount(1);
  await expect(table.locator('.v-card-title')).toHaveText('媒体信息');
  await expect(table.getByRole('columnheader', { name: '分类', exact: true })).toHaveCount(0);
  await expect(table.getByRole('table')).toHaveCount(0);
  await expect(table.getByRole('columnheader', { name: /下次|时间|提供者/ })).toHaveCount(0);
  await expect(table.locator('.scheduler-task-grid > .scheduler-task-card')).toHaveCount(2);
  await expect(table.locator('[data-task-key="extract-episode-credits"] .task-drag-handle')).toBeVisible();
  await expect(table.locator('[data-task-key="backup-mediainfo"] .task-drag-handle')).toBeVisible();
  const execute = table.locator('[data-task-key="extract-episode-credits"]').getByRole('button', { name: '执行', exact: true });
  await expect(execute).toHaveClass(/v-btn--size-small/);
  await expect(execute).toHaveClass(/bg-primary/);
  await expect(execute.locator('.v-btn__prepend .v-icon')).toBeVisible();
});

test('task rows retain the original task identity when its phase changes and show its actual terminal state', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  api.respond('GET', '/api/tasks/available', tasks);
  api.setTask(active);
  await page.goto('/settings/scheduler');
  const table = page.getByTestId('scheduler-desktop-tasks');
  const credits = table.locator('[data-task-key="extract-episode-credits"]');
  const backup = table.locator('[data-task-key="backup-mediainfo"]');
  await expect(credits.getByText('运行中', { exact: true })).toBeVisible();
  await expect(credits.locator('.scheduler-progress')).toContainText('55%');
  await expect(backup.getByText('待执行', { exact: true })).toBeVisible();
  await expect(backup.locator('.scheduler-progress')).toHaveCount(0);
  await expect(credits.getByRole('button', { name: '执行', exact: true })).toBeDisabled();
  api.setTask({ ...active, is_running: false, state: 'completed', current_action: '空闲', message: '已完成' });
  await expect(credits.getByText('已完成', { exact: true })).toBeVisible();
  await expect(credits.locator('.scheduler-progress')).toHaveCount(0);
  await expect(credits.getByRole('button', { name: '执行', exact: true })).toBeEnabled();
});

test('single-episode extraction does not claim whole-library task progress', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  api.respond('GET', '/api/tasks/available', tasks);
  api.setTask({ ...active, task_id: 'single-episode-credits', last_action: '提取当前集片尾' });
  await page.goto('/settings/scheduler');
  await expect(page.getByTestId('scheduler-current-task')).toContainText('提取当前集片尾');
  const row = page.getByTestId('scheduler-desktop-tasks').locator('[data-task-key="extract-episode-credits"]');
  await expect(row.getByText('待执行', { exact: true })).toBeVisible();
  await expect(row.locator('.scheduler-progress')).toHaveCount(0);
  await expect(row.getByRole('button', { name: '执行', exact: true })).toBeDisabled();
});

test('mobile tasks use MP cards and keep long names, help and rounded controls fully visible', async ({ page, api }) => {
  await page.setViewportSize({ width: 390, height: 900 });
  const name = '验证电视剧片尾提取与完整媒体信息回读';
  const help = '需要核对原章节图片、片头、片尾和音轨，说明可以多行显示，圆角不得裁切。'.repeat(3);
  api.respond('GET', '/api/tasks/available', [{ ...tasks[0], name, help }]);
  await page.goto('/settings/scheduler');
  await expect(page.getByTestId('scheduler-desktop-tasks')).toBeHidden();
  const mobile = page.getByTestId('scheduler-mobile-tasks');
  await expect(mobile).toBeVisible();
  const card = mobile.locator('.mobile-scheduler-card');
  await card.scrollIntoViewIfNeeded();
  await expect(card.locator('.temp-task-name')).toHaveText(name);
  await expect(card.locator('.temp-task-description')).toHaveText(help);
  await expect(card.getByRole('button', { name: '执行', exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const geometry = await card.locator('.temp-task-description').evaluate(element => {
    const range = document.createRange(); range.selectNodeContents(element); const text = range.getBoundingClientRect(), card = element.closest('.mobile-scheduler-card').getBoundingClientRect();
    return { textHeight: element.scrollHeight, height: element.clientHeight, left: text.left - card.left, right: card.right - text.right, bottom: card.bottom - text.bottom };
  });
  expect(geometry.textHeight).toBeLessThanOrEqual(geometry.height + 1);
  expect(geometry.left).toBeGreaterThanOrEqual(8);
  expect(geometry.right).toBeGreaterThanOrEqual(8);
  expect(geometry.bottom).toBeGreaterThanOrEqual(8);
});

const categorizedTasks = [
  ...tasks,
  { key: 'clear-mediainfo-cache', name: '清理媒体信息缓存', category: 'system-maintenance', category_name: '系统维护', help: '仅清理 ETK 缓存' },
  { key: 'uncategorized-task', name: '未分类测试任务', help: '验证其他任务分类' },
];

for (const width of [390, 1440]) {
  test(`${width}px tasks stay in separate category cards and execute the selected task`, async ({ page, api }) => {
    await page.setViewportSize({ width, height: 900 });
    api.respond('GET', '/api/tasks/available', categorizedTasks);
    api.respond('POST', '/api/tasks/run', { message: '隔离任务执行成功' });
    await page.goto('/settings/scheduler');
    const view = page.getByTestId(width < 600 ? 'scheduler-mobile-tasks' : 'scheduler-desktop-tasks');
    await expect(view.locator('.scheduler-category-card')).toHaveCount(3);
    const media = view.locator('[data-task-category="媒体信息"]');
    const system = view.locator('[data-task-category="系统维护"]');
    await expect(media.locator('.task-record')).toHaveCount(2);
    await expect(system.locator('.task-record')).toHaveCount(1);
    await expect(view.locator('[data-task-category="其他任务"] .task-record')).toHaveCount(1);
    await expect(media).not.toContainText('清理媒体信息缓存');
    await system.getByRole('button', { name: '执行', exact: true }).click();
    await expect(page.getByText('隔离任务执行成功', { exact: true })).toBeVisible();
    const runs = api.requests.filter(request => request.method === 'POST' && request.path === '/api/tasks/run');
    expect(runs.map(request => request.body)).toEqual([{ task_name: 'clear-mediainfo-cache' }]);
  });

  test(`${width}px task search removes empty category cards and restores the catalog on clear`, async ({ page, api }) => {
    await page.setViewportSize({ width, height: 900 });
    api.respond('GET', '/api/tasks/available', categorizedTasks);
    await page.goto('/settings/scheduler');
    const view = page.getByTestId(width < 600 ? 'scheduler-mobile-tasks' : 'scheduler-desktop-tasks');
    const searchTasks = async query => {
      if (width < 600) await page.getByRole('button', { name: '打开搜索任务', exact: true }).click();
      await page.getByRole('searchbox', { name: '搜索任务', exact: true }).fill(query);
      if (width < 600) await page.getByRole('button', { name: '提交搜索', exact: true }).click();
    };
    await searchTasks('媒体信息');
    await expect(view.locator('.scheduler-category-card')).toHaveCount(2);
    await expect(view.locator('.task-record')).toHaveCount(3);
    await searchTasks('仅清理');
    await expect(view.locator('.scheduler-category-card')).toHaveCount(1);
    await expect(view.locator('.scheduler-category-card .v-card-title')).toHaveText('系统维护');
    await searchTasks('无匹配的任务');
    await expect(view.locator('.scheduler-category-card')).toHaveCount(0);
    await expect(view.getByText('未找到匹配任务', { exact: true })).toBeVisible();
    await searchTasks('');
    await expect(view.locator('.scheduler-category-card')).toHaveCount(3);
    await expect(view.locator('.task-record')).toHaveCount(4);
    expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
  });
}

test('Telegram task menu uses the primary filled save control', async ({ page, api }) => {
  api.respond('GET', '/api/tasks/available', tasks);
  await page.goto('/settings/scheduler');
  await page.getByRole('button', { name: '配置 TG 菜单任务', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByText('配置 Telegram 快捷菜单', { exact: true }) });
  const save = dialog.getByRole('button', { name: '保存', exact: true });
  await expect(save).toHaveClass(/v-btn--variant-flat/);
  await expect(save).toHaveClass(/bg-primary/);
  await expect(save.locator('.v-btn__prepend .v-icon')).toBeVisible();
  const position = await save.evaluate(element => ({ left: element.getBoundingClientRect().left, parent: element.closest('.v-card-actions').getBoundingClientRect().left }));
  expect(position.left - position.parent).toBeLessThanOrEqual(24);
});

const taskKeys = group => group.locator('.scheduler-task-grid > .task-record').evaluateAll(cards => cards.map(card => card.dataset.taskKey));
async function moveCardBefore(page, group, moving, before, touch = false) {
  const handle = group.locator(`[data-task-key="${moving}"] .task-drag-handle`);
  await handle.scrollIntoViewIfNeeded();
  const start = await handle.boundingBox();
  const target = await group.locator(`[data-task-key="${before}"]`).boundingBox();
  if (touch) {
    const session = await page.context().newCDPSession(page);
    const point = { x: start.x + start.width / 2, y: start.y + start.height / 2 };
    await session.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [point] });
    await page.waitForTimeout(180);
    for (let step = 1; step <= 20; step++) {
      await session.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{
        x: point.x + (target.x + 12 - point.x) * step / 20,
        y: point.y + (target.y + 16 - point.y) * step / 20,
      }] });
    }
    await session.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
    await session.detach();
    return;
  }
  await page.mouse.move(start.x + start.width / 2, start.y + start.height / 2);
  await page.mouse.down();
  await page.mouse.move(target.x + 12, target.y + 16, { steps: 20 });
  await page.mouse.up();
}

for (const width of [390, 1440]) {
  test(`${width}px task cards keep a drag as draft and persist only their order on explicit save`, async ({ page, api }) => {
    await page.setViewportSize({ width, height: 1000 });
    api.respond('GET', '/api/tasks/available', categorizedTasks);
    api.respond('POST', '/api/tasks/run', { message: '排序后的任务执行成功' });
    await page.goto('/settings/scheduler');
    const group = page.locator('[data-task-category="媒体信息"]');
    await expect.poll(() => taskKeys(group)).toEqual(tasks.map(task => task.key));
    await moveCardBefore(page, group, tasks[1].key, tasks[0].key);
    await expect.poll(() => taskKeys(group)).toEqual([tasks[1].key, tasks[0].key]);
    await expect(page.getByText('顺序已调整，保存后生效', { exact: true })).toBeVisible();
    expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
    await page.reload();
    await expect.poll(() => taskKeys(group)).toEqual(tasks.map(task => task.key));
    await moveCardBefore(page, group, tasks[1].key, tasks[0].key);
    await expect.poll(() => taskKeys(group)).toEqual([tasks[1].key, tasks[0].key]);
    await page.getByRole('button', { name: '保存任务顺序', exact: true }).click();
    await expect(page.getByText('任务顺序已保存。', { exact: true })).toBeVisible();
    const saves = api.requests.filter(request => request.method === 'POST' && request.path === '/api/config');
    expect(saves.map(request => request.body)).toEqual([{ manual_task_order: [tasks[1].key, tasks[0].key, 'clear-mediainfo-cache', 'uncategorized-task'] }]);
    await page.reload();
    await expect.poll(() => taskKeys(group)).toEqual([tasks[1].key, tasks[0].key]);
    await group.locator(`[data-task-key="${tasks[1].key}"]`).getByRole('button', { name: '执行', exact: true }).click();
    await expect(page.getByText('排序后的任务执行成功', { exact: true })).toBeVisible();
    expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/tasks/run').map(request => request.body)).toEqual([{ task_name: tasks[1].key }]);
  });
}

test('a failed task-order save leaves the cards and draft visible for retry', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  api.respond('GET', '/api/tasks/available', tasks);
  api.respond('POST', '/api/config', { error: '模拟保存失败' }, 500);
  await page.goto('/settings/scheduler');
  const group = page.locator('[data-task-category="媒体信息"]');
  await moveCardBefore(page, group, tasks[1].key, tasks[0].key);
  await expect.poll(() => taskKeys(group)).toEqual([tasks[1].key, tasks[0].key]);
  const save = page.getByRole('button', { name: '保存任务顺序', exact: true });
  await save.click();
  await expect(page.getByText('模拟保存失败', { exact: true }).first()).toBeVisible();
  await expect(group).toBeVisible();
  await expect.poll(() => taskKeys(group)).toEqual([tasks[1].key, tasks[0].key]);
  await expect(save).toBeEnabled();
  api.respond('POST', '/api/config', request => ({ saved_fields: request.body, message: '配置已保存' }));
  await save.click();
  await expect(page.getByText('任务顺序已保存。', { exact: true })).toBeVisible();
  await expect(page.getByText('顺序已调整，保存后生效', { exact: true })).toHaveCount(0);
});

test('task status polling keeps an active drag sortable and its preview outside glass cards', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  api.respond('GET', '/api/tasks/available', tasks);
  await page.goto('/settings/scheduler');
  const group = page.locator('[data-task-category="媒体信息"]');
  await expect.poll(() => taskKeys(group)).toEqual(tasks.map(task => task.key));
  const handle = group.locator(`[data-task-key="${tasks[1].key}"] .task-drag-handle`);
  await handle.scrollIntoViewIfNeeded();
  const start = await handle.boundingBox();
  await page.mouse.move(start.x + start.width / 2, start.y + start.height / 2);
  await page.mouse.down();
  await page.mouse.move(start.x + start.width / 2 - 8, start.y + start.height / 2, { steps: 2 });
  await expect(page.locator('body > .sortable-fallback')).toBeVisible();
  api.setTask({ ...active, is_running: false, state: 'completed', last_action: tasks[1].name, current_action: '空闲', message: '拖拽期间状态已回读' });
  await expect(group.locator(`[data-task-key="${tasks[1].key}"]`).getByText('已完成', { exact: true })).toBeVisible();
  const target = await group.locator(`[data-task-key="${tasks[0].key}"]`).boundingBox();
  await page.mouse.move(target.x + 12, target.y + 16, { steps: 20 });
  await page.mouse.up();
  await expect.poll(() => taskKeys(group)).toEqual([tasks[1].key, tasks[0].key]);
  await expect(page.getByText('顺序已调整，保存后生效', { exact: true })).toBeVisible();
  expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
});

test.describe('touch task-card sorting', () => {
  test.use({ viewport: { width: 390, height: 1000 }, isMobile: true, hasTouch: true });
  test('a long press on the handle reorders tasks without executing or saving one', async ({ page, api }) => {
    api.respond('GET', '/api/tasks/available', tasks);
    await page.goto('/settings/scheduler');
    const group = page.locator('[data-task-category="媒体信息"]');
    await moveCardBefore(page, group, tasks[1].key, tasks[0].key, true);
    await expect.poll(() => taskKeys(group)).toEqual([tasks[1].key, tasks[0].key]);
    expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
    await expect(group.getByRole('button', { name: '执行', exact: true }).first()).toBeEnabled();
  });
});

test('dragging search results saves the full order with hidden tasks in their existing slots', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  const hidden = { key: 'hidden-media', name: '隐藏媒体任务', category_name: '媒体信息', help: '不参与此搜索' };
  api.respond('GET', '/api/tasks/available', [
    { ...tasks[0], help: '排序可见' }, hidden, { ...tasks[1], help: '排序可见' }, categorizedTasks[2],
  ]);
  await page.goto('/settings/scheduler');
  const search = page.getByRole('searchbox', { name: '搜索任务', exact: true });
  await search.fill('排序可见');
  const group = page.locator('[data-task-category="媒体信息"]');
  await expect.poll(() => taskKeys(group)).toEqual(tasks.map(task => task.key));
  await moveCardBefore(page, group, tasks[1].key, tasks[0].key);
  await expect.poll(() => taskKeys(group)).toEqual([tasks[1].key, tasks[0].key]);
  await page.getByRole('button', { name: '保存任务顺序', exact: true }).click();
  await expect(page.getByText('任务顺序已保存。', { exact: true })).toBeVisible();
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/config').map(request => request.body))
    .toEqual([{ manual_task_order: [tasks[1].key, hidden.key, tasks[0].key, categorizedTasks[2].key] }]);
  await search.fill('');
  await expect.poll(() => taskKeys(group)).toEqual([tasks[1].key, hidden.key, tasks[0].key]);
  await expect(page.locator('[data-task-category="系统维护"] .task-record')).toHaveCount(1);
});

test('a server response does not replace a newer drag while task-order saving is pending', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  api.respond('GET', '/api/tasks/available', tasks);
  const pending = api.defer('POST', '/api/config');
  await page.goto('/settings/scheduler');
  const group = page.locator('[data-task-category="媒体信息"]');
  await moveCardBefore(page, group, tasks[1].key, tasks[0].key);
  await expect.poll(() => taskKeys(group)).toEqual([tasks[1].key, tasks[0].key]);
  const save = page.getByRole('button', { name: '保存任务顺序', exact: true });
  await save.click();
  const request = await pending.started;
  await expect(save).toBeDisabled();
  await moveCardBefore(page, group, tasks[0].key, tasks[1].key);
  await expect.poll(() => taskKeys(group)).toEqual(tasks.map(task => task.key));
  pending.release({ saved_fields: request.body, message: '配置已保存' });
  await expect(page.getByText('任务顺序已保存。', { exact: true })).toBeVisible();
  await expect.poll(() => taskKeys(group)).toEqual(tasks.map(task => task.key));
  await expect(page.getByText('顺序已调整，保存后生效', { exact: true })).toBeVisible();
  await expect(save).toBeEnabled();
  api.respond('POST', '/api/config', request => ({ saved_fields: request.body, message: '配置已保存' }));
  await save.click();
  await expect(page.getByText('顺序已调整，保存后生效', { exact: true })).toHaveCount(0);
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/config').map(request => request.body))
    .toEqual([{ manual_task_order: [tasks[1].key, tasks[0].key] }, { manual_task_order: tasks.map(task => task.key) }]);
});
