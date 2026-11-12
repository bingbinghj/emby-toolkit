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
  await expect(table.getByRole('columnheader', { name: '分类', exact: true })).toBeVisible();
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

test('Telegram task menu uses the icon and text save control', async ({ page, api }) => {
  api.respond('GET', '/api/tasks/available', tasks);
  await page.goto('/settings/scheduler');
  await page.getByRole('button', { name: '配置 TG 菜单任务', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByText('配置 Telegram 快捷菜单', { exact: true }) });
  const save = dialog.getByRole('button', { name: '保存', exact: true });
  await expect(save).toHaveClass(/v-btn--variant-text/);
  await expect(save.locator('.v-btn__prepend .v-icon')).toBeVisible();
});
