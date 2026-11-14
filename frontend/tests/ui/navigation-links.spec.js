import { test, expect } from './fixtures.js';

async function prepare(page, layout, width = 1440) {
  await page.setViewportSize({ width, height: 900 });
  await page.addInitScript(layout => {
    localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme: 'light', glassEnabled: false, layout }));
  }, layout);
}

async function recordPushes(page) {
  await page.evaluate(() => {
    const router = document.querySelector('#app').__vue_app__.config.globalProperties.$router;
    const push = router.push.bind(router);
    window.__etkNavigationPushes = [];
    router.push = (...args) => { window.__etkNavigationPushes.push(args); return push(...args); };
  });
}

async function expectSingleNavigation(page) {
  await expect(page).toHaveURL(/\/settings\/general$/);
  expect(await page.evaluate(() => window.__etkNavigationPushes.length)).toBe(1);
}

for (const layout of ['vertical', 'collapsed']) test(`${layout} ordinary sidebar links expose the route and navigate once`, async ({ page }) => {
  await prepare(page, layout);
  await page.goto('/dashboard');
  const drawer = page.locator('.layout-vertical-nav');
  const settings = drawer.getByRole('link', { name: '设置', exact: true });
  await expect(settings).toHaveAttribute('href', '/settings/general');
  await expect(drawer.getByRole('link', { name: '仪表盘', exact: true })).toHaveClass(/v-list-item--active/);
  await recordPushes(page);
  await settings.click();
  await expectSingleNavigation(page);
  await expect(settings).toHaveClass(/v-list-item--active/);
});

test('horizontal groups stay buttons and their pages are ordinary links', async ({ page }) => {
  await prepare(page, 'horizontal');
  await page.goto('/dashboard');
  const navigation = page.getByRole('navigation', { name: '主导航', exact: true });
  const group = navigation.getByRole('button', { name: '系统', exact: true });
  await expect(group).not.toHaveAttribute('href');
  await group.hover();
  const settings = page.locator('.theme-horizontal-nav__menu').getByRole('link', { name: '设置', exact: true });
  await expect(settings).toHaveAttribute('href', '/settings/general');
  await recordPushes(page);
  await settings.click();
  await expectSingleNavigation(page);
  await expect(page.locator('.theme-horizontal-nav__menu')).toBeHidden();
});

test('horizontal standalone custom menus keep their names and route links', async ({ page, api }) => {
  await prepare(page, 'horizontal');
  api.respond('GET', '/api/system/menu_config', [{ key: 'settings-general', label: '我的设置', visible: true }]);
  await page.goto('/dashboard');
  const settings = page.getByRole('navigation', { name: '主导航', exact: true }).getByRole('link', { name: '我的设置', exact: true });
  await expect(settings).toHaveAttribute('href', '/settings/general');
  await recordPushes(page);
  await settings.click();
  await expectSingleNavigation(page);
});

test('Control and middle click open the real menu href without navigating the original page', async ({ page }) => {
  await prepare(page, 'vertical');
  // The second tab only verifies native link opening; it need not boot another app.
  await page.context().route('**/settings/general', route => route.fulfill({ contentType: 'text/html', body: '<title>Navigation destination</title>' }));
  await page.goto('/dashboard');
  const drawer = page.locator('.layout-vertical-nav');
  const settings = drawer.getByRole('link', { name: '设置', exact: true });
  await expect(settings).toHaveAttribute('href', '/settings/general');
  await recordPushes(page);
  for (const click of [{ modifiers: ['Control'] }, { button: 'middle' }]) {
    const opened = page.context().waitForEvent('page');
    await settings.click(click);
    const tab = await opened;
    await expect(tab).toHaveURL(/\/settings\/general$/);
    await expect(page).toHaveURL(/\/dashboard$/);
    expect(await page.evaluate(() => window.__etkNavigationPushes.length)).toBe(0);
    await expect(drawer).toHaveClass(/\bvisible\b/);
    await tab.close();
  }
});

test('mobile dock links close the drawer even when activating the current route', async ({ page }) => {
  await prepare(page, 'vertical', 390);
  await page.goto('/dashboard');
  const dock = page.getByRole('navigation', { name: '快捷导航', exact: true });
  const dashboard = dock.getByRole('link', { name: '仪表盘', exact: true });
  await expect(dashboard).toHaveAttribute('href', '/dashboard');
  const more = dock.getByRole('button', { name: '更多导航', exact: true });
  await expect(more).not.toHaveAttribute('href');
  await more.click();
  const drawer = page.locator('.layout-vertical-nav');
  await expect(drawer).toHaveClass(/\bvisible\b/);
  await recordPushes(page);
  await dashboard.click();
  await expect(drawer).not.toHaveClass(/\bvisible\b/);
  await expect(page).toHaveURL(/\/dashboard$/);
  expect(await page.evaluate(() => window.__etkNavigationPushes.length)).toBe(1);
  await more.click();
  const settings = drawer.getByRole('link', { name: '设置', exact: true });
  await expect(settings).toHaveAttribute('href', '/settings/general');
  await settings.click();
  await expect(page).toHaveURL(/\/settings\/general$/);
  await expect(drawer).not.toHaveClass(/\bvisible\b/);
  expect(await page.evaluate(() => window.__etkNavigationPushes.length)).toBe(2);
});
