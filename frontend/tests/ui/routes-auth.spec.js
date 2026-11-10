import { test, expect } from './fixtures.js';

const privateRoutes = [
  ['/dashboard', '仪表盘'],
  ['/user-management', '用户管理'],
  ['/user-center', '用户管理'],
  ['/media-organize', '媒体整理'],
  ['/settings/notifications', '通知设置'],
  ['/review', '手动处理'],
  ['/media-center', '仪表盘'],
  ['/media-center/library/movies', '电影'],
  ['/library/movies', '电影'],
  ['/settings/general', '通用设置'],
  ['/settings/scheduler', '任务中心'],
  ['/settings/cover-generator', '媒体库封面生成'],
  ['/edit-media/101', '手动编辑媒体信息'],
];

for (const [path, heading] of privateRoutes) {
  test(`authenticated route renders ${path}`, async ({ page }) => {
    await page.goto(path);
    await expect(page.getByRole('heading', { name: heading, exact: true }).first()).toBeVisible();
    await expect(page.locator('.v-application')).toBeVisible();
  });
}

test.describe('login required', () => {
  test.use({ authState: 'login_required' });
  test('private route redirects to login and completed login opens dashboard', async ({ page, api }) => {
    await page.goto('/settings/general');
    await expect(page).toHaveURL(/\/login$/);
    await page.getByLabel('用户名', { exact: true }).fill('UI Test');
    await page.getByLabel('密码', { exact: true }).fill('fixture-password');
    await page.getByRole('button', { name: '登录', exact: true }).click();
    await expect(page).toHaveURL(/\/dashboard$/);
    await expect(page.getByRole('heading', { name: '仪表盘', exact: true })).toBeVisible();
    expect(api.requests.find(request => request.path === '/api/auth/login').body).toEqual({ username: 'UI Test', password: 'fixture-password' });
  });
});

test.describe('initial setup required', () => {
  test.use({ authState: 'setup_required' });
  test('private route redirects to setup with working connection form', async ({ page, api }) => {
    await page.goto('/review');
    await expect(page).toHaveURL(/\/setup$/);
    await expect(page.getByText('初始化配置向导', { exact: true })).toBeVisible();
    await page.getByLabel('服务器地址 (URL)', { exact: true }).fill('http://emby.invalid:8096');
    await page.getByLabel('API 密钥 (API Key)', { exact: true }).fill('fixture-only');
    await page.getByRole('button', { name: '测试并保存', exact: true }).click();
    await expect.poll(() => api.requests.some(request => request.path === '/api/auth/setup')).toBe(true);
    expect(api.requests.find(request => request.path === '/api/auth/setup').body).toEqual({ url: 'http://emby.invalid:8096', api_key: 'fixture-only' });
    await expect(page).toHaveURL(/\/login$/, { timeout: 10_000 });
  });
});

test.describe('ordinary account', () => {
  test.use({ admin: false });
  test('account details remain available and management navigation stays hidden', async ({ page, api }) => {
    await page.goto('/user-management');
    await expect(page.getByRole('heading', { name: '用户管理', exact: true })).toBeVisible();
    await expect(page.getByText('普通用户', { exact: true }).first()).toBeVisible();
    await expect(page.getByRole('button', { name: '通知中心', exact: true })).toHaveCount(0);
    await expect(page.locator('.v-navigation-drawer').getByText('通用设置', { exact: true })).toHaveCount(0);
    expect(api.requests.some(request => request.path === '/api/status')).toBe(false);
  });
});

test('menu order changes through accessible controls and is saved', async ({ page, api }) => {
  await page.goto('/user-management');
  await page.getByRole('button', { name: 'UI Test 的用户菜单', exact: true }).click();
  await page.getByText('自定义导航', { exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存并应用', exact: true }) });
  const firstGroup = dialog.locator('.menu-editor-group').nth(2);
  const items = firstGroup.locator('.menu-editor-item');
  await expect(items).toHaveCount(5);
  await items.nth(1).getByRole('button', { name: '上移菜单', exact: true }).click();
  await expect(items.nth(0).getByLabel('菜单名称', { exact: true })).toHaveValue('通知设置');
  await dialog.getByRole('button', { name: '保存并应用', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/system/menu_config')).toBe(true);
  const saved = api.requests.find(request => request.method === 'POST' && request.path === '/api/system/menu_config').body;
  expect(saved[2].children.map(item => item.key)).toEqual(['settings-notifications', 'settings-general', 'UserManagement', 'settings-scheduler', 'CoverGeneratorConfig']);
  await page.reload();
  await page.getByRole('button', { name: 'UI Test 的用户菜单', exact: true }).click();
  await page.getByText('自定义导航', { exact: true }).click();
  await expect(page.locator('.menu-editor-group').nth(2).locator('.menu-editor-item').first().getByLabel('菜单名称', { exact: true })).toHaveValue('通知设置');
});

test('mobile navigation opens without horizontal page overflow', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/user-management');
  const toggle = page.getByRole('button', { name: '切换导航', exact: true });
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  const drawer = page.locator('.v-navigation-drawer');
  await expect(drawer.getByRole('button', { name: '关闭导航', exact: true })).toBeVisible();
  await drawer.getByText('通用设置', { exact: true }).click();
  await expect(page).toHaveURL(/\/settings\/general$/);
  await expect(page.getByRole('heading', { name: '通用设置', exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(392);
});
