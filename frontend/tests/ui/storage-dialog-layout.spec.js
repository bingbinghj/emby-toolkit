import { test, expect } from './fixtures.js';

for (const width of [390, 1440]) {
  test(`${width} 115 interface labels align and its Cookie hint clears the request interval`, async ({ page, api }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/settings/general?tab=directory');
    await page.getByRole('button', { name: '编辑 115 存储', exact: true }).click();
    const dialog = page.locator('.v-dialog.v-overlay--active').filter({ has: page.getByRole('button', { name: '关闭存储配置', exact: true }) });
    const priority = dialog.locator('[data-field="p115_api_priority"]');
    const interval = dialog.locator('[data-field="p115_request_interval"]');
    const hint = priority.getByText('默认优先 Cookie，失败后再尝试 OpenAPI。', { exact: true });
    await interval.scrollIntoViewIfNeeded();
    await expect(hint).toBeVisible();
    const layout = await dialog.evaluate(element => {
      const priority = element.querySelector('[data-field="p115_api_priority"]');
      const interval = element.querySelector('[data-field="p115_request_interval"]');
      const bounds = node => {
        const { x, top, bottom, width } = node.getBoundingClientRect();
        return { x, top, bottom, width };
      };
      const priorityLabel = priority.querySelector('.app-responsive-input__label') || priority.querySelector('.v-input__control > .v-label');
      const intervalLabel = interval.querySelector('.app-responsive-input__label') || interval.querySelector('.v-field-label--floating');
      const hint = priority.querySelector('.app-responsive-input__hint') || priority.querySelector('.v-messages__message');
      return { priority: bounds(priority), interval: bounds(interval), priorityLabel: bounds(priorityLabel), intervalLabel: bounds(intervalLabel), hint: bounds(hint) };
    });
    expect(Math.abs(layout.priorityLabel.x - layout.intervalLabel.x)).toBeLessThanOrEqual(2);
    expect(layout.interval.top - layout.priority.bottom).toBeGreaterThanOrEqual(15);
    expect(layout.intervalLabel.top - layout.hint.bottom).toBeGreaterThanOrEqual(4);
    await expect(priority.getByRole('radio', { name: '优先 Cookie（默认）', exact: true })).toBeChecked();
    await expect(priority.getByRole('radio', { name: '优先 OpenAPI（备用）', exact: true })).toBeVisible();
    await dialog.screenshot({ path: testInfo.outputPath(`storage-dialog-${width}.png`) });
    await dialog.getByRole('button', { name: '取消存储配置', exact: true }).click();
    expect(api.requests.filter(request => request.method === 'POST' && request.path !== '/api/config/rename_preview')).toEqual([]);
  });
}
