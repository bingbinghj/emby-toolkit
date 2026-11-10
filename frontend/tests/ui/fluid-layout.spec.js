import { test, expect } from './fixtures.js';

for (const width of [1920, 1440, 390]) {
  test(`${width}px dashboard and settings use MoviePilot's fluid content spacing`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    for (const path of ['/dashboard', '/settings/general']) {
      await page.goto(path);
      if (path === '/settings/general') {
        await expect(page.getByRole('tab', { name: '系统', exact: true })).toHaveAttribute('aria-selected', 'true');
        await expect(page.locator('.setting-card').first()).toBeVisible();
      } else await expect(page.locator('h1')).toBeVisible();
      await expect.poll(() => page.locator('.layout-page-content').evaluate(element => {
        const style = getComputedStyle(element);
        const child = element.querySelector('.main-content-wrapper');
        const main = element.closest('.v-main');
        const mainStyle = getComputedStyle(main);
        return {
          left: parseFloat(style.paddingLeft), right: parseFloat(style.paddingRight),
          maxWidth: style.maxWidth,
          contentWidth: Math.round(element.getBoundingClientRect().width),
          childWidth: Math.round(child.getBoundingClientRect().width),
          available: Math.round(main.clientWidth - parseFloat(mainStyle.paddingLeft) - parseFloat(mainStyle.paddingRight)),
          overflow: document.documentElement.scrollWidth > window.innerWidth,
        };
      })).toEqual({
        left: 8, right: 8, maxWidth: 'none',
        contentWidth: width > 1280 ? width - 260 : width,
        childWidth: width > 1280 ? width - 276 : width - 16,
        available: width > 1280 ? width - 260 : width,
        overflow: false,
      });
    }
  });
}

for (const layout of ['collapsed', 'horizontal']) {
  test(`${layout} wide dashboard fills available content without horizontal overflow`, async ({ page }) => {
    await page.setViewportSize({ width: 1920, height: 900 });
    await page.addInitScript(layout => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ layout })), layout);
    await page.goto('/dashboard');
    await expect(page.getByRole('heading', { name: '仪表盘', exact: true })).toBeVisible();
    await expect.poll(() => page.locator('.layout-page-content').evaluate(element => {
      const bounds = element.getBoundingClientRect();
      return Math.round(bounds.right);
    })).toBe(1920);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(1920);
  });
}
