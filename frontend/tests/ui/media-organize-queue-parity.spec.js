import { test, expect } from './fixtures.js';

const historyPath = '/api/media_organize/history';
const queuePath = '/api/media_organize/queue';
const item = { item_id: 'queue-parity-movie', name: '队列布局验证电影', item_type: 'Movie' };
const task = (id, overrides = {}) => ({ id, item_id: item.item_id, item_json: { ...item, file_name: `${id}.mkv` },
  mode: 'reprocess', status: 'queued', message: '', progress: 0, progress_known: false, ...overrides });
const queuePage = items => ({ items, total: items.length, queued_count: items.filter(row => row.status === 'queued').length,
  running_count: items.filter(row => row.status === 'running').length });

for (const viewport of [{ width: 1280, height: 900 }, { width: 390, height: 844 }]) {
  test(`queue summary and progress match the MP structure at ${viewport.width}px`, async ({ page, api }) => {
    await page.setViewportSize(viewport);
    api.respond('GET', historyPath, { items: [], total: 0 });
    api.respond('GET', queuePath, queuePage([
      task('complete', { status: 'completed' }),
      task('running', { status: 'running', progress: 87, progress_known: false, current_action: '读取媒体' }),
    ]));
    await page.goto('/media-organize');
    await page.getByTestId('organize-queue-open').click();
    const dialog = page.getByTestId('organize-queue-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.locator('.queue-heading .v-card-subtitle')).toHaveText('本页 1 项媒体，共 2 项任务');
    const overall = dialog.getByRole('region', { name: '整体进度' });
    await expect(overall).toContainText('本页已完成 1 / 2');
    await expect(overall.locator('.queue-overall-value')).toHaveText('50%');
    await expect(overall.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '50');
    const media = dialog.getByRole('navigation', { name: '队列媒体列表' }).getByRole('button');
    await expect(media).toContainText('已完成 1 / 2');
    await expect(media.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '50');
    const running = page.getByTestId('organize-queue-task-running');
    await expect(running.locator('.queue-task-progress')).toContainText('处理中 · 读取媒体');
    await expect(running.getByRole('progressbar')).not.toHaveAttribute('aria-valuenow');
    await expect(running).not.toContainText('87%');
    await expect(running.getByRole('button', { name: /取消等待任务/ })).toBeDisabled();
    await expect(page.getByTestId('organize-queue-task-complete').locator('.queue-task-progress')).toHaveText('100%');
    const statusBox = await running.locator('.queue-task-status').boundingBox();
    const progressBox = await running.locator('.queue-task-progress').boundingBox();
    if (viewport.width >= 960) {
      expect(progressBox.x).toBeGreaterThan(statusBox.x + statusBox.width - 1);
      const dialogBox = await dialog.boundingBox();
      expect(dialogBox.width).toBeLessThanOrEqual(960);
      await expect(dialog.locator('.queue-task-header > span')).toHaveText(['', '文件名', '状态', '进度', '操作']);
    } else {
      expect(progressBox.y).toBeGreaterThan(statusBox.y + statusBox.height - 1);
      await expect(dialog.locator('.queue-task-header')).toBeHidden();
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    expect(api.requests.some(request => request.method !== 'GET' && request.path.startsWith(queuePath))).toBe(false);
  });
}

test('desktop queue lists scroll inside the dialog while media and column headers remain visible', async ({ page, api }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  api.respond('GET', historyPath, { items: [], total: 0 });
  const tasks = Array.from({ length: 40 }, (_, index) => task(`file-${String(index).padStart(2, '0')}`, {
    item_json: { ...item, file_name: `${'较长媒体文件名'.repeat(8)}.${index}.mkv` },
  }));
  api.respond('GET', queuePath, queuePage(tasks));
  await page.goto('/media-organize');
  await page.getByTestId('organize-queue-open').click();
  const dialog = page.getByTestId('organize-queue-dialog');
  await expect(dialog.locator('.queue-task-list')).toBeVisible();
  const columnBounds = await dialog.locator('.queue-task-header').boundingBox();
  const scrollState = await dialog.locator('.queue-task-list').evaluate(element => {
    const result = { height: element.clientHeight, total: element.scrollHeight, overflow: getComputedStyle(element).overflowY };
    element.scrollTop = element.scrollHeight;
    return result;
  });
  expect(scrollState.overflow).toBe('auto');
  expect(scrollState.total).toBeGreaterThan(scrollState.height);
  await expect(page.getByTestId('organize-queue-task-file-39')).toBeInViewport();
  const afterScroll = await dialog.locator('.queue-task-header').boundingBox();
  expect(afterScroll.y).toBeCloseTo(columnBounds.y, 1);
  await expect(dialog.locator('.queue-active-media')).toBeInViewport();
  expect(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await expect(page.getByTestId('organize-queue-task-file-39').locator('.queue-task-name')).toHaveAttribute('title', tasks[39].item_json.file_name);
});
