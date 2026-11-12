import test from 'node:test';
import assert from 'node:assert/strict';
import { dashboardModules, normalizeDashboardLayout, normalizeDashboardProfile, dashboardLayoutProfile, dashboardColumns, storageFrom115, formatDashboardBytes, embyDashboardImage, formatDashboardCount, dashboardSnapshot, projectDashboardItems, projectDashboardStatistics, dashboardMonthlyCount } from '../src/utils/dashboard.js';

const response = rt_space_info => ({ data: { valid: true, user_info: { rt_space_info } } });

test('115 byte fields produce quota, usage, available space and the actual ratio', () => {
  const space = storageFrom115(response({ all_total: { size: '1099511627776' }, all_use: { size: 549755813888 }, all_remain: { size: 549755813888 } }));
  assert.equal(space.totalText, '1 TB');
  assert.equal(space.usedText, '512 GB');
  assert.equal(space.availableText, '512 GB');
  assert.equal(space.percent, 50);
});

test('a missing capacity can be derived from the other two actual 115 capacities', () => {
  assert.equal(storageFrom115(response({ all_total: { size: 100 }, all_remain: { size: 40 } })).used, 60);
  assert.equal(storageFrom115(response({ all_use: { size: 60 }, all_remain: { size: 40 } })).total, 100);
  assert.equal(storageFrom115(response({ all_total: { size: 100 }, all_use: { size: 60 } })).available, 40);
});

test('partial formatted 115 space remains partial instead of inventing totals or zero usage', () => {
  const space = storageFrom115(response({ all_remain: { size_format: '3 TB' } }));
  assert.equal(space.total, null);
  assert.equal(space.used, null);
  assert.equal(space.percent, null);
  assert.equal(space.availableText, '3 TB');
  assert.equal(space.totalText, '未知');
});

test('unavailable and contradictory 115 results cannot masquerade as empty storage', () => {
  for (const value of [undefined, { data: { valid: false, msg: '凭证失效' } }, response({}), response({ all_total: { size: 10 }, all_use: { size: 20 } })]) {
    assert.throws(() => storageFrom115(value));
  }
  assert.equal(formatDashboardBytes(0), '0 B');
  assert.equal(formatDashboardBytes(0.25), '0.25 B');
  assert.equal(formatDashboardBytes(NaN), '未知');
});

test('OpenAPI user capacity fields are read without treating the server disk as 115 storage', () => {
  const space = storageFrom115({ data: { valid: true, user_info: { total_size: 1024, used_size: 256, remaining_size: 768 } } });
  assert.equal(space.totalText, '1 KB');
  assert.equal(space.percent, 25);
});

test('persisted dashboard layout repairs unknown and duplicate module entries', () => {
  assert.deepEqual(normalizeDashboardLayout({ order: ['recent', 'recent', 'unknown'], hidden: ['statistics', 'unknown', 'statistics'] }), {
    order: ['recent', 'storage', 'statistics', 'libraries'], hidden: ['statistics'],
  });
  assert.deepEqual(normalizeDashboardLayout(null).order, dashboardModules.map(item => item.id));
});

test('library covers and recent posters always use Emby primary images', () => {
  assert.equal(embyDashboardImage({ id: 'movies', image_tag: 'composite/tag' }, true), '/image_proxy/Items/movies/Images/Primary?quality=90&tag=composite%2Ftag');
  assert.equal(embyDashboardImage({ id: 'episode', series_id: 'series', series_image_tag: 'poster' }), '/image_proxy/Items/series/Images/Primary?quality=90&tag=poster');
  assert.equal(embyDashboardImage({ id: 'episode', item_type: 'Episode', image_tag: 'episode-screenshot', series_id: 'series', series_image_tag: 'poster' }), '/image_proxy/Items/series/Images/Primary?quality=90&tag=poster');
  assert.equal(embyDashboardImage({ id: 'music' }), '/image_proxy/Items/music/Images/Primary?quality=90');
  assert.equal(embyDashboardImage({ id: '../unsafe' }), '');
});

test('unknown or malformed media counts remain unknown while a confirmed zero is shown', () => {
  for (const value of [null, undefined, NaN, Infinity, '10', -1]) assert.equal(formatDashboardCount(value), '—');
  assert.equal(formatDashboardCount(0), '0');
  assert.equal(formatDashboardCount(13781), '13,781');
});

