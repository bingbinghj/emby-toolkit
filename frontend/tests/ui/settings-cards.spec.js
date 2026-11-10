import { test, expect } from './fixtures.js';
import { readFileSync } from 'node:fs';

const card = (page, title) => page.locator('.setting-card').filter({ has: page.getByText(title, { exact: true }) });

const allFields = {
  system: ['delay_between_items_sec', 'media_info_retry_interval', 'media_info_retry_attempts', 'subtitle_chinese_default_language', 'min_score_for_review', 'max_actors_to_process', 'max_episode_actors_to_process', 'extract_thumb', 'actor_role_add_prefix', 'remove_actors_without_avatars', 'keyword_to_tags', 'studio_to_chinese', 'emby_server_url', 'emby_public_url', 'emby_api_key', 'emby_webhook_secret', 'emby_webhook_ingest_enabled', 'emby_user_id', 'emby_admin_user', 'emby_admin_pass', 'emby_api_timeout', 'ai_provider', 'ai_api_key', 'ai_model_name', 'ai_base_url', 'ai_request_timeout', 'ai_max_retries', 'ai_translation_batch_size', 'ai_translation_mode', 'log_rotation_size_mb', 'log_rotation_backup_count'],
  directory: ['p115_auth_method', 'p115_app_id', 'p115_api_priority', 'p115_request_interval', 'emby_metadata_path', 'local_strm_root', 'cd2_media_root', 'p115_fingerprint_media_root_path', 'p115_generate_mediainfo', 'p115_enable_sync_delete'],
  rule: ['monitor_enabled', 'monitor_paths', 'monitor_exclude_dirs', 'monitor_exclude_refresh_delay', 'monitor_extensions', 'tmdb_image_language_preference', 'monitor_sha1_pc_search', 'auto_repair_truehd_core_on_ingest', 'auto_fill_language_titles_on_ingest', 'auto_extract_intro_fingerprint_on_ingest', 'auto_extract_video_thumbnails_on_ingest', 'extract_video_thumbnails_after_save', 'movie_rename_format', 'tv_rename_format'],
  search: ['tmdb_api_key', 'tmdb_api_base_url', 'douban_enable_online_api', 'api_douban_default_cooldown_seconds', 'douban_cookie'],
};

test('all 60 original settings fields are reachable through the four MP settings tabs', async ({ page, api }) => {
  test.setTimeout(60_000);
  const configuration = JSON.parse(readFileSync(new URL('./config.json', import.meta.url)));
  configuration.ai_provider = 'openai';
  configuration.ai_translate_actor_role = false;
  api.respond('GET', '/api/config', configuration);
  const reached = [];
  for (const [tab, fields] of Object.entries(allFields)) {
    await page.goto(`/setting?tab=${tab}`);
    if (tab === 'system') await page.getByRole('checkbox', { name: '翻译演员与角色', exact: true }).check();
    if (tab === 'directory') await page.getByRole('radio', { name: '自定义 AppID 扫码', exact: true }).check();
    for (const field of fields) {
      const control = page.locator(`[data-field="${field}"]`).locator('input,textarea').first();
      await expect(control, `${tab}: ${field}`).toBeVisible();
      reached.push(field);
    }
  }
  expect(new Set(reached).size).toBe(60);
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/config')).toEqual([]);
});

const groups = [
  ['system', ['基础设置', '媒体服务器', 'AI 增强', '日志配置', '数据管理'], ['delay_between_items_sec', 'emby_api_key', 'ai_api_key', 'log_rotation_size_mb']],
  ['directory', ['存储', '目录'], ['p115_request_interval', 'local_strm_root', 'emby_metadata_path']],
  ['rule', ['实时监控', '入库自动处理', '整理 & 刮削'], ['monitor_exclude_refresh_delay', 'auto_fill_language_titles_on_ingest', 'movie_rename_format']],
  ['search', ['数据源与API'], ['tmdb_api_key', 'douban_enable_online_api']],
];

for (const [group, titles, fields] of groups) {
  test(`${group} settings expose their fields without any collapsed cards`, async ({ page }) => {
    await page.goto(`/setting?tab=${group}`);
    for (const title of titles) {
      await expect(card(page, title)).toBeVisible();
      await expect(card(page, title).locator('.setting-card__content')).toBeVisible();
    }
    for (const field of fields) await expect(page.locator(`[data-field="${field}"]`).locator('input,textarea').first()).toBeVisible();
  });
}

test('service configuration remains visibly unverified while its settings are fully expanded', async ({ page, api }) => {
  api.respond('GET', '/api/emby_libraries', { error: 'Emby 尚未连接' }, 503);
  await page.goto('/setting?tab=system');
  const emby = card(page, '媒体服务器');
  await expect(emby.getByText(/已配置.*未验证/)).toBeVisible();
  await expect(emby.locator('[data-field="emby_api_key"] input')).toBeVisible();
  await expect(emby.locator('[data-field="emby_api_key"] input')).toHaveAttribute('type', 'password');
});

