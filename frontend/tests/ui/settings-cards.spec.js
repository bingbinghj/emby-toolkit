import { test, expect } from './fixtures.js';
import { readFileSync } from 'node:fs';

const card = (page, title) => page.locator('.setting-card').filter({ has: page.getByText(title, { exact: true }) });
const mediaServerDialog = page => page.locator('.v-dialog.v-overlay--active').filter({ has: page.getByRole('button', { name: '关闭媒体服务器配置', exact: true }) });
async function openMediaServer(page) {
  await page.getByRole('button', { name: '编辑 Emby 媒体服务器', exact: true }).click();
  return mediaServerDialog(page);
}
const storageAccountFields = ['p115_auth_method', 'p115_app_id', 'p115_api_priority', 'p115_request_interval'];
async function openStorage(page) {
  await page.getByRole('button', { name: '编辑 115 存储', exact: true }).click();
  return page.locator('.v-dialog.v-overlay--active').filter({ has: page.getByRole('button', { name: '关闭存储配置', exact: true }) });
}

const allFields = {
  system: ['delay_between_items_sec', 'media_info_retry_interval', 'media_info_retry_attempts', 'max_actors_to_process', 'max_episode_actors_to_process', 'extract_thumb', 'actor_role_add_prefix', 'remove_actors_without_avatars', 'keyword_to_tags', 'studio_to_chinese', 'emby_server_url', 'emby_api_key', 'emby_webhook_secret', 'emby_webhook_ingest_enabled', 'emby_user_id', 'emby_admin_user', 'emby_admin_pass', 'emby_api_timeout', 'ai_provider', 'ai_api_key', 'ai_model_name', 'ai_base_url', 'ai_request_timeout', 'ai_max_retries', 'ai_translation_batch_size', 'ai_translation_mode', 'log_rotation_size_mb', 'log_rotation_backup_count'],
  directory: ['p115_auth_method', 'p115_app_id', 'p115_api_priority', 'p115_request_interval', 'emby_metadata_path', 'local_strm_root', 'cd2_media_root', 'p115_fingerprint_media_root_path', 'p115_generate_mediainfo', 'p115_enable_sync_delete', 'monitor_sha1_pc_search'],
  rule: ['monitor_enabled', 'monitor_paths', 'monitor_exclude_dirs', 'monitor_exclude_refresh_delay', 'monitor_extensions', 'tmdb_image_language_preference', 'auto_repair_truehd_core_on_ingest', 'auto_fill_language_titles_on_ingest', 'auto_extract_intro_fingerprint_on_ingest', 'auto_extract_credits_on_ingest', 'auto_extract_video_thumbnails_on_ingest', 'extract_video_thumbnails_after_save', 'movie_rename_format', 'tv_rename_format'],
  search: ['tmdb_api_key', 'tmdb_api_base_url', 'douban_enable_online_api', 'api_douban_default_cooldown_seconds', 'douban_cookie'],
};

test('all 58 active settings fields are reachable through the four MP settings tabs', async ({ page, api }) => {
  test.setTimeout(60_000);
  const configuration = JSON.parse(readFileSync(new URL('./config.json', import.meta.url)));
  configuration.ai_provider = 'openai';
  configuration.ai_translate_actor_role = false;
  api.respond('GET', '/api/config', configuration);
  const reached = [];
  for (const [tab, fields] of Object.entries(allFields)) {
    await page.goto(`/setting?tab=${tab}`);
    if (tab === 'system') await page.getByRole('checkbox', { name: '翻译演员与角色', exact: true }).check();
    let server = null, storage = null;
    for (const field of fields) {
      const serverField = tab === 'system' && field.startsWith('emby_');
      const storageField = tab === 'directory' && storageAccountFields.includes(field);
      if (serverField && !server) server = await openMediaServer(page);
      if (!serverField && server) {
        await server.getByRole('button', { name: '取消媒体服务器配置', exact: true }).click();
        server = null;
      }
      if (storageField && !storage) {
        storage = await openStorage(page);
        await storage.getByRole('radio', { name: '自定义 AppID 扫码', exact: true }).check();
      }
      if (!storageField && storage) {
        await storage.getByRole('button', { name: '取消存储配置', exact: true }).click();
        storage = null;
      }
      const control = page.locator(`[data-field="${field}"]`).locator('input,textarea').first();
      await expect(control, `${tab}: ${field}`).toBeVisible();
      reached.push(field);
    }
  }
  expect(new Set(reached).size).toBe(58);
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
    let server = null, storage = null;
    for (const field of fields) {
      const serverField = group === 'system' && field.startsWith('emby_');
      const storageField = group === 'directory' && storageAccountFields.includes(field);
      if (serverField && !server) server = await openMediaServer(page);
      if (!serverField && server) {
        await server.getByRole('button', { name: '取消媒体服务器配置', exact: true }).click();
        server = null;
      }
      if (storageField && !storage) storage = await openStorage(page);
      if (!storageField && storage) {
        await storage.getByRole('button', { name: '取消存储配置', exact: true }).click();
        storage = null;
      }
      await expect(page.locator(`[data-field="${field}"]`).locator('input,textarea').first()).toBeVisible();
    }
  });
}