const memoryStorage = () => {
  const values = new Map();
  return { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key), values };
};

test('MoviePilot-style snapshots are isolated by the user captured when the widget was created', () => {
  const storage = memoryStorage();
  const userA = dashboardSnapshot('user-a', 'recent', storage, () => 100);
  const userB = dashboardSnapshot('user-b', 'recent', storage, () => 100);
  userA.write([{ id: 'one', name: 'Movie' }]);
  assert.equal(userB.read(), undefined);
  userB.write([]);
  userA.write([{ id: 'late-a', name: 'Late response for A' }]);
  assert.deepEqual(userB.read().value, []);
  assert.equal(userA.read().value[0].id, 'late-a');
  assert.equal(dashboardSnapshot('', 'recent', storage).read(), undefined);
});

test('snapshots expire after 24 hours and reject future or malformed saved times', () => {
  const storage = memoryStorage();
  let now = 100;
  const snapshot = dashboardSnapshot('user', 'libraries', storage, () => now);
  snapshot.write([]);
  now += 24 * 60 * 60 * 1000;
  assert.deepEqual(snapshot.read().value, []);
  now += 1;
  assert.equal(snapshot.read(), undefined);
  assert.equal(storage.values.size, 0);
  snapshot.write([]);
  now -= 1;
  assert.equal(snapshot.read(), undefined);
  storage.setItem('ETK_DASHBOARD_SNAPSHOT_V1:user:libraries', '{bad-json');
  assert.equal(snapshot.read(), undefined);
});

test('a confirmed empty result remains a successful snapshot and optional storage failures do not break data', () => {
  const storage = memoryStorage();
  const snapshot = dashboardSnapshot('user', 'recent', storage, () => 100);
  assert.equal(snapshot.read(), undefined);
  snapshot.write([]);
  assert.deepEqual(snapshot.read(), { savedAt: 100, value: [] });
  const unavailable = dashboardSnapshot('user', 'recent', { getItem() { throw new Error('blocked'); }, setItem() { throw new Error('blocked'); }, removeItem() {} }, () => 100);
  assert.equal(unavailable.read(), undefined);
  assert.deepEqual(unavailable.write([]), { savedAt: 100, value: [] });
});

test('media snapshots project safe display fields and reject nested or unsafe card content', () => {
  assert.deepEqual(projectDashboardItems([{ id: '101', name: 'Movie', image_tag: 'poster', Path: '/private/media', credentials: 'private-value' }]), [{ id: '101', name: 'Movie', image_tag: 'poster' }]);
  assert.deepEqual(projectDashboardItems([{ id: 'movies', name: 'Library', image_tag: 'primary', connection: { token: 'private-value' } }], true), [{ id: 'movies', name: 'Library', image_tag: 'primary' }]);
  assert.throws(() => projectDashboardItems([{ id: '../bad', name: 'Bad' }]));
  assert.throws(() => projectDashboardItems([{ id: '101', name: 'Bad', image_tag: { token: 'private-value' } }]));
});

test('statistics replace atomically only when the whole widget has a confirmed successful result', () => {
  const valid = { movie_count: { count: 1 }, tv_count: { count: 2 }, episode_count: { count: 3 }, music_count: { count: 0 } };
  assert.deepEqual(projectDashboardStatistics(valid), valid);
  assert.throws(() => projectDashboardStatistics({ ...valid, movie_count: { count: 0, error: '未确认' } }));
  assert.throws(() => projectDashboardStatistics({ ...valid, episode_count: { count: null } }));
});

test('monthly additions preserve actual history counts, source and partial recording metadata', () => {
  const current = new Date(2026, 10, 15);
  const value = Object.fromEntries(['movie_count', 'tv_count', 'episode_count', 'music_count'].map((key, index) => [key, { count: 100, month_count: index }]));
  value.month = { period: '2026-11', state: 'recording', source: 'etk-history', recording_since: '2026-11-10T18:30:57Z', partial: true, private_token: 'must-not-cache' };
  const projected = projectDashboardStatistics(value);
  assert.equal(dashboardMonthlyCount(projected, 'movie_count', current), 0);
  assert.equal(dashboardMonthlyCount(projected, 'episode_count', current), 2);
  assert.equal(projected.month.partial, true);
  assert.equal(projected.month.recording_since, '2026-11-10T18:30:57Z');
  assert.equal(projected.month.private_token, undefined);
});

