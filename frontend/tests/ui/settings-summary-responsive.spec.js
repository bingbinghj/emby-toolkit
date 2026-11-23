import { writeFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';

// MP v3.2.11 cf02b12 retains the grid; entity cards now use a compact header and information dock.
const mp = {
  1440: { outerWidth: 1164, gridWidth: 1122, cardWidth: 366 },
  2194: { outerWidth: 1918, gridWidth: 1876, cardWidth: 302.666667 },
};

for (const width of [390, 768, 1024, 1440, 1920, 2194, 3840]) {
  test(`${width}px media server and storage cards use MP's native grid, padding and natural dimensions`, async ({ page, api }, testInfo) => {
    test.setTimeout(60_000);
    await page.setViewportSize({ width, height: width >= 2194 ? 1234 : 900 });
    await page.addInitScript(() => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({
      theme: 'dark', glassEnabled: true, glassQuality: 'css', glassAppearance: 'clear', layout: 'vertical',
    })));
    api.respond('GET', '/api/p115/status', { data: { valid: true, has_token: true, user_info: {
      rt_space_info: { all_total: { size: 150.64 * 1024 ** 4 }, all_use: { size: 132.62 * 1024 ** 4 }, all_remain: { size: 18.02 * 1024 ** 4 } },
    } } });
    const geometries = [];
    for (const [tab, name, footer] of [
      ['system', '编辑 Emby 媒体服务器', '配置媒体服务器'], ['directory', '编辑 115 存储', '配置存储'],
    ]) {
      await page.goto(`/settings/general?tab=${tab}`);
      const summary = page.getByRole('button', { name, exact: true });
      await expect(summary).toBeVisible();
      await summary.scrollIntoViewIfNeeded();
      await summary.locator('img').evaluate(image => { image.loading = 'eager'; });
      await expect.poll(() => summary.locator('img').evaluate(image => image.complete && image.naturalWidth > 0), { timeout: 15_000 }).toBe(true);
      // Third-party font availability must not block geometry of rendered native controls.
      await expect.poll(() => summary.evaluate(el => el.getBoundingClientRect().width)).toBeGreaterThan(0);
      const geometry = await summary.evaluate(element => {
        const outer = element.closest('.setting-card');
        const bounds = element => {
          const { x, y, width, height } = element.getBoundingClientRect();
          return { x, y, width, height };
        };
        const padding = element => {
          const style = getComputedStyle(element);
          return [style.paddingTop, style.paddingRight, style.paddingBottom, style.paddingLeft].map(parseFloat);
        };
        const grid = element.parentElement;
        return { outer: bounds(outer), summary: bounds(element), grid: bounds(grid), row: bounds(outer.parentElement.parentElement),
          headerPadding: padding(outer.querySelector('.v-card-item')), bodyPadding: padding(grid.parentElement),
          summaryPadding: padding(element.querySelector('.mp-setting-card__header')), columns: getComputedStyle(grid).gridTemplateColumns.split(' ').length,
          rootFont: parseFloat(getComputedStyle(document.documentElement).fontSize), overflow: document.documentElement.scrollWidth - innerWidth };
      });
      geometries.push({ tab, ...geometry });
      writeFileSync(testInfo.outputPath('summary-dimensions.json'), JSON.stringify(geometries, null, 2));
      expect(geometry.overflow).toBeLessThanOrEqual(1);
      expect(geometry.headerPadding).toEqual([20, 20, 20, 20]);
      expect(geometry.bodyPadding).toEqual([0, 20, 20, 20]);
      expect(geometry.summaryPadding).toEqual([14, 12, 12, 15]);
      expect(geometry.outer.width).toBeCloseTo(geometry.row.width - 24, 0);
      expect(geometry.grid.width).toBeCloseTo(geometry.outer.width - 42, 0);
      const columns = Math.floor((geometry.grid.width + 12) / (17 * geometry.rootFont + 12));
      expect(geometry.columns).toBe(columns);
      expect(geometry.summary.width).toBeCloseTo((geometry.grid.width - 12 * (columns - 1)) / columns, 0);
      expect(geometry.summary.height).toBeGreaterThanOrEqual(104);
      expect(geometry.summary.height).toBeLessThan(122);
      await expect(summary.locator('.mp-setting-card__dock')).toBeVisible();
      if (mp[width]) {
        expect(geometry.outer.width).toBeCloseTo(mp[width].outerWidth, 0);
        expect(geometry.grid.width).toBeCloseTo(mp[width].gridWidth, 0);
        expect(geometry.summary.width).toBeCloseTo(mp[width].cardWidth, 0);
      }
      if (tab === 'system') await expect(summary.getByLabel(/音乐数量/)).toHaveCount(0);
      else await expect(summary.getByText('132.6 TB / 150.6 TB', { exact: true })).toBeVisible();
      const close = page.getByRole('button', { name: tab === 'system' ? '关闭媒体服务器配置' : '关闭存储配置', exact: true });
      for (const entry of [summary, page.getByRole('button', { name: footer, exact: true })]) {
        await entry.click();
        await expect(close).toBeVisible();
        await close.click();
        await expect(close).toBeHidden();
      }
      if (mp[width]) await summary.locator('xpath=ancestor::*[contains(concat(" ",normalize-space(@class)," ")," setting-card ")][1]').screenshot({ path: testInfo.outputPath(`summary-${tab}-${width}.png`) });
    }
    expect(api.requests.filter(request => request.method !== 'GET' && request.path !== '/api/config/rename_preview')).toHaveLength(0);
  });
}
