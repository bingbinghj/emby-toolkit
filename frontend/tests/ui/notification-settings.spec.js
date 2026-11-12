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
  await page.getByRole('button', { name: '保存通知渠道', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.path === '/api/portal/telegram-chat-id')).toBe(true);
  const config = api.requests.find(request => request.method === 'POST' && request.path === '/api/config').body;
  expect(config).toEqual({ telegram_bot_token: 'notification-fixture-token', telegram_channel_id: '@notification-fixture', telegram_notify_types: expect.any(Array), telegram_enabled: true, telegram_channel_name: 'Telegram', telegram_topic_id: '', telegram_users: '', telegram_admins: '', telegram_api_url: '' });
  expect(api.requests.find(request => request.path === '/api/portal/telegram-chat-id').body).toEqual({ chat_id: '123456' });
  await expect(page.getByText('通知渠道已保存', { exact: true })).toBeVisible();
});

test('unread notification configuration blocks writes and offers a retry', async ({ page, api }) => {
  api.respond('GET', '/api/config', { error: '通知配置暂不可读取' }, 503);
  await page.goto('/settings/notifications');
  await expect(page.getByText('通知配置暂不可读取', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '保存通知渠道', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: '保存通知发送范围', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: '保存通知发送时间', exact: true })).toBeDisabled();
  expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
  api.respond('GET', '/api/config', { telegram_bot_token: '', telegram_channel_id: '', telegram_notify_types: [] });
  await page.getByRole('button', { name: '重新读取通知设置', exact: true }).click();
  await expect(page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '保存通知渠道', exact: true })).toBeEnabled();
});

test('a failed notification save keeps the editor available for retry', async ({ page, api }) => {
  api.respond('POST', '/api/config', { error: '模拟通知配置写入失败' }, 500);
  await page.goto('/settings/notifications');
  await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
  const channel = page.getByLabel('Chat ID', { exact: true });
  await channel.fill('@notification-draft');
  await page.getByRole('button', { name: '确定', exact: true }).click();
  await page.getByRole('button', { name: '保存通知渠道', exact: true }).click();
  await expect(page.getByText('模拟通知配置写入失败', { exact: true }).first()).toBeVisible();
  await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
  await expect(channel).toHaveValue('@notification-draft');
  await page.getByRole('button', { name: '取消', exact: true }).click();
  await expect(page.getByRole('button', { name: '保存通知渠道', exact: true })).toBeEnabled();
  expect(api.requests.filter(request => request.path === '/api/portal/telegram-chat-id')).toEqual([]);
});

