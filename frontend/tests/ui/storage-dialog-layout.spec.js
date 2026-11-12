import { test, expect } from './fixtures.js';

for (const width of [390, 1440]) {
  test(`${width} 115 authorization labels share rows with their controls and its hint clears the request interval`, async ({ page, api }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/settings/general?tab=directory');
    await page.getByRole('button', { name: '编辑 115 存储', exact: true }).click();
    const dialog = page.locator('.v-dialog.v-overlay--active').filter({ has: page.getByRole('button', { name: '关闭存储配置', exact: true }) });
    const priority = dialog.locator('[data-field="p115_api_priority"]');
    const interval = dialog.locator('[data-field="p115_request_interval"]');
    const hint = priority.getByText('默认优先 Cookie，失败后再尝试 OpenAPI。', { exact: true });
    const rows = await dialog.evaluate(element => {
      const bounds = node => {
        const { left, right, top, bottom } = node.getBoundingClientRect();
        return { left, right, top, bottom };
      };
      return [...element.querySelectorAll('.storage-auth-radio, .storage-auth-row')].map(row => {
        const radio = row.classList.contains('storage-auth-radio');
        const label = row.querySelector(radio ? '.v-input__control > .v-label' : '.storage-auth-label');
        const control = radio ? row.querySelector('.v-selection-control-group') : row.children[1];
        return { name: label.textContent.trim(), label: bounds(label), control: bounds(control) };
      });
    });
    expect(rows.map(row => row.name)).toEqual(['授权方式', 'OpenAPI', 'Cookie', '115接口']);
    for (const row of rows) {
      expect(Math.abs(row.label.left - rows[0].label.left), row.name).toBeLessThanOrEqual(2);
      expect(row.control.left - row.label.right, row.name).toBeGreaterThanOrEqual(8);
      expect(row.label.bottom, row.name).toBeGreaterThan(row.control.top);
      expect(row.label.top, row.name).toBeLessThan(row.control.bottom);
    }
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
    expect(Math.abs(layout.priority.x - layout.interval.x)).toBeLessThanOrEqual(2);
    expect(layout.interval.top - layout.priority.bottom).toBeGreaterThanOrEqual(15);
    expect(layout.intervalLabel.top - layout.hint.bottom).toBeGreaterThanOrEqual(4);
    await expect(priority.getByRole('radio', { name: '优先 Cookie（默认）', exact: true })).toBeChecked();
    await expect(priority.getByRole('radio', { name: '优先 OpenAPI（备用）', exact: true })).toBeVisible();
    await dialog.getByRole('radio', { name: '自定义 AppID 扫码', exact: true }).check();
    const appIdRow = dialog.locator('[data-field="p115_app_id"]');
    await expect(appIdRow.getByText('OpenAPI', { exact: true })).toBeVisible();
    await expect(appIdRow.getByRole('textbox', { name: '自定义 AppID', exact: true })).toBeVisible();
    await dialog.screenshot({ path: testInfo.outputPath(`storage-dialog-${width}.png`) });
    await dialog.getByRole('button', { name: '取消存储配置', exact: true }).click();
    expect(api.requests.filter(request => request.method === 'POST' && request.path !== '/api/config/rename_preview')).toEqual([]);
  });
}
