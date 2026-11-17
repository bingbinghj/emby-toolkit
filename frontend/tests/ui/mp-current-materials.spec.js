import { test, expect } from './fixtures.js';

// MP v3 dbbad909 and the test server's style-B4Ycg3oZ.css use these same scales.
const scales = { none: [8, 8, 8, 10], small: [12, 12, 10, 14], default: [20, 16, 14, 22], large: [24, 20, 16, 26], extra: [28, 24, 18, 30] };
const variables = ['--app-theme-surface-radius', '--app-field-radius', '--app-control-radius', '--app-overlay-radius'];

for (const glassEnabled of [false, true]) {
  test(`${glassEnabled ? 'glass' : 'plain'} materials match current MP geometry and preserve status colors`, async ({ page, api }) => {
    await page.addInitScript(glassEnabled => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme: 'light', glassEnabled, glassQuality: 'css', glassAppearance: 'clear', layout: 'vertical', radius: 'default' })), glassEnabled);
    api.respond('GET', '/api/media_organize/history', { total: 2, items: [
      { id: 'failed', item_id: '101', name: '失败记录', item_type: 'Movie', status: 'failed', message: '详细失败原因' },
      { id: 'success', item_id: '102', name: '成功记录', item_type: 'Movie', status: 'success', message: '入库成功' },
    ] });
    await page.goto('/media-organize');
    for (const [title, color] of [['失败', 'error'], ['成功', 'success']]) {
      const chip = page.getByRole('button', { name: `${title}，查看${title}记录详情`, exact: true });
      const colors = await chip.evaluate((element, color) => {
        const style = getComputedStyle(element);
        return { actual: style.color, text: getComputedStyle(element.querySelector('.v-chip__content')).color,
          expected: `rgb(${style.getPropertyValue(`--v-theme-${color}`).trim().split(/[\s,]+/).join(', ')})` };
      }, color);
      expect(colors.actual).toBe(colors.expected);
      expect(colors.text).toBe(colors.expected);
    }
    await page.getByRole('button', { name: '失败，查看失败记录详情', exact: true }).click();
    const dialog = page.getByTestId('organize-detail-dialog');
    for (const [radius, expected] of Object.entries(scales)) {
      await page.evaluate(radius => { document.documentElement.dataset.themeRadius = radius; }, radius);
      const geometry = await dialog.evaluate((element, variables) => ({
        tokens: variables.map(name => parseFloat(getComputedStyle(element).getPropertyValue(name))),
        dialogRadius: parseFloat(getComputedStyle(element).borderTopLeftRadius),
      }), variables);
      expect(geometry.tokens).toEqual(expected);
      await expect(dialog).toHaveCSS('border-top-left-radius', `${expected[3]}px`);
    }
    await expect(dialog.getByRole('button', { name: '重新整理', exact: true })).toBeVisible();
    await expect(dialog.getByRole('button', { name: '重新刮削', exact: true })).toBeVisible();
    expect(api.requests.filter(request => request.method !== 'GET')).toHaveLength(0);
  });
}
