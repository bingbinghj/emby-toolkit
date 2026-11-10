import { test, expect } from './fixtures.js';

const themes = [
  { name: 'light', theme: 'light', glassEnabled: false },
  { name: 'dark', theme: 'dark', glassEnabled: false },
  ...['clear', 'tinted', 'frosted'].flatMap(glassAppearance => ['adaptive', 'clear'].map(glassUIStyle => ({ name: `glass-${glassAppearance}-${glassUIStyle}`, theme: 'dark', glassEnabled: true, glassAppearance, glassUIStyle, glassQuality: 'css' }))),
];

for (const theme of themes) {
  for (const layout of ['vertical', 'collapsed', 'horizontal']) {
    test(`${theme.name} ${layout} navigation remains attached to the viewport while settings scroll`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width: 1440, height: 700 });
      await page.addInitScript(settings => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify(settings)), { ...theme, layout });
      await page.goto('/settings/general');
      const navbar = page.locator('.layout-navbar');
      const drawer = page.locator('.layout-vertical-nav');
      await expect(navbar).toBeVisible();
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme.glassEnabled ? 'glass' : 'plain');
      await expect(page.locator('html')).toHaveAttribute('data-theme-preference', theme.theme);
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollHeight)).toBeGreaterThan(1100);
      // Horizontal navigation has the same small MP floating transition at both positions.
      await page.evaluate(() => window.scrollTo(0, 200));
      await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThanOrEqual(200);
      await expect.poll(() => navbar.evaluate(element => getComputedStyle(element).position)).toBe('fixed');
      const start = { navbar: await navbar.boundingBox(), drawer: layout !== 'horizontal' ? await drawer.boundingBox() : null };
      await page.evaluate(() => window.scrollTo(0, 700));
      await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThanOrEqual(600);
      await expect.poll(async () => Math.abs((await navbar.boundingBox()).y - start.navbar.y)).toBeLessThan(1);
      if (start.drawer) {
        await expect.poll(() => drawer.evaluate(element => getComputedStyle(element).position)).toBe('fixed');
        await expect.poll(async () => Math.abs((await drawer.boundingBox()).y - start.drawer.y)).toBeLessThan(1);
        const end = await drawer.boundingBox();
        expect(end.height).toBeLessThanOrEqual(700);
        if (layout === 'vertical') expect(await drawer.locator('.v-list-item-title').first().evaluate(element => parseFloat(getComputedStyle(element).fontSize))).toBeGreaterThanOrEqual(16);
      }
      await expect(page.getByRole('button', { name: /折叠导航|展开导航|切换导航/ })).toHaveCount(0);
      await page.screenshot({ path: testInfo.outputPath('scrolled-shell.png') });
    });
  }
}

test('mobile navigation can open after scrolling and closes on navigation', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/settings/general');
  await page.evaluate(() => window.scrollTo(0, 500));
  const toggle = page.getByRole('button', { name: '切换导航', exact: true });
  await expect(toggle).toBeInViewport();
  await toggle.click();
  const drawer = page.locator('.layout-vertical-nav');
  await expect(drawer.getByRole('button', { name: '关闭导航', exact: true })).toBeInViewport();
  await drawer.getByText('仪表盘', { exact: true }).click();
  await expect(page).toHaveURL(/\/dashboard$/);
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(392);
});

test('user menu shows the avatar with role above username, and default management follows cover generation', async ({ page }) => {
  await page.goto('/');
  await expect(page).toHaveURL(/\/dashboard$/);
  const drawer = page.locator('.layout-vertical-nav');
  const navNames = await drawer.locator('.v-list-item-title').allTextContents();
  expect(navNames.at(-1)).toBe('用户管理');
  expect(navNames.indexOf('用户管理')).toBe(navNames.indexOf('封面生成') + 1);
  await page.getByRole('button', { name: 'UI Test 的用户菜单', exact: true }).click();
  const menu = page.locator('.user-profile-menu');
  await expect(menu.locator('.v-avatar')).toBeVisible();
  const role = menu.getByText('管理员', { exact: true }), username = menu.getByText('UI Test', { exact: true });
  await expect(role).toBeVisible();
  await expect(username).toBeVisible();
  expect((await role.boundingBox()).y).toBeLessThan((await username.boundingBox()).y);
});
