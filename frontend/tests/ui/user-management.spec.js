import { test, expect } from './fixtures.js';

test('user management uses MP user cards with native roles and status', async ({ page }) => {
  await page.goto('/user-management');
  await expect(page.getByRole('heading', { name: '用户管理', exact: true })).toHaveCount(0);
  await expect(page.getByText('Media Member', { exact: true })).toBeVisible();
  await expect(page.getByText('已停用', { exact: true })).toBeVisible();
  await expect(page.locator('.user-card')).toHaveCount(2);
  for (const label of ['Bot Token', 'Chat ID', '个人 Chat ID']) await expect(page.getByLabel(label, { exact: true })).toHaveCount(0);
  await expect(page.getByText('原备注', { exact: true })).toHaveCount(0);
});

test('edit profile updates real Emby name role and status without notification contacts', async ({ page, api }) => {
  await page.goto('/user-management');
  await page.getByRole('button', { name: '编辑 Media Member 的 Emby 资料', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存', exact: true }) });
  for (const label of ['Bot Token', 'Chat ID', '个人 Chat ID']) await expect(dialog.getByLabel(label, { exact: true })).toHaveCount(0);
  await dialog.getByLabel('用户名', { exact: true }).fill('New Member');
  await dialog.getByLabel('状态', { exact: true }).focus();
  await dialog.getByLabel('状态', { exact: true }).press('Enter');
  await page.getByRole('option', { name: '激活', exact: true }).click();
  await dialog.getByRole('button', { name: '保存', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/portal/users/member')).toBe(true);
  expect(api.requests.find(request => request.method === 'POST' && request.path === '/api/portal/users/member').body).toEqual({ name: 'New Member', is_administrator: false, is_disabled: false });
  await expect(page.getByText('New Member', { exact: true })).toBeVisible();
  await expect(page.getByText('已停用', { exact: true })).toHaveCount(0);
});

test('promoting a disabled user enables their account before saving the Emby administrator policy', async ({ page, api }) => {
  await page.goto('/user-management');
  await page.getByRole('button', { name: '编辑 Media Member 的 Emby 资料', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存', exact: true }) });
  await dialog.getByLabel('角色', { exact: true }).focus();
  await dialog.getByLabel('角色', { exact: true }).press('Enter');
  await page.getByRole('option', { name: '管理员', exact: true }).click();
  await dialog.getByRole('button', { name: '保存', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(api.requests.find(request => request.method === 'POST' && request.path === '/api/portal/users/member').body).toEqual({ name: 'Media Member', is_administrator: true, is_disabled: false });
});

test('failed Emby profile save keeps the dialog and username draft', async ({ page, api }) => {
  api.respond('POST', '/api/portal/users/member', { error: '用户资料暂时无法保存' }, 503);
  await page.goto('/user-management');
  await page.getByRole('button', { name: '编辑 Media Member 的 Emby 资料', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存', exact: true }) });
  await dialog.getByLabel('用户名', { exact: true }).fill('保留草稿');
  await dialog.getByRole('button', { name: '保存', exact: true }).click();
  await expect(dialog.getByText('用户资料暂时无法保存', { exact: true })).toBeVisible();
  await expect(dialog.getByLabel('用户名', { exact: true })).toHaveValue('保留草稿');
});

test('current user cannot disable themselves or remove their administrator role', async ({ page }) => {
  await page.goto('/user-management');
  await page.getByRole('button', { name: '编辑 UI Test 的 Emby 资料', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存', exact: true }) });
  await expect(dialog.getByLabel('角色', { exact: true })).toBeDisabled();
  await expect(dialog.getByLabel('状态', { exact: true })).toBeDisabled();
  await expect(dialog.getByLabel('用户名', { exact: true })).toBeEnabled();
  await expect(dialog.getByRole('button', { name: '上传头像', exact: true })).toBeVisible();
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

test('all Emby users can preview avatars, cancel changes and restore the default only on save', async ({ page, api }) => {
  await page.goto('/user-management');
  await page.getByRole('button', { name: '编辑 Media Member 的 Emby 资料', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存', exact: true }) });
  await expect(dialog.getByRole('button', { name: '上传头像', exact: true })).toBeVisible();
  const avatar = { name: 'avatar.png', mimeType: 'image/png', buffer: Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aCWQAAAAASUVORK5CYII=', 'base64') };
  await dialog.locator('input[type=file]').setInputFiles(avatar);
  await expect(dialog.locator('.avatar-editor img')).toHaveAttribute('src', /^blob:/);
  expect(api.requests.filter(request => request.method !== 'GET' && request.path.endsWith('/avatar'))).toHaveLength(0);
  await dialog.getByRole('button', { name: '取消头像修改', exact: true }).click();
  await expect(dialog.locator('.avatar-editor img')).toHaveCount(0);
  await dialog.getByRole('button', { name: '重置默认头像', exact: true }).click();
  expect(api.requests.filter(request => request.path.endsWith('/avatar'))).toHaveLength(0);
  await dialog.getByRole('button', { name: '保存', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(api.requests.filter(request => request.path.endsWith('/avatar')).map(request => ({ path: request.path, method: request.method }))).toEqual([{ path: '/api/portal/users/member/avatar', method: 'DELETE' }]);
});

test('saving an avatar uses the selected Emby user and waits for its verified image tag', async ({ page, api }) => {
  await page.goto('/user-management');
  await page.getByRole('button', { name: '编辑 Media Member 的 Emby 资料', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存', exact: true }) });
  await dialog.locator('input[type=file]').setInputFiles({ name: 'avatar.png', mimeType: 'image/png', buffer: Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aCWQAAAAASUVORK5CYII=', 'base64') });
  await dialog.getByRole('button', { name: '保存', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.locator('.user-card').filter({ hasText: 'Media Member' }).locator('img')).toHaveAttribute('src', /Users\/member\/Images\/Primary\?tag=updated-avatar/);
  expect(api.requests.some(request => request.path === '/api/portal/upload-avatar')).toBe(false);
});

test('Emby permission and playback changes send a narrow verified patch', async ({ page, api }) => {
  await page.goto('/user-management');
  await page.getByRole('button', { name: '编辑 Media Member 的 Emby 资料', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存', exact: true }) });
  await dialog.getByRole('tab', { name: '权限', exact: true }).click();
  await dialog.getByRole('checkbox', { name: '允许远程连接', exact: true }).uncheck();
  await dialog.getByRole('tab', { name: '播放设置', exact: true }).click();
  await dialog.getByLabel('字幕模式', { exact: true }).focus();
  await dialog.getByLabel('字幕模式', { exact: true }).press('Enter');
  await page.getByRole('option', { name: '只播放强制字幕', exact: true }).click();
  await dialog.getByRole('button', { name: '保存', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(api.requests.find(request => request.method === 'POST' && request.path === '/api/portal/users/member').body).toEqual({ name: 'Media Member', is_administrator: false, is_disabled: true, policy: { EnableRemoteAccess: false }, configuration: { SubtitleMode: 'OnlyForced' } });
});

test('failed detail reads do not expose default permissions or permit saving an inferred profile', async ({ page, api }) => {
  api.respond('GET', '/api/portal/users/member', { error: 'Emby 用户资料无法读取' }, 503);
  await page.goto('/user-management');
  await page.getByRole('button', { name: '编辑 Media Member 的 Emby 资料', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存', exact: true }) });
  await expect(dialog.getByText('Emby 用户资料无法读取', { exact: true })).toBeVisible();
  await expect(dialog.getByRole('button', { name: '保存', exact: true })).toBeDisabled();
  await expect(dialog.getByRole('checkbox')).toHaveCount(0);
  expect(api.requests.some(request => request.path === '/api/portal/users/member' && request.method === 'POST')).toBe(false);
});

test('new Emby user creation validates confirmation and uses the bottom-right MP action', async ({ page, api }) => {
  await page.goto('/user-management');
  const add = page.getByRole('button', { name: '添加用户', exact: true });
  const position = await add.boundingBox();
  const viewport = page.viewportSize();
  expect(viewport.width - position.x - position.width).toBeLessThanOrEqual(32);
  expect(viewport.height - position.y - position.height).toBeLessThanOrEqual(32);
  await add.click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '添加', exact: true }) });
  await dialog.getByLabel('用户名', { exact: true }).fill('新用户');
  await dialog.getByLabel('密码', { exact: true }).fill('test-secret');
  await dialog.getByLabel('确认密码', { exact: true }).fill('different');
  await dialog.getByRole('button', { name: '添加', exact: true }).click();
  await expect(dialog.getByText('两次输入的密码不一致', { exact: true })).toBeVisible();
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/portal/users')).toHaveLength(0);
  await dialog.getByLabel('确认密码', { exact: true }).fill('test-secret');
  await dialog.getByRole('button', { name: '添加', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.getByText('新用户', { exact: true })).toBeVisible();
  expect(api.requests.find(request => request.method === 'POST' && request.path === '/api/portal/users').body).toEqual({ name: '新用户', is_administrator: false, is_disabled: false, new_password: 'test-secret' });
});

test('a confirmed partial creation retries its actual Emby account instead of creating another user', async ({ page, api }) => {
  const created = { id: 'partial-user', name: '已创建用户', is_administrator: false, is_disabled: false, policy: {}, configuration: {} };
  api.respond('POST', '/api/portal/users', { created: true, id: 'partial-user', user: created, error: '密码保存未确认' }, 503);
  api.respond('GET', '/api/portal/users/partial-user', { user: created, libraries: { items: [] } });
  api.respond('POST', '/api/portal/users/partial-user', { id: 'partial-user', user: created, message: 'Emby 用户资料已保存' });
  await page.goto('/user-management');
  await page.getByRole('button', { name: '添加用户', exact: true }).click();
  const overlay = page.locator('.v-overlay--active').filter({ has: page.getByLabel('用户名', { exact: true }) });
  await overlay.getByLabel('用户名', { exact: true }).fill('已创建用户');
  await overlay.getByLabel('密码', { exact: true }).fill('test-secret');
  await overlay.getByLabel('确认密码', { exact: true }).fill('test-secret');
  await overlay.getByRole('button', { name: '添加', exact: true }).click();
  await expect(overlay.getByText(/用户已创建，请核对资料后保存/)).toBeVisible();
  await overlay.getByRole('button', { name: '保存', exact: true }).click();
  await expect(overlay).toHaveCount(0);
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/portal/users')).toHaveLength(1);
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/portal/users/partial-user')).toHaveLength(1);
});

test('closing the editor cancels an avatar draft without writing the user or the avatar', async ({ page, api }) => {
  await page.goto('/user-management');
  await page.getByRole('button', { name: '编辑 Media Member 的 Emby 资料', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存', exact: true }) });
  await dialog.locator('input[type=file]').setInputFiles({ name: 'avatar.png', mimeType: 'image/png', buffer: Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aCWQAAAAASUVORK5CYII=', 'base64') });
  await dialog.getByRole('button', { name: '关闭编辑用户', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(api.requests.filter(request => request.method !== 'GET' && request.path.includes('/api/portal/users'))).toHaveLength(0);
});

test('navbar search filters current Emby users and clears the previous page context', async ({ page, api }) => {
  await page.goto('/user-management');
  const search = page.locator('.navbar-context-search:visible');
  await expect(search).toHaveAttribute('aria-label', '搜索用户');
  await search.fill('Media Member');
  await expect(page.locator('.user-card')).toHaveCount(1);
  await expect(page.locator('.user-card').first()).toContainText('Media Member');
  await search.fill('');
  await expect(page.locator('.user-card')).toHaveCount(2);
  expect(api.requests.some(request => request.path === '/api/search_emby_library')).toBe(false);
});
