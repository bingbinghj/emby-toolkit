import { test, expect } from './fixtures.js';

for (const width of [390, 1440]) {
  test(`${width}px notification containers stay outside the document flow`, async ({ page, api }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto('/dashboard');
    await expect(page.locator('.dashboard-grid')).toBeVisible();
    const containers = page.locator('.Vue-Toastification__container');
    await expect(containers).toHaveCount(6);
    for (const container of await containers.all()) await expect(container).toHaveCSS('position', 'fixed');
    const bounds = await page.evaluate(() => ({
      app: document.querySelector('#app').getBoundingClientRect().bottom + scrollY,
      document: document.documentElement.scrollHeight,
    }));
    expect(bounds.document - bounds.app).toBeLessThanOrEqual(2);

    api.respond('GET', '/api/dashboard/media', { error: '模拟通知定位检查' }, 503);
    await page.reload();
    await expect(page.locator('.Vue-Toastification__container')).toHaveCount(6);
    expect(await containers.first().evaluate(element => getComputedStyle(element).position)).toBe('fixed');
  });
}
