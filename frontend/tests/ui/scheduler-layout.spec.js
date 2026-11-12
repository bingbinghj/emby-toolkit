import { test, expect } from './fixtures.js';

const tasks = [
  { key: 'extract-episode-credits', name: '提取片尾', category: 'media-info', category_name: '媒体信息', help: '仅处理电视剧分集，保留片头、章节图片和音轨' },
  { key: 'backup-mediainfo', name: '备份媒体信息', category: 'media-info', category_name: '媒体信息', help: '只备份缓存，不删除文件' },
];
const active = { task_id: 'full-library-credits', is_running: true, state: 'running', last_action: '提取片尾', current_action: '比较第九集声纹', message: '已处理第九集，读取第十集', progress_known: true, progress: 55, logs: [] };

test('desktop tasks use the MP table and the small primary execute button without fabricated schedules', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  api.respond('GET', '/api/tasks/available', tasks);
  await page.goto('/settings/scheduler');
  const table = page.getByTestId('scheduler-desktop-tasks');
  await expect(table).toBeVisible();
  await expect(page.getByTestId('scheduler-mobile-tasks')).toBeHidden();
  await expect(table.locator('.scheduler-category-card')).toHaveCount(1);
  await expect(table.locator('.v-card-title')).toHaveText('媒体信息');
  await expect(table.getByRole('columnheader', { name: '分类', exact: true })).toHaveCount(0);
  await expect(table.getByRole('columnheader', { name: '任务名称', exact: true })).toBeVisible();
  await expect(table.getByRole('columnheader', { name: '任务状态', exact: true })).toBeVisible();
  await expect(table.getByRole('columnheader', { name: /下次|时间|提供者/ })).toHaveCount(0);
  await expect(table.locator('tbody tr')).toHaveCount(2);
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
});
