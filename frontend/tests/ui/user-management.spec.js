import { test, expect } from './fixtures.js';

test('user management shows real roles and disabled status without notification configuration', async ({ page }) => {
  await page.goto('/user-management');
  await expect(page.getByRole('heading', { name: '用户管理', exact: true })).toBeVisible();
  await expect(page.getByText('Media Member', { exact: true })).toBeVisible();
  await expect(page.getByText('已禁用', { exact: true })).toBeVisible();
  await expect(page.getByLabel('Telegram Bot Token', { exact: true })).toHaveCount(0);
});

test('user detail save changes only ETK notes and notification contact', async ({ page, api }) => {
  await page.goto('/user-management');
  await page.getByRole('button', { name: '编辑 Media Member 的ETK资料', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存用户资料', exact: true }) });
  await dialog.getByLabel('备注', { exact: true }).fill('修改后的备注');
  await dialog.getByLabel('Telegram 用户ID（私聊ID）', { exact: true }).fill('456');
  await dialog.getByRole('button', { name: '保存用户资料', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/portal/users/member')).toBe(true);
  expect(api.requests.find(request => request.method === 'POST' && request.path === '/api/portal/users/member').body).toEqual({ notes: '修改后的备注', telegram_chat_id: '456' });
  await expect(page.getByText('修改后的备注', { exact: true })).toBeVisible();
  await expect(page.getByText('已禁用', { exact: true })).toBeVisible();
});

test('failed user detail save keeps the dialog and draft', async ({ page, api }) => {
  api.respond('POST', '/api/portal/users/member', { error: '用户资料暂时无法保存' }, 503);
  await page.goto('/user-management');
  await page.getByRole('button', { name: '编辑 Media Member 的ETK资料', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存用户资料', exact: true }) });
  await dialog.getByLabel('备注', { exact: true }).fill('保留草稿');
  await dialog.getByRole('button', { name: '保存用户资料', exact: true }).click();
  await expect(dialog.getByText('用户资料暂时无法保存', { exact: true })).toBeVisible();
  await expect(dialog.getByLabel('备注', { exact: true })).toHaveValue('保留草稿');
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