test('service settings omit generic status badges and retain actual read errors', async ({ page, api }) => {
  api.respond('GET', '/api/emby_libraries', { error: 'Emby 尚未连接' }, 503);
  await page.goto('/setting?tab=system');
  const emby = card(page, '媒体服务器');
  await expect(emby.getByText(/已配置.*未验证/)).toHaveCount(0);
  const server = await openMediaServer(page);
  await expect(server.locator('.v-alert')).toContainText('Emby 尚未连接');
  await expect(server.locator('[data-field="emby_api_key"] input')).toBeVisible();
  await expect(server.locator('[data-field="emby_api_key"] input')).toHaveAttribute('type', 'password');
});

test('unread media libraries cannot clear an existing exclusion list during another settings edit', async ({ page, api }) => {
  const configuration = JSON.parse(readFileSync(new URL('./config.json', import.meta.url)));
  configuration.libraries_to_ignore = ['preserved-offline-library'];
  api.respond('GET', '/api/config', configuration);
  api.respond('GET', '/api/emby_libraries', { error: '无法读取媒体库' }, 503);
  await page.goto('/setting?tab=system');
  const server = await openMediaServer(page);
  const exclusions = server.locator('.settings-field').filter({ has: page.getByText('排除媒体库', { exact: true }) });
  const exclusionInput = exclusions.locator('input:not([type="hidden"])').first();
  await expect(exclusionInput).toBeVisible();
  await expect(exclusionInput).toBeDisabled();
  await page.locator('[data-field="emby_server_url"] input').fill('http://edited-emby.invalid:8096');
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
  await page.getByRole('tab', { name: '数据源', exact: true }).click();
  await expect(input).toHaveValue('unsaved-fixture-key');
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/config')).toEqual([]);
});

test('115 settings show authorization actions without generic card connection badges', async ({ page, api }) => {
  api.respond('GET', '/api/p115/status', { data: { has_token: true, auth_method: 'web' } });
  await page.goto('/setting?tab=directory');
  const account = await openStorage(page);
  await expect(account.getByRole('button', { name: '重新登录', exact: true })).toBeVisible();
  await expect(account.locator('[data-field="p115_request_interval"] input')).toBeVisible();
  await expect(account.locator('.setting-card__status')).toHaveCount(0);
  api.respond('GET', '/api/p115/status', { data: { has_token: false, auth_method: 'web' } });
  await account.getByRole('button', { name: '刷新状态', exact: true }).click();
  await expect(account.getByRole('button', { name: '登录授权', exact: true })).toBeVisible();
});

