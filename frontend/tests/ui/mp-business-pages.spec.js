import { test, expect } from './fixtures.js';

const pages = [
  ['/review', '.review-page'],
  ['/media-organize', '.media-organize-page'],
  ['/settings/scheduler', '.scheduler-page'],
  ['/settings/cover-generator', '.cover-page'],
];

for (const width of [390, 1440]) {
  test(`${width}px business pages omit duplicate page headings and preserve responsive readable content`, async ({ page, api }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.addInitScript(() => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({
      theme: 'dark', glassEnabled: true, glassQuality: 'css', glassAppearance: 'clear', layout: 'vertical',
    })));
    api.respond('GET', '/api/tasks/available', [{
      key: 'clear-mediainfo-cache', name: '清理媒体信息缓存', category: 'system-maintenance', category_name: '系统维护',
      description: '只清理 ETK 的媒体信息缓存',
      help: '验证长任务说明在移动屏幕中完整换行，执行按钮位于说明之外，媒体库和文件的实际数据不会在此次浏览器验证中修改。',
    }]);
    for (const [path, title] of pages) {
      await page.goto(path);
      const content = page.locator(title);
      await expect(content).toBeVisible();
      const firstCard = content.locator('.v-card').first();
      await expect(firstCard).toBeVisible();
      await expect.poll(async () => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      const headingBounds = await firstCard.boundingBox();
      const navbarBounds = await page.locator('.layout-navbar').boundingBox();
      expect(headingBounds.y).toBeGreaterThanOrEqual(navbarBounds.y + navbarBounds.height);
      await expect(content.locator('h1:not(.sr-only)')).toHaveCount(0);
      if (path === '/settings/scheduler') {
        const copy = page.locator('.temp-task-description:visible').first();
        await expect(copy).toContainText('完整换行');
        const button = page.locator('.task-record:visible').first().getByRole('button', { name: '执行' });
        const [copyBounds, buttonBounds] = await Promise.all([copy.boundingBox(), button.boundingBox()]);
        const overlap = Math.max(0, Math.min(copyBounds.x + copyBounds.width, buttonBounds.x + buttonBounds.width) - Math.max(copyBounds.x, buttonBounds.x))
          * Math.max(0, Math.min(copyBounds.y + copyBounds.height, buttonBounds.y + buttonBounds.height) - Math.max(copyBounds.y, buttonBounds.y));
        expect(overlap).toBe(0);
        await expect(page.getByTestId(width < 600 ? 'scheduler-mobile-tasks' : 'scheduler-desktop-tasks')).toBeVisible();
      }
      if (path === '/settings/cover-generator') {
        await page.getByRole('tab', { name: '单图风格设置', exact: true }).click();
        await expect(page.getByText('可填写字体下载链接，或上传字体到服务器后填写本地路径；本地路径优先于下载链接。', { exact: true })).toBeVisible();
      }
    }
    expect(api.requests.some(request => ['POST', 'PATCH'].includes(request.method) && request.path !== '/api/config/cover_generator/preview')).toBe(false);
  });
}
