import { test, expect } from './fixtures.js';

const card = (page, title) => page.locator('.collapsible-setting-card').filter({
  has: page.getByRole('button', { name: title, exact: true }),
});

test('service settings start collapsed and retain a visible configuration status', async ({ page, api }) => {
  api.respond('GET', '/api/emby_libraries', { error: 'Emby 尚未连接' }, 503);
  await page.goto('/settings/general');
  await expect(page.getByRole('button', { name: '基础设置', exact: true })).toHaveAttribute('aria-expanded', 'true');
  const sources = card(page, '数据源与API');
  await expect(sources.getByRole('button', { name: '数据源与API', exact: true })).toHaveAttribute('aria-expanded', 'false');
  await expect(sources.locator('[data-field="tmdb_api_key"] input')).toBeHidden();
  await expect(sources.getByText(/未配置/).first()).toBeVisible();
  await page.getByRole('tab', { name: 'Emby', exact: true }).click();
  const emby = card(page, 'Emby 连接设置');
  await expect(emby.getByRole('button', { name: 'Emby 连接设置', exact: true })).toHaveAttribute('aria-expanded', 'false');
  await expect(emby.getByText(/已配置.*未验证/)).toBeVisible();
  await expect(emby.locator('[data-field="emby_api_key"] input')).toBeHidden();
});

test('expanding a service card allows saving only its edited settings', async ({ page, api }) => {
  await page.goto('/settings/general');
  const sources = card(page, '数据源与API');
  await sources.getByRole('button', { name: '数据源与API', exact: true }).click();
  await sources.locator('[data-field="tmdb_api_key"] input').fill('replacement-fixture-key');
  await sources.getByRole('button', { name: '保存数据源与API', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/config')).toBe(true);
  const payload = api.requests.find(request => request.method === 'POST' && request.path === '/api/config').body;
  expect(payload.tmdb_api_key).toBe('replacement-fixture-key');
  expect(payload).not.toHaveProperty('emby_api_key');
  expect(payload).not.toHaveProperty('ai_api_key');
  await expect(page.getByText('此卡片设置已保存！', { exact: true })).toBeVisible();
});

test('collapsing a settings card retains its unsaved draft', async ({ page, api }) => {
  await page.goto('/settings/general');
  const sources = card(page, '数据源与API');
  const toggle = sources.getByRole('button', { name: '数据源与API', exact: true });
  await toggle.click();
  const input = sources.locator('[data-field="tmdb_api_key"] input');
  await input.fill('unsaved-fixture-key');
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await expect(input).toBeHidden();
  await toggle.click();
  await expect(input).toHaveValue('unsaved-fixture-key');
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/config')).toEqual([]);
});

test('115 authorization remains visibly unverified until the server confirms a connection', async ({ page, api }) => {
  api.respond('GET', '/api/p115/status', { data: { has_token: true, auth_method: 'web' } });
  await page.goto('/settings/general');
  await page.getByRole('tab', { name: '115 网盘', exact: true }).click();
  const account = card(page, '账户与连接');
  await expect(account.getByRole('button', { name: '账户与连接', exact: true })).toHaveAttribute('aria-expanded', 'false');
  await expect(account.getByText(/已授权.*未验证/)).toBeVisible();
  await expect(account.getByText('已连接', { exact: true })).toHaveCount(0);
  api.respond('GET', '/api/p115/status', { data: { has_token: true, valid: true, auth_method: 'web' } });
  await account.getByRole('button', { name: '刷新状态', exact: true }).click();
  await expect(account.getByText('已连接', { exact: true })).toBeVisible();
  await expect(account.getByRole('button', { name: '账户与连接', exact: true })).toHaveAttribute('aria-expanded', 'false');
});

test('a 115 status read failure remains visible while its card is collapsed', async ({ page, api }) => {
  api.respond('GET', '/api/p115/status', { data: { has_token: true, valid: true } });
  await page.goto('/settings/general');
  await page.getByRole('tab', { name: '115 网盘', exact: true }).click();
  const account = card(page, '账户与连接');
  await expect(account.getByText('已连接', { exact: true })).toBeVisible();
  await expect.poll(() => api.requests.some(request => request.path === '/api/config/rename_preview')).toBe(true);
  const writes = () => api.requests.filter(request => ['POST', 'PUT', 'PATCH', 'DELETE'].includes(request.method)
    && request.path !== '/api/config/rename_preview');
  expect(writes()).toEqual([]);

  api.respond('GET', '/api/p115/status', { error: '115 状态读取暂时失败' }, 503);
  await account.getByRole('button', { name: '刷新状态', exact: true }).click();
  await expect(account.getByText('状态读取失败', { exact: true })).toBeVisible();
  await expect(account.getByRole('button', { name: '账户与连接', exact: true })).toHaveAttribute('aria-expanded', 'false');
  await expect(account.getByRole('button', { name: '刷新状态', exact: true })).toBeEnabled();
  expect(writes()).toEqual([]);
});
