import { readFileSync } from 'node:fs';
import { test as base, expect } from '@playwright/test';

const config = JSON.parse(readFileSync(new URL('./config.json', import.meta.url)));
const cover = JSON.parse(readFileSync(new URL('./cover-config.json', import.meta.url)));
const clone = value => structuredClone(value);
const image = '<svg xmlns="http://www.w3.org/2000/svg" width="120" height="180"><rect width="120" height="180" fill="#5277ab"/></svg>';

export const languageMapping = [
  { label: '国语', value: 'chi', aliases: ['chi', 'zho', 'zh', 'cmn'] },
  { label: '粤语', value: 'yue', aliases: ['yue'] },
  { label: '英语', value: 'eng', aliases: ['eng', 'en'] },
  { label: '日语', value: 'jpn', aliases: ['jpn', 'ja'] },
  { label: '简体中文', value: 'chs', aliases: ['chs'] },
  { label: '繁体中文', value: 'cht', aliases: ['cht'] },
];

export const mediaDetails = {
  item_id: '101', item_type: 'Movie', item_name: '浏览器验证电影', production_year: 2026,
  original_score: 8.2, review_reason: '', search_links: { baidu: 'https://example.invalid/search' },
  current_emby_cast: [
    { name: 'Actor One', role: 'Captain', tmdbId: '10', embyPersonId: 'person-10', originalName: 'Actor One', imageUrl: '' },
    { name: 'Actor Two', role: 'Doctor', tmdbId: '20', embyPersonId: 'person-20', originalName: 'Actor Two', imageUrl: '' },
  ],
};
export const mediaInfo = {
  sha1: 'a'.repeat(40), fingerprint_pending: false,
  media_path: '/media/Browser.Movie.2026.mkv', mediainfo_path: '/config/Browser.Movie.2026-mediainfo.json',
  subtitle_chinese_default_language: 'chs', iso_language_options: [],
  versions: [{ emby_id: '101', file_name: 'Browser.Movie.2026.mkv', display_name: '1080p MKV', path: '/media/Browser.Movie.2026.mkv' }],
  mediainfo: [{ MediaSourceInfo: { Container: 'mkv', MediaStreams: [
    { Type: 'Video', Index: 0, Codec: 'h264', Width: 1920, Height: 1080, Title: '', IsDefault: true },
    { Type: 'Audio', Index: 1, Codec: 'aac', Language: 'eng', Title: 'Original English', Channels: 2, BitRate: 192000, IsDefault: true },
    { Type: 'Subtitle', Index: 2, Codec: 'srt', Language: 'chs', Title: '简体中文', IsDefault: false },
  ] } }],
};
const idle = { is_running: false, state: 'idle', current_action: '无', progress: 0, progress_known: false, message: '等待任务', history: [], logs: [] };
const library = { id: 'movies', name: '电影', collection_type: 'movies', item_count: 1, image_tag: 'fixture' };
const tasks = [{ key: 'clear-mediainfo-cache', name: '清理媒体信息缓存', category: 'system-maintenance', category_name: '系统维护', description: '只清理 ETK 的媒体信息缓存', help: '隔离测试任务' }];

