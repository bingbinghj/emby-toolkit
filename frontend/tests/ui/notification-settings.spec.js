import { test, expect } from './fixtures.js';

test('notification settings preserve Telegram fields and save only their configuration scope', async ({ page, api }) => {
  await page.goto('/settings/notifications');
  await expect(page.getByRole('heading', { name: '通知设置', exact: true })).toBeVisible();
  const token = page.getByLabel('Telegram Bot Token', { exact: true });
  await expect(token).toHaveAttribute('type', 'password');
  await token.fill('notification-fixture-token');
  await page.getByLabel('全局通知频道', { exact: true }).fill('@notification-fixture');
  await page.getByLabel('Telegram 用户ID（私聊ID）', { exact: true }).fill('123456');
  await page.getByRole('button', { name: '保存通知设置', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.path === '/api/portal/telegram-chat-id')).toBe(true);
  const config = api.requests.find(request => request.method === 'POST' && request.path === '/api/config').body;
  expect(config).toEqual({ telegram_bot_token: 'notification-fixture-token', telegram_channel_id: '@notification-fixture', telegram_notify_types: expect.any(Array) });
  expect(api.requests.find(request => request.path === '/api/portal/telegram-chat-id').body).toEqual({ chat_id: '123456' });
  await expect(page.getByText('通知设置已保存', { exact: true })).toBeVisible();
});

test('unread notification configuration blocks writes and offers a retry', async ({ page, api }) => {
  api.respond('GET', '/api/config', { error: '通知配置暂不可读取' }, 503);
  await page.goto('/settings/notifications');
  await expect(page.getByText('通知配置暂不可读取', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '保存通知设置', exact: true })).toBeDisabled();
  expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
  api.respond('GET', '/api/config', { telegram_bot_token: '', telegram_channel_id: '', telegram_notify_types: [] });
  await page.getByRole('button', { name: '重新读取通知设置', exact: true }).click();
  await expect(page.getByLabel('Telegram Bot Token', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '保存通知设置', exact: true })).toBeEnabled();
});

test('a failed notification save keeps the editor available for retry', async ({ page, api }) => {
  api.respond('POST', '/api/config', { error: '模拟通知配置写入失败' }, 500);
  await page.goto('/settings/notifications');
  const channel = page.getByLabel('全局通知频道', { exact: true });
  await channel.fill('@notification-draft');
  await page.getByRole('button', { name: '保存通知设置', exact: true }).click();
  await expect(page.getByText('模拟通知配置写入失败', { exact: true }).first()).toBeVisible();
  await expect(channel).toHaveValue('@notification-draft');
  await expect(page.getByRole('button', { name: '保存通知设置', exact: true })).toBeEnabled();
  expect(api.requests.filter(request => request.path === '/api/portal/telegram-chat-id')).toEqual([]);
});

test('leaving notification settings during the configuration save prevents a late personal write', async ({ page, api }) => {
  const saving = api.defer('POST', '/api/config');
  await page.goto('/settings/notifications');
  await page.getByLabel('Telegram 用户ID（私聊ID）', { exact: true }).fill('123456');
  await page.getByRole('button', { name: '保存通知设置', exact: true }).click();
  await saving.started;
  await page.locator('.v-navigation-drawer').getByText('任务中心', { exact: true }).click();
  await expect(page).toHaveURL(/\/settings\/scheduler$/);
  saving.release({ message: '配置已保存', saved_fields: {} });
  await page.waitForTimeout(200);
  expect(api.requests.filter(request => request.path === '/api/portal/telegram-chat-id')).toEqual([]);
  await expect(page.getByText('通知设置已保存', { exact: true })).toHaveCount(0);
});
