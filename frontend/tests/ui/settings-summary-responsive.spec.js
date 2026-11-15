import { writeFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';

// Production MP v3, identical viewport and 16px root font; measured 2026-11-15.
const mp = {
  1440: { outerWidth: 1164, gridWidth: 1122, cardWidth: 271.5 },
  2194: { outerWidth: 1918, gridWidth: 1876, cardWidth: 257.703125 },
};

for (const width of [390, 768, 1024, 1440, 1920, 2194, 3840]) {
  test(`${width}px media server and storage cards use MP's native grid, padding and natural dimensions`, async ({ page, api }, testInfo) => {
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
      await expect.poll(() => summary.locator('img').evaluate(image => image.complete && image.naturalWidth > 0)).toBe(true);
      await page.evaluate(() => document.fonts.ready);
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
          summaryPadding: padding(element.querySelector('.v-card-text')), columns: getComputedStyle(grid).gridTemplateColumns.split(' ').length,
          rootFont: parseFloat(getComputedStyle(document.documentElement).fontSize), overflow: document.documentElement.scrollWidth - innerWidth };
      });
      geometries.push({ tab, ...geometry });
      writeFileSync(testInfo.outputPath('summary-dimensions.json'), JSON.stringify(geometries, null, 2));
      expect(geometry.overflow).toBeLessThanOrEqual(1);
      expect(geometry.headerPadding).toEqual([20, 20, 20, 20]);
      expect(geometry.bodyPadding).toEqual([0, 20, 20, 20]);
      expect(geometry.summaryPadding).toEqual([20, 20, 20, 20]);
      expect(geometry.outer.width).toBeCloseTo(geometry.row.width - 24, 0);
      expect(geometry.grid.width).toBeCloseTo(geometry.outer.width - 42, 0);
      const columns = Math.floor((geometry.grid.width + 12) / (16 * geometry.rootFont + 12));
      expect(geometry.columns).toBe(columns);
      expect(geometry.summary.width).toBeCloseTo((geometry.grid.width - 12 * (columns - 1)) / columns, 0);
      expect(geometry.summary.height).toBeCloseTo(tab === 'system' ? 122 : 123.96875, 0);
      expect(geometry.outer.height).toBeCloseTo(tab === 'system' ? 313.9375 : 315.90625, 0);
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
