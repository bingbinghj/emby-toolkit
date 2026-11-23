import { test, expect } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';

for (const appearance of ['clear', 'tinted', 'frosted']) for (const scheme of ['light', 'dark']) for (const width of [390, 1440]) {
  test(`新版 MP 设置卡片、弹窗和背景可读 ${appearance} ${scheme} ${width}`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.addInitScript(({scheme, appearance}) => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({
      theme: scheme, glassEnabled: true, glassQuality: 'css', glassAppearance: appearance,
    })), {scheme, appearance});
    await page.goto('/settings/general?tab=system');
    const card = page.getByRole('button', { name: '编辑 Emby 媒体服务器', exact: true });
    await expect(card.locator('.mp-setting-card__dock')).toBeVisible();
    await expect(page.locator('.wallpaper-layer-active')).toBeVisible();
    await card.scrollIntoViewIfNeeded();
    for (const locator of [card.locator('.mp-setting-card__title'), card.locator('.mp-setting-card__subtitle-text')]) {
      const size = await locator.evaluate(el => ({ width: el.clientWidth, content: el.scrollWidth, height: el.clientHeight, contentHeight: el.scrollHeight }));
      expect(size.content).toBeLessThanOrEqual(size.width + 1);
      expect(size.contentHeight).toBeLessThanOrEqual(size.height + 1);
    }
    for (const reading of await measureReadingContrast(page, [
      { role: '设置卡片标题', locator: card.locator('.mp-setting-card__title') },
      { role: '设置卡片副标题', locator: card.locator('.mp-setting-card__subtitle-text') },
      { role: '媒体统计', locator: card.locator('.mp-setting-card__stat-value').first() },
    ])) expect(reading.contrast, reading.role).toBeGreaterThanOrEqual(4.5);
    await page.screenshot({ path: testInfo.outputPath('settings.png') });
    await card.click();
    const dialog = page.getByRole('dialog');
    await expect(dialog.getByRole('button', { name: '关闭媒体服务器配置', exact: true })).toBeVisible();
    const box = await dialog.boundingBox();
    expect(box.x).toBeGreaterThanOrEqual(-1);
    expect(box.x + box.width).toBeLessThanOrEqual(width + 1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width + 1);
    await page.screenshot({ path: testInfo.outputPath('dialog.png') });
  });
}

for (const quality of ['css', 'balanced', 'high']) {
  test(`登录页尊重 ${quality} 档位并在图形上下文受限时降级`, async ({ page, api }) => {
    await page.addInitScript(quality => {
      localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme: 'light', glassEnabled: true, glassQuality: quality }));
      const getContext = HTMLCanvasElement.prototype.getContext;
      window.__mpContextOptions = [];
      HTMLCanvasElement.prototype.getContext = function (type, options) {
        if (type === 'webgl2') { window.__mpContextOptions.push(options); return null; }
        return getContext.call(this, type, options);
      };
    }, quality);
    api.setAuth('login_required');
    await page.goto('/login');
    await expect(page.getByRole('button', { name: '登录', exact: true })).toBeVisible();
    await expect(page.locator('html')).toHaveAttribute('data-glass-quality', quality);
    if (quality === 'css') {
      expect(await page.evaluate(() => window.__mpContextOptions)).toEqual([]);
      await expect(page.locator('canvas[data-presentation-space]')).toHaveCount(0);
    } else {
      await expect.poll(() => page.evaluate(() => window.__mpContextOptions.length)).toBeGreaterThan(0);
      expect(await page.evaluate(() => window.__mpContextOptions.every(options => options.failIfMajorPerformanceCaveat === true))).toBe(true);
      await expect(page.locator('html')).toHaveAttribute('data-glass-renderer-state', 'fallback');
    }
    await expect(page.locator('.wallpaper-layer-active')).toBeVisible();
  });
}
