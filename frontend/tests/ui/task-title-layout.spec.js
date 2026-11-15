import { test, expect } from './fixtures.js';

const taskNames = [
  ['repair-video-thumbnails', '修复视频预览缩略图'],
  ['restore-nfo-and-images', '还原NFO和封面'],
  ['repair-truehd-core-shift', '纠正高清音轨错位'],
];
const tasks = taskNames.map(([key, name]) => ({
  key, name, category: 'media-info', category_name: '媒体信息', help: '保留任务说明和完整操作入口。',
}));

for (const width of [1440, 1280, 1024, 390]) {
  test(`${width}px task names use one line when the card has enough space`, async ({ page, api }) => {
    await page.setViewportSize({ width, height: 1000 });
    api.respond('GET', '/api/tasks/available', tasks);
    await page.goto('/settings/scheduler');
    for (const task of tasks) {
      const card = page.locator(`[data-task-key="${task.key}"]`);
      await card.scrollIntoViewIfNeeded();
      await expect(card.locator('.temp-task-name')).toHaveText(task.name);
      const geometry = await card.locator('.temp-task-name').evaluate(element => {
        const range = document.createRange(); range.selectNodeContents(element);
        const text = range.getBoundingClientRect(), heading = element.getBoundingClientRect();
        const card = element.closest('.scheduler-task-card').getBoundingClientRect();
        const handle = element.closest('.scheduler-task-card').querySelector('.task-drag-handle').getBoundingClientRect();
        const icon = element.closest('.scheduler-task-card').querySelector('.scheduler-task-summary .v-icon').getBoundingClientRect();
        return { textHeight: text.height, lineHeight: parseFloat(getComputedStyle(element).lineHeight),
          left: text.left - card.left, right: card.right - text.right,
          headingHeight: heading.height, overlapsHandle: text.right > handle.left && text.top < handle.bottom && text.bottom > handle.top,
          overlapsIcon: text.right > icon.left && text.bottom > icon.top && text.top < icon.bottom };
      });
      expect(geometry.textHeight).toBeLessThanOrEqual(geometry.lineHeight + 1);
      expect(geometry.left).toBeGreaterThanOrEqual(8);
      expect(geometry.right).toBeGreaterThanOrEqual(8);
      expect(geometry.overlapsHandle).toBe(false);
      expect(geometry.overlapsIcon).toBe(false);
      await expect(card.getByRole('button', { name: '执行', exact: true })).toBeVisible();
    }
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
  });
}

test('narrow cards wrap long task names without clipping or overlapping their controls', async ({ page, api }) => {
  await page.setViewportSize({ width: 320, height: 1000 });
  const name = '验证电视剧片尾提取与完整媒体信息回读和所有附属文件';
  api.respond('GET', '/api/tasks/available', [{ ...tasks[0], name }]);
  await page.goto('/settings/scheduler');
  const card = page.locator(`[data-task-key="${tasks[0].key}"]`);
  await card.scrollIntoViewIfNeeded();
  await expect(card.locator('.temp-task-name')).toHaveText(name);
  const geometry = await card.locator('.temp-task-name').evaluate(element => {
    const range = document.createRange(); range.selectNodeContents(element); const text = range.getBoundingClientRect();
    const card = element.closest('.scheduler-task-card').getBoundingClientRect();
    const icon = element.closest('.scheduler-task-card').querySelector('.scheduler-task-summary .v-icon').getBoundingClientRect();
    return { textHeight: element.scrollHeight, height: element.clientHeight, left: text.left - card.left,
      right: card.right - text.right, bottom: card.bottom - text.bottom, iconGap: icon.top - text.bottom };
  });
  expect(geometry.textHeight).toBeLessThanOrEqual(geometry.height + 1);
  expect(geometry.left).toBeGreaterThanOrEqual(8);
  expect(geometry.right).toBeGreaterThanOrEqual(8);
  expect(geometry.bottom).toBeGreaterThanOrEqual(8);
  expect(geometry.iconGap).toBeGreaterThanOrEqual(0);
  await expect(card.getByRole('button', { name: '执行', exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});