test('legacy, cross-month and failed monthly statistics never become a confirmed zero', () => {
  const current = new Date(2026, 10, 15);
  const base = { movie_count: { count: 11 }, tv_count: { count: 7 }, episode_count: { count: 42 }, music_count: { count: 3 } };
  assert.equal(dashboardMonthlyCount(projectDashboardStatistics(base), 'movie_count', current), null);
  const value = { ...base, movie_count: { count: 12, month_count: null }, month: { period: '2026-11', state: 'unavailable', source: 'etk-history', error: '历史记录读取失败' } };
  const projected = projectDashboardStatistics(value);
  assert.equal(projected.movie_count.count, 12, 'Emby totals can still update while admissions are unavailable');
  assert.equal(dashboardMonthlyCount(projected, 'movie_count', current), null);
  assert.equal(projected.month.error, '历史记录读取失败');
  assert.equal(dashboardMonthlyCount({ ...value, movie_count: { count: 12, month_count: 3 }, month: { period: '2025-11', state: 'recording', source: 'etk-history' } }, 'movie_count', current), null);
  assert.equal(dashboardMonthlyCount({ ...value, movie_count: { count: 12, month_count: 3 }, month: { period: '2026-10', state: 'recording', source: 'etk-history' } }, 'movie_count', current), null);
});

test('malformed monthly numbers and identity metadata cannot become a successful snapshot', () => {
  const valid = { movie_count: { count: 1 }, tv_count: { count: 2 }, episode_count: { count: 3 }, music_count: { count: 0 } };
  for (const count of ['3', -1, 1.5, Infinity, Number.MAX_SAFE_INTEGER + 1]) assert.throws(() => projectDashboardStatistics({ ...valid, movie_count: { count: 1, month_count: count } }));
  for (const month of [{ period: '2026-13', state: 'recording', source: 'etk-history' }, { period: '2026-11', state: 'recording', source: 'emby-created-date' }, { period: '2026-11', state: 'recording', source: 'etk-history', error: { token: 'private' } }]) assert.throws(() => projectDashboardStatistics({ ...valid, month }));
});


test('library snapshots retain confirmed zero and error counts without persisting private fields', () => {
  assert.deepEqual(projectDashboardItems([{ id: 'movies', name: '电影', item_count: 0, token: 'private' }], true), [{ id: 'movies', name: '电影', item_count: 0 }]);
  assert.deepEqual(projectDashboardItems([{ id: 'movies', name: '电影', item_count: null, count_error: '数量读取失败' }], true), [{ id: 'movies', name: '电影', item_count: null, count_error: '数量读取失败' }]);
});


test('GridStack profiles keep MP desktop, tablet and mobile columns and reject invalid geometry', () => {
  assert.equal(dashboardLayoutProfile(1920), 'desktop');
  assert.equal(dashboardLayoutProfile(960), 'tablet');
  assert.equal(dashboardLayoutProfile(640), 'mobile');
  assert.deepEqual(['desktop', 'tablet', 'mobile'].map(dashboardColumns), [12, 6, 1]);
  const legacy = { order: ['libraries', 'recent'], hidden: ['statistics'] };
  assert.deepEqual(normalizeDashboardProfile(legacy, 'desktop'), { ...normalizeDashboardLayout(legacy), items: {} });
  const value = { items: { storage: { x: 0, y: 2, w: 4, h: 12, manualHeight: true }, statistics: { x: 10, y: 0, w: 6, h: 8 }, recent: { x: -1, y: 0, w: 12, h: 8 }, libraries: { x: 0, y: 0, w: 1, h: 2000 } } };
  assert.deepEqual(normalizeDashboardProfile(value, 'desktop').items, { storage: { x: 0, y: 2, w: 4, h: 12, manualHeight: true } });
  assert.deepEqual(normalizeDashboardProfile(value, 'mobile').items, {});
});


test('dashboard profile timestamps survive canonical projection and malformed timestamps do not', () => {
  assert.equal(normalizeDashboardProfile({ updatedAt: 123456 }).updatedAt, 123456);
  for (const updatedAt of [-1, NaN, Infinity, '123', 1.5, Number.MAX_SAFE_INTEGER + 1]) assert.equal(normalizeDashboardProfile({ updatedAt }).updatedAt, undefined);
});