test('unread media libraries cannot clear an existing exclusion list during another settings edit', async ({ page, api }) => {
  const configuration = JSON.parse(readFileSync(new URL('./config.json', import.meta.url)));
  configuration.libraries_to_ignore = ['preserved-offline-library'];
  api.respond('GET', '/api/config', configuration);
  api.respond('GET', '/api/emby_libraries', { error: '无法读取媒体库' }, 503);
  await page.goto('/setting?tab=system');
  const exclusions = card(page, '媒体服务器').locator('.settings-field').filter({ has: page.getByText('排除媒体库', { exact: true }) });
  const exclusionInput = exclusions.locator('input:not([type="hidden"])').first();
  await expect(exclusionInput).toBeVisible();
  await expect(exclusionInput).toBeDisabled();
  await page.locator('[data-field="emby_public_url"] input').fill('http://public.invalid:8096');
  await page.getByRole('button', { name: '保存媒体服务器', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/config')).toBe(true);
  const payload = api.requests.find(request => request.method === 'POST' && request.path === '/api/config').body;
  expect(payload.libraries_to_ignore).toEqual(['preserved-offline-library']);
  expect(payload).not.toHaveProperty('emby_metadata_path');
});

test('an expanded service card saves only its edited settings', async ({ page, api }) => {
  await page.goto('/setting?tab=search');
  const sources = card(page, '数据源与API');
  await sources.locator('[data-field="tmdb_api_key"] input').fill('replacement-fixture-key');
  await sources.getByRole('button', { name: '保存数据源与API', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/config')).toBe(true);
  const payload = api.requests.find(request => request.method === 'POST' && request.path === '/api/config').body;
  expect(payload.tmdb_api_key).toBe('replacement-fixture-key');
  expect(payload).not.toHaveProperty('emby_api_key');
  expect(payload).not.toHaveProperty('ai_api_key');
  await expect(page.getByText('此卡片设置已保存！', { exact: true })).toBeVisible();
});

test('all six AI checkboxes read and save their boolean values through the Vuetify model', async ({ page, api }) => {
  const switches = [
    ['ai_translate_actor_role', '翻译演员与角色'],
    ['ai_translate_on_sync', '同步媒体数据时翻译'],
    ['ai_translate_title', '翻译片名'],
    ['ai_translate_overview', '翻译简介'],
    ['ai_translate_episode_overview', '翻译分集简介'],
    ['ai_joke_fallback', '无简介小笑话占位'],
  ];
  const configuration = JSON.parse(readFileSync(new URL('./config.json', import.meta.url)));
  for (const [index, [field]] of switches.entries()) configuration[field] = index % 2 === 0;
  api.respond('GET', '/api/config', configuration);
  await page.goto('/setting?tab=system');
  for (const [field, label] of switches) {
    const checkbox = page.getByRole('checkbox', { name: label, exact: true });
    if (configuration[field]) await expect(checkbox).toBeChecked();
    else await expect(checkbox).not.toBeChecked();
    await checkbox.check();
  }
  const writes = () => api.requests.filter(request => request.method === 'POST' && request.path === '/api/config');
  await page.getByRole('button', { name: '保存AI 增强', exact: true }).click();
  await expect.poll(() => writes().length).toBe(1);
  for (const [field] of switches) expect(writes()[0].body[field]).toBe(true);
  expect(writes()[0].body).not.toHaveProperty('emby_api_key');
  expect(writes()[0].body).not.toHaveProperty('delay_between_items_sec');
  await expect(page.getByText('此卡片设置已保存！', { exact: true })).toBeVisible();

  for (const [, label] of switches) await page.getByRole('checkbox', { name: label, exact: true }).uncheck();
  await page.getByRole('button', { name: '保存AI 增强', exact: true }).click();
  await expect.poll(() => writes().length).toBe(2);
  for (const [field] of switches) expect(writes()[1].body[field]).toBe(false);
});

test('switching settings tabs retains an unsaved draft', async ({ page, api }) => {
  await page.goto('/setting?tab=search');
  const input = page.locator('[data-field="tmdb_api_key"] input');
  await input.fill('unsaved-fixture-key');
  await page.getByRole('tab', { name: '系统', exact: true }).click();
  await expect(input).toBeHidden();
  await page.getByRole('tab', { name: '搜索 & 下载', exact: true }).click();
  await expect(input).toHaveValue('unsaved-fixture-key');
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/config')).toEqual([]);
});

test('115 authorization remains unverified until the server confirms a connection', async ({ page, api }) => {
  api.respond('GET', '/api/p115/status', { data: { has_token: true, auth_method: 'web' } });
  await page.goto('/setting?tab=directory');
  const account = card(page, '存储');
  await expect(account.getByText(/已授权.*未验证/)).toBeVisible();
  await expect(account.locator('[data-field="p115_request_interval"] input')).toBeVisible();
  await expect(account.getByText('已连接', { exact: true })).toHaveCount(0);
  api.respond('GET', '/api/p115/status', { data: { has_token: true, valid: true, auth_method: 'web' } });
  await account.getByRole('button', { name: '刷新状态', exact: true }).click();
  await expect(account.getByText('已连接', { exact: true })).toBeVisible();
});

test('a 115 status read failure remains visible and never saves credentials', async ({ page, api }) => {
  api.respond('GET', '/api/p115/status', { data: { has_token: true, valid: true } });
  await page.goto('/setting?tab=directory');
  const account = card(page, '存储');
  await expect(account.getByText('已连接', { exact: true })).toBeVisible();
  await expect.poll(() => api.requests.some(request => request.path === '/api/config/rename_preview')).toBe(true);
  const writes = () => api.requests.filter(request => ['POST', 'PUT', 'PATCH', 'DELETE'].includes(request.method) && request.path !== '/api/config/rename_preview');
  expect(writes()).toEqual([]);
  api.respond('GET', '/api/p115/status', { error: '115 状态读取暂时失败' }, 503);
  await account.getByRole('button', { name: '刷新状态', exact: true }).click();
  await expect(account.getByText('状态读取失败', { exact: true })).toBeVisible();
  await expect(account.getByRole('button', { name: '刷新状态', exact: true })).toBeEnabled();
  await expect(account.locator('[data-field="p115_request_interval"] input')).toBeVisible();
  expect(writes()).toEqual([]);
});
