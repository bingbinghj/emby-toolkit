import test from 'node:test';
import assert from 'node:assert/strict';
import { dashboardModules, normalizeDashboardLayout, reorderVisibleDashboard, storageFrom115, formatDashboardBytes, embyDashboardImage, formatDashboardCount, dashboardSnapshot, projectDashboardItems, projectDashboardStatistics } from '../src/utils/dashboard.js';

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

test('dragging visible modules preserves the positions of hidden module slots', () => {
  assert.deepEqual(reorderVisibleDashboard(['storage', 'statistics', 'recent', 'libraries'], ['libraries', 'recent', 'storage']), ['libraries', 'statistics', 'recent', 'storage']);
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
