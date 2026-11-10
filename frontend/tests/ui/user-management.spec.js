import { test, expect } from './fixtures.js';

test('user management uses MP user cards with native roles and status', async ({ page }) => {
  await page.goto('/user-management');
  await expect(page.getByRole('heading', { name: '用户管理', exact: true })).toBeVisible();
  await expect(page.getByText('Media Member', { exact: true })).toBeVisible();
  await expect(page.getByText('已禁用', { exact: true })).toBeVisible();
  await expect(page.locator('.user-card')).toHaveCount(2);
  await expect(page.getByLabel('Telegram Bot Token', { exact: true })).toHaveCount(0);
  await expect(page.getByText('原备注', { exact: true })).toHaveCount(0);
});

test('edit profile updates real Emby name role and status without notification contacts', async ({ page, api }) => {
  await page.goto('/user-management');
  await page.getByRole('button', { name: '编辑 Media Member 的 Emby 资料', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存用户资料', exact: true }) });
  await expect(dialog.getByLabel('Telegram 用户ID（私聊ID）', { exact: true })).toHaveCount(0);
  await dialog.getByLabel('用户名', { exact: true }).fill('New Member');
  await dialog.getByLabel('用户状态', { exact: true }).focus();
  await dialog.getByLabel('用户状态', { exact: true }).press('Enter');
  await page.getByRole('option', { name: '正常', exact: true }).click();
  await dialog.getByRole('button', { name: '保存用户资料', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/portal/users/member')).toBe(true);
  expect(api.requests.find(request => request.method === 'POST' && request.path === '/api/portal/users/member').body).toEqual({ name: 'New Member', is_administrator: false, is_disabled: false });
  await expect(page.getByText('New Member', { exact: true })).toBeVisible();
  await expect(page.getByText('已禁用', { exact: true })).toHaveCount(0);
});

test('promoting a disabled user enables their account before saving the Emby administrator policy', async ({ page, api }) => {
  await page.goto('/user-management');
  await page.getByRole('button', { name: '编辑 Media Member 的 Emby 资料', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存用户资料', exact: true }) });
  await dialog.getByLabel('用户角色', { exact: true }).focus();
  await dialog.getByLabel('用户角色', { exact: true }).press('Enter');
  await page.getByRole('option', { name: '管理员', exact: true }).click();
  await dialog.getByRole('button', { name: '保存用户资料', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(api.requests.find(request => request.method === 'POST' && request.path === '/api/portal/users/member').body).toEqual({ name: 'Media Member', is_administrator: true, is_disabled: false });
});

test('failed Emby profile save keeps the dialog and username draft', async ({ page, api }) => {
  api.respond('POST', '/api/portal/users/member', { error: '用户资料暂时无法保存' }, 503);
  await page.goto('/user-management');
  await page.getByRole('button', { name: '编辑 Media Member 的 Emby 资料', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存用户资料', exact: true }) });
  await dialog.getByLabel('用户名', { exact: true }).fill('保留草稿');
  await dialog.getByRole('button', { name: '保存用户资料', exact: true }).click();
  await expect(dialog.getByText('用户资料暂时无法保存', { exact: true })).toBeVisible();
  await expect(dialog.getByLabel('用户名', { exact: true })).toHaveValue('保留草稿');
});

test('current user cannot disable themselves or remove their administrator role', async ({ page }) => {
  await page.goto('/user-management');
  await page.getByRole('button', { name: '编辑 UI Test 的 Emby 资料', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存用户资料', exact: true }) });
  await expect(dialog.getByLabel('用户角色', { exact: true })).toBeDisabled();
  await expect(dialog.getByLabel('用户状态', { exact: true })).toBeDisabled();
  await expect(dialog.getByLabel('用户名', { exact: true })).toBeEnabled();
  await expect(dialog.getByRole('button', { name: '更换头像', exact: true })).toBeVisible();
});

test('password change validates confirmation and uses an independent Emby operation', async ({ page, api }) => {
  await page.goto('/user-management');
  await page.getByRole('button', { name: '修改 Media Member 的密码', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存新密码', exact: true }) });
  await dialog.getByLabel('新密码', { exact: true }).fill('fixture-password');
  await dialog.getByLabel('确认新密码', { exact: true }).fill('different');
  await dialog.getByRole('button', { name: '保存新密码', exact: true }).click();
  await expect(dialog.getByText('两次输入的密码不一致', { exact: true })).toBeVisible();
  expect(api.requests.filter(request => request.method === 'POST' && request.path.startsWith('/api/portal/users/'))).toHaveLength(0);
  await dialog.getByLabel('确认新密码', { exact: true }).fill('fixture-password');
  await dialog.getByRole('button', { name: '保存新密码', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(api.requests.find(request => request.path === '/api/portal/users/member/password').body).toEqual({ new_password: 'fixture-password' });
  expect(api.requests.some(request => request.path === '/api/portal/users/member' && request.method === 'POST')).toBe(false);
});

test('mobile user cards and edit dialog fit the viewport', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/user-management');
  await expect(page.locator('.user-card')).toHaveCount(2);
  await page.getByRole('button', { name: '编辑 Media Member 的 Emby 资料', exact: true }).click();
  await expect(page.getByLabel('用户名', { exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('legacy media center navigation becomes dashboard and user management', async ({ page, api }) => {
  api.respond('GET', '/api/system/menu_config', [{ key: 'group-discovery', label: '发现', children: [
    { key: 'MediaCenter', label: '媒体中心', visible: true },
    { key: 'UserCenter', label: '用户中心', visible: true },
  ] }]);
  await page.goto('/media-center');
  await expect(page).toHaveURL(/\/dashboard$/);
  const navigation = page.locator('.v-navigation-drawer');
  await expect(navigation.getByText('仪表盘', { exact: true })).toBeVisible();
  await expect(navigation.getByText('用户管理', { exact: true })).toHaveCount(1);
  await expect(navigation.getByText('用户管理', { exact: true })).toBeVisible();
  await expect(navigation.getByText('媒体中心', { exact: true })).toHaveCount(0);
  await expect(navigation.getByText('用户中心', { exact: true })).toHaveCount(0);
});
