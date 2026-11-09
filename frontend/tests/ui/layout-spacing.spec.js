import { test, expect } from './fixtures.js';

async function expectTitleBelowNavbar(page, title) {
  const heading = page.getByRole('heading', { name: title, exact: true });
  await expect(heading).toBeVisible();
  await expect.poll(async () => {
    const navbar = await page.locator('.layout-navbar').boundingBox();
    const content = await heading.boundingBox();
    return navbar && content ? content.y - (navbar.y + navbar.height) : -Infinity;
  }, { message: '页面标题必须位于导航栏底部之后，并保留可读间距' }).toBeGreaterThanOrEqual(12);
  const navbar = await page.locator('.layout-navbar').boundingBox();
  const content = await heading.boundingBox();
  expect(content.y - (navbar.y + navbar.height), 'VMain 已预留导航高度，不应重复添加整段导航占位').toBeLessThanOrEqual(48);
}

const routes = [
  { path: '/media-center', title: '媒体中心' },
  { path: '/settings/general', title: '通用设置' },
];
for (const colorScheme of ['light', 'dark']) {
  for (const width of [1440, 390]) {
    for (const route of routes) {
      test(`${colorScheme} ${width}px ${route.title} keeps its heading below the floating navbar`, async ({ page }) => {
        await page.setViewportSize({ width, height: 900 });
        await page.emulateMedia({ colorScheme, reducedMotion: 'reduce' });
        await page.addInitScript(theme => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({
          theme, glassEnabled: true, glassQuality: 'css', glassAppearance: 'clear', layout: 'vertical',
        })), colorScheme);
        await page.goto(route.path, { waitUntil: 'domcontentloaded' });
        await expectTitleBelowNavbar(page, route.title);
      });
    }
  }
}

for (const layout of ['collapsed', 'horizontal']) {
  test(`${layout} desktop layout reserves navbar height once`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
    await page.addInitScript(layout => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({
      theme: 'dark', glassEnabled: true, glassQuality: 'css', glassAppearance: 'clear', layout,
    })), layout);
    await page.goto('/media-center', { waitUntil: 'domcontentloaded' });
    await expectTitleBelowNavbar(page, '媒体中心');
  });
}
