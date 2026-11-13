import { writeFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';

for (const width of [390, 768, 1440, 2194, 3840]) {
  test(`${width}px media server and storage summary cards follow the available settings width`, async ({ page, api }, testInfo) => {
    await page.setViewportSize({ width, height: width >= 2194 ? 1234 : 900 });
    const geometries = [];
    for (const [tab, name] of [['system', '编辑 Emby 媒体服务器'], ['directory', '编辑 115 存储']]) {
      await page.goto(`/settings/general?tab=${tab}`);
      const summary = page.getByRole('button', { name, exact: true });
      await expect(summary).toBeVisible();
      const geometry = await summary.evaluate(element => {
        const outer = element.closest('.setting-card');
        const column = outer.parentElement;
        const row = column.parentElement;
        const bounds = element => {
          const r = element.getBoundingClientRect();
          return { left: r.left, right: r.right, width: r.width };
        };
        const grid = element.parentElement;
        return { outer: bounds(outer), summary: bounds(element), grid: bounds(grid), row: bounds(row),
          overflow: document.documentElement.scrollWidth - innerWidth };
      });
      geometries.push({ tab, ...geometry });
      expect(geometry.overflow).toBeLessThanOrEqual(1);
      expect(Math.abs(geometry.summary.width - geometry.grid.width)).toBeLessThanOrEqual(1);
      if (width >= 768) expect(geometry.outer.width / geometry.row.width).toBeLessThanOrEqual(.5);
      else expect(geometry.outer.width / geometry.row.width).toBeGreaterThan(.9);
      expect(geometry.summary.left).toBeGreaterThanOrEqual(geometry.outer.left);
      expect(geometry.summary.right).toBeLessThanOrEqual(geometry.outer.right);
    }
    writeFileSync(testInfo.outputPath('summary-widths.json'), JSON.stringify(geometries, null, 2));
    expect(api.requests.filter(request => request.method !== 'GET' && request.path !== '/api/config/rename_preview')).toHaveLength(0);
  });
}