test('leaving notification settings during the configuration save prevents a late personal write', async ({ page, api }) => {
  const saving = api.defer('POST', '/api/config');
  await page.goto('/settings/notifications');
  await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
  await page.getByLabel('个人 Chat ID', { exact: true }).fill('123456');
  await page.getByRole('button', { name: '确定', exact: true }).click();
  await page.getByRole('button', { name: '保存通知渠道', exact: true }).click();
  await saving.started;
  await page.locator('.v-navigation-drawer').getByText('任务中心', { exact: true }).click();
  await expect(page).toHaveURL(/\/settings\/scheduler$/);
  saving.release({ message: '配置已保存', saved_fields: {} });
  await page.waitForTimeout(200);
  expect(api.requests.filter(request => request.path === '/api/portal/telegram-chat-id')).toEqual([]);
  await expect(page.getByText('通知渠道已保存', { exact: true })).toHaveCount(0);
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
  await page.getByRole('button', { name: '保存通知渠道', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/config')).toBe(true);
  expect(api.requests.find(request => request.method === 'POST' && request.path === '/api/config').body.telegram_bot_token).toBe('confirmed-token');
});

test('each notification card saves only its fields and preserves the other unsaved drafts', async ({ page, api }) => {
  await page.goto('/settings/notifications');
  await expect(page.getByLabel('拦截通知', { exact: true })).toHaveCount(0);
  const row = page.locator('.scope-row').filter({ hasText: '整理入库' });
  await row.getByRole('radio', { name: '所有用户', exact: true }).check();
  await page.getByLabel('开始时间', { exact: true }).fill('23:00');
  await page.getByLabel('结束时间', { exact: true }).fill('06:00');
  await page.getByRole('button', { name: '保存通知发送范围', exact: true }).click();
  await expect(page.getByText('通知发送范围已保存', { exact: true })).toBeVisible();
  const writes = () => api.requests.filter(request => request.method === 'POST' && request.path === '/api/config');
  expect(writes()[0].body).toEqual({ notification_scope: { library_new: 'all', recognize_fail: 'admin' } });
  await expect(page.getByLabel('开始时间', { exact: true })).toHaveValue('23:00');
  await page.getByRole('button', { name: '保存通知发送时间', exact: true }).click();
  await expect(page.getByText('通知发送时间已保存', { exact: true })).toBeVisible();
  expect(writes()[1].body).toEqual({ notification_time: { start: '23:00', end: '06:00' } });
  await expect(row.getByRole('radio', { name: '所有用户', exact: true })).toBeChecked();
  expect(api.requests.filter(request => request.path === '/api/portal/telegram-chat-id')).toEqual([]);
});

test('a pending notification card save blocks additional writes, and failure retains the other drafts', async ({ page, api }) => {
  const saving = api.defer('POST', '/api/config');
  await page.goto('/settings/notifications');
  await page.getByLabel('开始时间', { exact: true }).fill('21:30');
  const row = page.locator('.scope-row').filter({ hasText: '整理入库' });
  await row.getByRole('radio', { name: '所有用户', exact: true }).check();
  await page.getByRole('button', { name: '保存通知发送时间', exact: true }).click();
  await saving.started;
  await expect(page.getByRole('button', { name: '保存通知发送范围', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: '保存通知渠道', exact: true })).toBeDisabled();
  const writes = () => api.requests.filter(request => request.method === 'POST' && request.path === '/api/config');
  expect(writes()).toHaveLength(1);
  saving.release({ saved_fields: { notification_time: { start: '21:30', end: '23:59' } } });
  await expect(page.getByRole('button', { name: '保存通知发送范围', exact: true })).toBeEnabled();
  await expect(row.getByRole('radio', { name: '所有用户', exact: true })).toBeChecked();

  api.respond('POST', '/api/config', { error: '模拟范围卡片写入失败' }, 500);
  await page.getByRole('button', { name: '保存通知发送范围', exact: true }).click();
  await expect(page.getByText('模拟范围卡片写入失败', { exact: true }).first()).toBeVisible();
  await expect(row.getByRole('radio', { name: '所有用户', exact: true })).toBeChecked();
  await expect(page.getByLabel('开始时间', { exact: true })).toHaveValue('21:30');
  await expect(page.getByRole('button', { name: '保存通知发送范围', exact: true })).toBeEnabled();
  expect(writes()[1].body).toEqual({ notification_scope: { library_new: 'all', recognize_fail: 'admin' } });
});

test('notification scope and time can save when only the personal account read failed', async ({ page, api }) => {
  api.respond('GET', '/api/portal/account-info', { error: '模拟个人账号读取失败' }, 503);
  await page.goto('/settings/notifications');
  await expect(page.getByText('模拟个人账号读取失败', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '保存通知渠道', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: '保存通知发送范围', exact: true })).toBeEnabled();
  await expect(page.getByRole('button', { name: '保存通知发送时间', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: '保存通知发送时间', exact: true }).click();
  await expect(page.getByText('通知发送时间已保存', { exact: true })).toBeVisible();
  expect(api.requests.filter(request => request.path === '/api/portal/telegram-chat-id')).toEqual([]);
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
  expect(api.requests.find(request => request.path === '/api/telegram/test').body).toEqual({ token: 'unsaved-token', chat_id: '@unsaved-channel', api_url: '', topic_id: '' });
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
  expect(request.body).toEqual({ token: 'draft-bot-b-token', api_url: '' });
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/config')).toEqual([]);
});

for (const width of [390, 1440]) {
  test(`${width} MP Telegram channel uses its real configuration fields and saves the confirmed draft`, async ({ page, api }) => {
    test.setTimeout(90_000);
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/settings/notifications');
    const card = page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true });
    await expect(card.locator('.app-card-summary__image img')).toBeVisible();
    await card.click();
    const channel = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '关闭通知渠道', exact: true }) });
    await expect(channel.getByText('配置', { exact: true })).toBeVisible();
    await channel.getByLabel('名称', { exact: true }).fill('媒体通知');
    await channel.getByLabel('话题 ID', { exact: true }).fill('22');
    await channel.getByLabel('用户白名单', { exact: true }).fill('123,456');
    await channel.getByLabel('管理员白名单', { exact: true }).fill('123');
    await channel.getByLabel('代理API地址', { exact: true }).fill('https://proxy.invalid/telegram');
    await channel.getByRole('button', { name: '确定', exact: true }).click();
    await expect(card.locator('.app-card-summary__title')).toHaveText('媒体通知');
    expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
    await page.getByRole('button', { name: '保存通知渠道', exact: true }).click();
    await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/config')).toBe(true);
    expect(api.requests.find(request => request.method === 'POST' && request.path === '/api/config').body).toMatchObject({
      telegram_channel_name: '媒体通知', telegram_topic_id: '22', telegram_users: '123,456', telegram_admins: '123', telegram_api_url: 'https://proxy.invalid/telegram',
    });
  });
}

test('Telegram channel requires a name and cancelling message-type changes preserves the saved channel', async ({ page, api }) => {
  await page.goto('/settings/notifications');
  await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
  const types = page.locator('[data-field="telegram_notify_types"]');
  const originalTypes = await types.locator('.v-chip__content').allTextContents();
  await types.locator('.v-field__clearable').click();
  await page.getByLabel('名称', { exact: true }).fill(' ');
  await page.getByRole('button', { name: '确定', exact: true }).click();
  await expect(page.getByText('通知渠道名称不能为空', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '关闭通知渠道', exact: true })).toBeVisible();
  await page.getByRole('button', { name: '取消', exact: true }).click();
  await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
  await expect(page.getByLabel('名称', { exact: true })).toHaveValue('Telegram');
  await expect(types.locator('.v-chip__content')).toHaveText(originalTypes);
  expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
});