export const test = base.extend({
  authState: ['logged_in', { option: true }],
  admin: [true, { option: true }],
  api: [async ({ page, authState, admin }, use) => {
    const handlers = new Map(), requests = [], unexpected = [], errors = [];
    let auth = authState, currentTask = clone(idle), storedConfig = clone(config), storedCover = clone(cover), menu = [];
    const user = { id: 'ui-test-user', name: 'UI Test', username: 'UI Test', is_admin: admin, user_type: 'emby_user' };
    const key = (method, path) => `${method.toUpperCase()} ${path}`;
    const respond = (method, path, body, status = 200) => handlers.set(key(method, path), { body, status });
    respond('GET', '/api/auth/check_status', () => ({ status: auth, ...(auth === 'logged_in' ? { user } : {}) }));
    respond('POST', '/api/auth/login', () => { auth = 'logged_in'; return { user }; });
    respond('POST', '/api/auth/logout', () => { auth = 'login_required'; return { status: 'ok' }; });
    respond('POST', '/api/auth/setup', () => { auth = 'login_required'; return { status: 'ok' }; });
    respond('GET', '/api/config', () => storedConfig);
    respond('POST', '/api/config', request => { Object.assign(storedConfig, request.body); return { message: '配置已保存', saved_fields: request.body }; });
    respond('POST', '/api/config/rename_preview', { movie: '浏览器验证电影 (2026)/浏览器验证电影.mkv', tv: '浏览器验证剧集 (2026)/Season 01/浏览器验证剧集.S01E01.mkv' });
    respond('GET', '/api/status', () => currentTask);
    respond('GET', '/api/system/menu_config', () => menu);
    respond('POST', '/api/system/menu_config', request => { menu = clone(request.body); return { message: '导航已保存' }; });
    respond('POST', '/api/system/menu_config/reset', () => { menu = []; return { message: '导航已重置' }; });
    respond('GET', '/api/portal/account-info', { name: 'UI Test', status: 'active', registration_date: '2026-01-01T00:00:00Z', expiration_date: null, telegram_chat_id: '', telegram_channel_id: '', profile_image_tag: null });
    respond('GET', '/api/portal/users', { users: [
      { id: 'ui-test-user', name: 'UI Test', is_administrator: true, is_disabled: false, notes: '', telegram_chat_id: '', profile_image_tag: null },
      { id: 'member', name: 'Media Member', is_administrator: false, is_disabled: true, notes: '原备注', telegram_chat_id: '123', profile_image_tag: null },
    ] });
    respond('POST', '/api/portal/users/member', { message: '用户资料已保存', id: 'member' });
    respond('POST', '/api/portal/telegram-chat-id', request => ({ telegram_chat_id: request.body.telegram_chat_id, message: '通知设置已保存' }));
    respond('GET', '/api/portal/telegram-bot-info', { username: 'fixture_bot' });
    respond('GET', '/api/media_center/libraries', { libraries: [library] });
    respond('GET', '/api/dashboard/media', {
      statistics: { movie_count: { count: 1 }, tv_count: { count: 0 }, episode_count: { count: 0 }, music_count: { count: 0 } },
      recent: [{ id: '101', name: mediaDetails.item_name, item_type: 'Movie', production_year: 2026, image_tag: 'fixture' }],
      libraries: [library], errors: {},
    });
    respond('GET', '/api/media_center/libraries/movies/items', { library, items: [{ Id: '101', Name: mediaDetails.item_name, Type: 'Movie', ProductionYear: 2026, ImageTags: { Primary: 'fixture' } }], total: 1, start_index: 0, anchor_index: 0 });
    respond('GET', '/api/review_items', { items: [{ item_id: '101', item_name: mediaDetails.item_name, item_type: 'Movie', failed_at: '2026-11-09T12:00:00Z', reason: '人工复核', original_score: 8.2 }], total_items: 1 });
    respond('GET', '/api/search_emby_library', { items: [{ item_id: '101', item_name: mediaDetails.item_name, item_type: 'Movie' }], total_items: 1 });
    respond('GET', '/api/media_for_editing/101', mediaDetails);
    respond('GET', '/api/media_info/edit/101', mediaInfo);
    respond('GET', '/api/media_info/recovery', { pending: false, records: [] });
    respond('POST', '/api/media_info/edit/101', { message: '媒体信息已更新', warnings: 0, metadata_refresh: { state: 'verified', verified_ids: ['101'] } });
    respond('POST', '/api/update_media_cast_sa/101', { message: '演员已保存' });
    respond('POST', '/api/actions/translate_cast_sa', mediaDetails.current_emby_cast);
    respond('GET', '/api/tasks/available', tasks);
    respond('GET', '/api/tasks/fingerprint-scan', { progress: null, report_available: false });
    respond('GET', '/api/p115/status', { data: { is_logged_in: false, cookie_status: '未登录', auth_method: 'web' } });
    respond('GET', '/api/emby_libraries', [{ Id: 'movies', Name: '电影', CollectionType: 'movies' }]);
    respond('GET', '/api/config/cover_generator', () => storedCover);
    respond('POST', '/api/config/cover_generator', request => { storedCover = clone(request.body); return { message: '封面配置已保存' }; });
    respond('POST', '/api/config/cover_generator/preview', request => ({ image: request.body.base_image }));
    respond('GET', '/api/config/cover_generator/libraries', [{ label: '电影', value: 'movies' }]);
    for (const name of ['keyword_mapping', 'studio_mapping', 'country_mapping', 'rating_priority', 'unified_ratings']) respond('GET', `/api/mappings/config/${name}`, []);
    respond('GET', '/api/mappings/config/language_mapping', languageMapping);
    respond('GET', '/api/mappings/config/rating_mapping', { US: [{ original: 'PG', label: '全年龄', emby_value: 5 }, { original: 'PG-13', label: '青少年', emby_value: 8 }] });
    respond('GET', '/api/logs/list', { files: [{ name: 'app.log', size: 32, modified: '2026-11-09T12:00:00Z' }] });

    page.on('pageerror', error => errors.push(error.message));
    await page.route('**/image_proxy/**', route => route.fulfill({ contentType: 'image/svg+xml', body: image }));
    await page.route('https://**', route => route.abort());
    await page.route('**/api/**', async route => {
      const source = route.request(), url = new URL(source.url());
      let body;
      try { body = source.postDataJSON(); } catch { body = source.postData(); }
      const request = { method: source.method(), path: url.pathname, query: Object.fromEntries(url.searchParams), body };
      requests.push(request);
      const handler = handlers.get(key(request.method, request.path));
      if (!handler) {
        unexpected.push(`${request.method} ${request.path}`);
        await route.fulfill({ status: 501, json: { error: `没有定义测试接口：${request.method} ${request.path}` } });
        return;
      }
      const payload = typeof handler.body === 'function' ? await handler.body(request) : clone(handler.body);
      await route.fulfill({ status: handler.status, json: payload });
    });

    const api = {
      respond, requests, unexpected,
      setAuth: status => { auth = status; },
      setTask: task => { currentTask = clone(task); },
      defer(method, path) {
        let release, received;
        const started = new Promise(resolve => { received = resolve; });
        const completion = new Promise(resolve => { release = resolve; });
        respond(method, path, async request => { received(request); return completion; });
        return { started, release };
      },
      queueTask(method, path, result, { id = 'ui-background-task', status = 207, polls = 1, state = 'completed_with_warnings' } = {}) {
        let remaining = polls;
        respond(method, path, () => {
          currentTask = { ...clone(idle), task_id: id, is_running: true, state: 'running', current_action: '浏览器后台任务', last_action: '浏览器后台任务', progress: 25, progress_known: true, message: '处理中' };
          return { task_id: id, message: '任务已提交' };
        }, 202);
        respond('GET', `/api/tasks/result/${id}`, () => {
          if (remaining-- > 0) return currentTask;
          currentTask = { ...currentTask, is_running: false, state, progress: 100, message: result.message || '已完成，有提醒', result_status: status, result };
          return currentTask;
        });
      },
    };
    await use(api);
    expect(unexpected, '所有 ETK API 都必须定义真实响应契约').toEqual([]);
    expect(errors, '页面不得产生未处理的 JavaScript 异常').toEqual([]);
  }, { auto: true }],
});

export { expect };
