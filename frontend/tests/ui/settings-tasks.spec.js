import { test, expect } from './fixtures.js';

const cards = [
  ['规则', '保存实时监控', 'monitor_exclude_refresh_delay', '35', 35],
  ['搜索 & 下载', '保存数据源与API', 'tmdb_api_key', 'new-fixture-tmdb-key', 'new-fixture-tmdb-key'],
  ['存储 & 目录', '保存存储', 'p115_request_interval', '1.2', 1.2],
  ['存储 & 目录', '保存目录', 'local_strm_root', '/config/ui-strm', '/config/ui-strm'],
  ['系统', '保存媒体服务器', 'emby_public_url', 'http://public.invalid:8096', 'http://public.invalid:8096'],
  ['系统', '保存AI 增强', 'ai_base_url', 'https://ai.invalid/v1', 'https://ai.invalid/v1'],
  ['系统', '保存日志配置', 'log_rotation_size_mb', '32', 32],
  ['规则', '保存整理与刮削', 'movie_rename_format', '{{title}} ({{year}})/{{title}}', '{{title}} ({{year}})/{{title}}'],
];

for (const [tab, button, field, value, savedValue] of cards) {
  test(`${button} submits its edited field through the ETK configuration API`, async ({ page, api }) => {
    await page.goto('/settings/general');
    await page.getByRole('tab', { name: tab, exact: true }).click();
    await page.locator(`[data-field="${field}"]`).locator('input,textarea').first().fill(value);
    await page.getByRole('button', { name: button, exact: true }).click();
    await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/config')).toBe(true);
    const payload = api.requests.find(request => request.method === 'POST' && request.path === '/api/config').body;
    expect(payload[field]).toBe(savedValue);
    await expect(page.getByText('此卡片设置已保存！', { exact: true })).toBeVisible();
  });
}

test('ingest card switch saves its boolean value without altering another card', async ({ page, api }) => {
  await page.goto('/settings/general');
  await page.getByRole('tab', { name: '规则', exact: true }).click();
  const enabled = page.locator('[data-field="auto_fill_language_titles_on_ingest"] input');
  const previous = await enabled.isChecked();
  await enabled.setChecked(!previous);
  await page.getByRole('button', { name: '保存入库自动处理', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/config')).toBe(true);
  const payload = api.requests.find(request => request.method === 'POST' && request.path === '/api/config').body;
  expect(payload.auto_fill_language_titles_on_ingest).toBe(!previous);
  expect(payload).not.toHaveProperty('log_rotation_size_mb');
  await expect(page.getByText('此卡片设置已保存！', { exact: true })).toBeVisible();
});

test('saving a settings card submits only its fields', async ({ page, api }) => {
  await page.goto('/settings/general');
  await page.locator('[data-field="delay_between_items_sec"] input').fill('1.5');
  await page.getByRole('button', { name: '保存基础设置', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/config')).toBe(true);
  const payload = api.requests.find(request => request.method === 'POST' && request.path === '/api/config').body;
  expect(payload.delay_between_items_sec).toBe(1.5);
  expect(payload).not.toHaveProperty('emby_api_key');
  expect(payload).not.toHaveProperty('telegram_bot_token');
  expect(payload).not.toHaveProperty('monitor_paths');
  await expect(page.getByText('此卡片设置已保存！', { exact: true })).toBeVisible();
});

test('settings save failure preserves the edited field and exposes backend reason', async ({ page, api }) => {
  api.respond('POST', '/api/config', { error: '测试配置写入失败' }, 500);
  await page.goto('/settings/general');
  const delay = page.locator('[data-field="delay_between_items_sec"] input');
  await delay.fill('2.5');
  await page.getByRole('button', { name: '保存基础设置', exact: true }).click();
  await expect(page.getByText('测试配置写入失败', { exact: true }).first()).toBeVisible();
  await expect(delay).toHaveValue('2.5');
});

test('cover basic settings retain style fields and submit current switch values', async ({ page, api }) => {
  await page.goto('/settings/cover-generator');
  await page.getByLabel('启用封面生成', { exact: true }).check();
  await page.getByRole('button', { name: '保存基础设置', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/config/cover_generator')).toBe(true);
  const payload = api.requests.find(request => request.method === 'POST' && request.path === '/api/config/cover_generator').body;
  expect(payload.enabled).toBe(true);
  expect(payload.cover_style).toBe('single_1');
  await expect(page.getByText('配置已成功保存！', { exact: true })).toBeVisible();
});

test('202 fingerprint task waits for terminal state and displays its warning details', async ({ page, api }) => {
  api.queueTask('POST', '/api/tasks/fingerprint-scan', {
    message: '扫描已完成，有未确认项', warnings: 1, counts: { consistent: 0, repaired: 0, unknown: 1 },
    warning_details: ['101：文件身份尚未确认'], failures: [],
  });
  await page.goto('/settings/scheduler');
  const scan = page.getByRole('button', { name: '扫描全库并补齐', exact: true });
  await scan.click();
  await expect.poll(() => api.requests.some(request => request.path === '/api/tasks/result/ui-background-task')).toBe(true);
  await expect(page.getByText('101：文件身份尚未确认', { exact: true }).first()).toBeVisible();
  await expect(scan).toBeEnabled();
  await page.getByRole('button', { name: '通知中心', exact: true }).click();
  await expect(page.getByText('已完成，有提醒', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('扫描已完成，有未确认项', { exact: true }).first()).toBeVisible();
  expect(api.requests.filter(request => request.path === '/api/tasks/result/ui-background-task').length).toBeGreaterThanOrEqual(2);
});
