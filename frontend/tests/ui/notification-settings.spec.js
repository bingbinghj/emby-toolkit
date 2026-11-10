import { test, expect } from './fixtures.js';

test('notification settings preserve Telegram fields and save only their configuration scope', async ({ page, api }) => {
  await page.goto('/settings/notifications');
  await expect(page.getByRole('tab', { name: '通知', exact: true })).toHaveAttribute('aria-selected', 'true');
  await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
  const token = page.getByLabel('Bot Token', { exact: true });
  await expect(token).toHaveAttribute('type', 'password');
  await token.fill('notification-fixture-token');
  await page.getByLabel('Chat ID', { exact: true }).fill('@notification-fixture');
  await page.getByLabel('个人 Chat ID', { exact: true }).fill('123456');
  await page.getByRole('button', { name: '确定', exact: true }).click();
  await page.getByRole('button', { name: '保存通知设置', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.path === '/api/portal/telegram-chat-id')).toBe(true);
  const config = api.requests.find(request => request.method === 'POST' && request.path === '/api/config').body;
  expect(config).toEqual({ telegram_bot_token: 'notification-fixture-token', telegram_channel_id: '@notification-fixture', telegram_notify_types: expect.any(Array), telegram_enabled: true, notification_scope: { library_new: 'admin', recognize_fail: 'admin' }, notification_time: { start: '00:00', end: '23:59' } });
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
  await expect(page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '保存通知设置', exact: true })).toBeEnabled();
});

test('a failed notification save keeps the editor available for retry', async ({ page, api }) => {
  api.respond('POST', '/api/config', { error: '模拟通知配置写入失败' }, 500);
  await page.goto('/settings/notifications');
  await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
  const channel = page.getByLabel('Chat ID', { exact: true });
  await channel.fill('@notification-draft');
  await page.getByRole('button', { name: '确定', exact: true }).click();
  await page.getByRole('button', { name: '保存通知设置', exact: true }).click();
  await expect(page.getByText('模拟通知配置写入失败', { exact: true }).first()).toBeVisible();
  await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
  await expect(channel).toHaveValue('@notification-draft');
  await page.getByRole('button', { name: '取消', exact: true }).click();
  await expect(page.getByRole('button', { name: '保存通知设置', exact: true })).toBeEnabled();
  expect(api.requests.filter(request => request.path === '/api/portal/telegram-chat-id')).toEqual([]);
});

test('leaving notification settings during the configuration save prevents a late personal write', async ({ page, api }) => {
  const saving = api.defer('POST', '/api/config');
  await page.goto('/settings/notifications');
  await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
  await page.getByLabel('个人 Chat ID', { exact: true }).fill('123456');
  await page.getByRole('button', { name: '确定', exact: true }).click();
  await page.getByRole('button', { name: '保存通知设置', exact: true }).click();
  await saving.started;
  await page.locator('.v-navigation-drawer').getByText('任务中心', { exact: true }).click();
  await expect(page).toHaveURL(/\/settings\/scheduler$/);
  saving.release({ message: '配置已保存', saved_fields: {} });
  await page.waitForTimeout(200);
  expect(api.requests.filter(request => request.path === '/api/portal/telegram-chat-id')).toEqual([]);
  await expect(page.getByText('通知设置已保存', { exact: true })).toHaveCount(0);
});


test('MP notification channel cancel discards edits, while confirming keeps only a local draft until save', async ({ page, api }) => {
  await page.goto('/settings/notifications');
  await expect.poll(() => api.requests.some(request => request.path === '/api/config')).toBe(true);
  const original = api.requests.find(request => request.path === '/api/config');
  expect(original.method).toBe('GET');
  await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
  const token = page.getByLabel('Bot Token', { exact: true });
  const previous = await token.inputValue();
  await token.fill('cancelled-token');
  await page.getByRole('button', { name: '取消', exact: true }).click();
  expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
  await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
  await expect(token).toHaveValue(previous);
  await token.fill('confirmed-token');
  await page.getByRole('button', { name: '确定', exact: true }).click();
  expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
  await page.getByRole('button', { name: '保存通知设置', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/config')).toBe(true);
  expect(api.requests.find(request => request.method === 'POST' && request.path === '/api/config').body.telegram_bot_token).toBe('confirmed-token');
});

test('MP notification scope and sending window persist, and the unused intercept option is absent', async ({ page, api }) => {
  await page.goto('/settings/notifications');
  await expect(page.getByLabel('拦截通知', { exact: true })).toHaveCount(0);
  const row = page.locator('.scope-row').filter({ hasText: '整理入库' });
  await row.getByRole('radio', { name: '所有用户', exact: true }).check();
  await page.getByLabel('开始时间', { exact: true }).fill('23:00');
  await page.getByLabel('结束时间', { exact: true }).fill('06:00');
  await page.getByRole('button', { name: '保存通知设置', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/config')).toBe(true);
  const saved = api.requests.find(request => request.method === 'POST' && request.path === '/api/config').body;
  expect(saved.notification_scope.library_new).toBe('all');
  expect(saved.notification_time).toEqual({ start: '23:00', end: '06:00' });
  expect(saved.telegram_notify_types).not.toContain('intercept_notify');
});

test('notification channel test uses the unsaved dialog token and receiver without saving configuration', async ({ page, api }) => {
  api.respond('POST', '/api/telegram/test', { success: true, message: '测试消息已发送' });
  await page.goto('/settings/notifications');
  await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
  await page.getByLabel('Bot Token', { exact: true }).fill('unsaved-token');
  await page.getByLabel('Chat ID', { exact: true }).fill('@unsaved-channel');
  await page.getByLabel('个人 Chat ID', { exact: true }).fill('');
  await page.getByRole('button', { name: '发送测试', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.path === '/api/telegram/test')).toBe(true);
  expect(api.requests.find(request => request.path === '/api/telegram/test').body).toEqual({ token: 'unsaved-token', chat_id: '@unsaved-channel' });
  expect(api.requests.filter(request => request.method === 'POST' && request.path !== '/api/telegram/test')).toEqual([]);
});


test('opening the Telegram bot queries the new dialog token through a body instead of the saved bot', async ({ page, api }) => {
  api.respond('POST', '/api/portal/telegram-bot-info', { bot_username: 'draft_bot' });
  await page.goto('/settings/notifications');
  await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
  await page.getByLabel('Bot Token', { exact: true }).fill('draft-bot-b-token');
  const popupPromise = page.waitForEvent('popup');
  await page.getByRole('button', { name: '打开机器人', exact: true }).click();
  const popup = await popupPromise;
  await popup.close();
  const request = api.requests.find(request => request.path === '/api/portal/telegram-bot-info');
  expect(request.method).toBe('POST');
  expect(request.query).toEqual({});
  expect(request.body).toEqual({ token: 'draft-bot-b-token' });
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/config')).toEqual([]);
});