test('a 115 status read failure remains visible and never saves credentials', async ({ page, api }) => {
  api.respond('GET', '/api/p115/status', { data: { has_token: true, valid: true } });
  await page.goto('/setting?tab=directory');
  const account = await openStorage(page);
  await expect(account.getByRole('button', { name: '重新登录', exact: true })).toBeVisible();
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


test('numeric settings use MP native spin controls and save numbers after keyboard stepping', async ({ page, api }) => {
  await page.goto('/setting?tab=system');
  const field = page.locator('[data-field="delay_between_items_sec"] input');
  await expect(field).toHaveAttribute('type', 'number');
  await expect(field).toHaveAttribute('min', '0');
  await expect(field).toHaveAttribute('step', '0.1');
  await expect(page.locator('.v-number-input__control')).toHaveCount(0);
  await field.fill('0.5');
  await field.press('ArrowUp');
  await expect(field).toHaveValue('0.6');
  await page.locator('.v-card').filter({ hasText: '基础设置' }).getByRole('button', { name: '保存基础设置', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/config')).toBe(true);
  const payload = api.requests.find(request => request.method === 'POST' && request.path === '/api/config').body;
  expect(payload.delay_between_items_sec).toBe(0.6);
  expect(payload).not.toHaveProperty('min_score_for_review');
  await expect(page.locator('[data-field="min_score_for_review"]')).toHaveCount(0);
});

test('monitor formats have built-in media candidates and preserve custom selections', async ({ page, api }) => {
  await page.goto('/setting?tab=rule');
  const field = page.locator('[data-field="monitor_extensions"] input:not([type="hidden"])');
  await field.click();
  await page.getByRole('option', { name: '.rmvb', exact: true }).click();
  await field.fill('custom-ext');
  await field.press('Enter');
  await field.press('Escape');
  await page.getByRole('button', { name: '保存实时监控', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/config')).toBe(true);
  const payload = api.requests.find(request => request.method === 'POST' && request.path === '/api/config').body;
  expect(payload.monitor_extensions).toContain('.rmvb');
  expect(payload.monitor_extensions).toContain('custom-ext');
  expect(payload.monitor_extensions).toContain('.strm');
});

test('rename examples display real recent movie and episode titles and preserve format drafts', async ({ page, api }) => {
  api.respond('POST', '/api/config/rename_preview', { movie: '最新电影 (2026)/最新电影.mkv', tv: '最新电视剧 (2025)/S02E09.mp4', movie_sample: { id: 'new-movie', title: '最新电影' }, tv_sample: { id: 'new-episode', title: '最新电视剧', season_episode: 'S02E09' }, errors: {} });
  await page.goto('/setting?tab=rule');
  await expect(page.getByText('电影示例 · 最新电影', { exact: true })).toBeVisible();
  await expect(page.getByText('电视剧示例 · 最新电视剧 S02E09', { exact: true })).toBeVisible();
  const movie = page.locator('[data-field="movie_rename_format"] textarea').first();
  await movie.fill('{{title}}.{{year}}.mkv');
  await expect.poll(() => api.requests.filter(request => request.path === '/api/config/rename_preview').some(request => request.body.movie_format === '{{title}}.{{year}}.mkv')).toBe(true);
  await expect(movie).toHaveValue('{{title}}.{{year}}.mkv');
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/config')).toEqual([]);
});

test('empty recent examples and per-type failures display truthful messages', async ({ page, api }) => {
  api.respond('POST', '/api/config/rename_preview', { movie: '', tv: '', movie_sample: null, tv_sample: null, errors: { tv: 'Emby 最近剧集读取失败' } });
  await page.goto('/setting?tab=rule');
  await expect(page.getByText('暂无最近入库的电影', { exact: true })).toBeVisible();
  await expect(page.getByText('Emby 最近剧集读取失败', { exact: true })).toBeVisible();
  await expect(page.getByText(/肖申克的救赎|凡人修仙传/)).toHaveCount(0);
});


test('cover style numeric settings keep native stepping and scoped numeric saves', async ({ page, api }) => {
  await page.goto('/settings/cover-generator');
  await page.getByRole('tab', { name: '单图风格设置', exact: true }).click();
  const font = page.getByLabel('中文字体大小比例', { exact: true }).filter({ visible: true }).first();
  await expect(font).toHaveAttribute('type', 'number');
  await font.fill('1.0');
  await font.press('ArrowUp');
  await expect(font).toHaveValue('1.1');
  await page.locator('.v-card').filter({ has: page.getByText('封面样式与标题', { exact: true }) }).getByRole('button', { name: '保存', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.method === 'PATCH' && request.path === '/api/config/cover_generator')).toBe(true);
  const single = api.requests.find(request => request.method === 'PATCH' && request.path === '/api/config/cover_generator').body;
  expect(single.zh_font_size).toBe(1.1);
  expect(single).not.toHaveProperty('enabled');
  expect(single).not.toHaveProperty('multi_1_blur');
  await page.getByRole('tab', { name: '多图风格设置', exact: true }).click();
  const blur = page.getByLabel('背景模糊程度', { exact: true }).filter({ visible: true }).first();
  await page.getByRole('checkbox', { name: '启用模糊背景', exact: true }).uncheck();
  await expect(blur).toBeDisabled();
  await page.getByRole('checkbox', { name: '启用模糊背景', exact: true }).check();
  await expect(blur).toBeEnabled();
  await blur.fill('25');
  await blur.press('ArrowUp');
  await expect(blur).toHaveValue('26');
  await expect(page.locator('.v-number-input__control')).toHaveCount(0);
});
