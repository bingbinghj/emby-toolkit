import { test, expect } from './fixtures.js';

const path = '/api/actions/reorganize';
const first = { id: '00000000-0000-4000-8000-000000000101', item_id: '101', name: '第一页电影', item_type: 'Movie', status: 'success', created_at: '2026-11-10T02:00:00Z', image_tag: 'fixture' };
const second = { id: '00000000-0000-4000-8000-000000000202', item_id: '202', name: '第二页电影', item_type: 'Movie', status: 'success', created_at: '2026-11-09T02:00:00Z', image_tag: 'fixture' };
const listPath = '/api/media_organize/history';
const list = (items, total = items.length) => ({ items, total });
const result = ids => ({ selected_count: ids.length, updated: ids.length, failed: 0, items: ids.map(item_id => ({ item_id, status: 'completed', report: { updated: 1 } })), message: '媒体重新整理完成' });
const selectRow = page => page.getByTestId('organize-items').locator('tbody tr').first().getByRole('checkbox').check();
const posts = api => api.requests.filter(request => request.method === 'POST' && request.path === path);

test('a selected movie runs the full background operation after scope confirmation', async ({ page, api }) => {
  api.respond('GET', listPath, list([first]));
  api.queueTask('POST', path, result(['101']), { id: 'organize-complete', status: 200, state: 'completed', polls: 0 });
  await page.goto('/media-organize');
  await selectRow(page);
  await page.getByTestId('organize-selected').click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByTestId('organize-confirm') });
  await expect(dialog).toContainText('第一页电影');
  await expect(dialog).toContainText('电影包含关联版本，剧集只处理所选单集');
  await dialog.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-result')).toContainText('媒体重新整理完成');
  expect(posts(api)).toHaveLength(1);
  expect(posts(api)[0].body).toEqual({ item_ids: ['101'], history_ids: ['00000000-0000-4000-8000-000000000101'] });
  expect(api.requests.some(request => request.path === '/api/tasks/result/organize-complete')).toBe(true);
});

test('cross page selections remain visible and only their IDs are submitted', async ({ page, api }) => {
  api.respond('GET', listPath, request => list(Number(request.query.page) >= 2 ? [second] : [first], 48));
  api.queueTask('POST', path, result(['101', '202']), { id: 'organize-two', status: 200, state: 'completed', polls: 0 });
  await page.goto('/media-organize');
  await selectRow(page);
  await page.locator('.v-pagination__item').filter({ hasText: /^2$/ }).getByRole('button').click();
  await expect(page.getByTestId('organize-items')).toContainText('第二页电影');
  await selectRow(page);
  await expect(page.getByTestId('organize-selection')).toContainText('已选择 2 条记录');
  await expect(page.getByTestId('organize-selection')).toContainText('第一页电影');
  await page.getByTestId('organize-selected').click();
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-result')).toBeVisible();
  expect(posts(api)[0].body).toEqual({ item_ids: ['101', '202'], history_ids: ['00000000-0000-4000-8000-000000000101', '00000000-0000-4000-8000-000000000202'] });
  expect(api.requests.find(request => request.path === listPath && request.query.page === '2').query.page).toBe('2');
});

test('a failed recent admission page retains the explicit prior selection and submits no work automatically', async ({ page, api }) => {
  api.respond('GET', listPath, list([first]));
  await page.goto('/media-organize');
  await selectRow(page);
  api.respond('GET', listPath, { error: 'Emby 媒体读取失败' }, 403);
  await page.getByRole('button', { name: '搜索', exact: true }).click();
  await expect(page.getByTestId('organize-load-error')).toContainText('Emby 媒体读取失败');
  await expect(page.getByTestId('organize-selection')).toContainText('已选择 1 条记录');
  expect(posts(api)).toHaveLength(0);
});

test('partial failure and warnings from the final job stay visible', async ({ page, api }) => {
  api.respond('GET', listPath, list([first]));
  api.queueTask('POST', path, { ...result(['101']), updated: 0, failed: 1, failures: ['101：原生回读未完成'], warning_details: ['101：原图备份已保留'] }, { id: 'organize-failed', status: 207, state: 'failed', polls: 0 });
  await page.goto('/media-organize');
  await selectRow(page);
  await page.getByTestId('organize-selected').click();
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-result')).toContainText('原生回读未完成');
  await expect(page.getByTestId('organize-result')).toContainText('原图备份已保留');
  await expect(page.getByTestId('organize-selection')).toContainText('已选择 1 条记录');
});

test('a bare or incomplete success response cannot claim the selected work finished', async ({ page, api }) => {
  api.respond('GET', listPath, list([first]));
  api.respond('POST', path, { ...result(['101']), items: [] });
  await page.goto('/media-organize');
  await selectRow(page);
  await page.getByTestId('organize-selected').click();
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-result')).toContainText('媒体整理结果尚未确认');
  await expect(page.getByTestId('organize-result')).toContainText('尚未取得完整任务结果');
});

test('rapid confirmation clicks do not start another organize job', async ({ page, api }) => {
  api.respond('GET', listPath, list([first]));
  const pending = api.defer('POST', path);
  await page.goto('/media-organize');
  await selectRow(page);
  await page.getByTestId('organize-selected').click();
  await page.getByTestId('organize-confirm').dblclick();
  await pending.started;
  expect(posts(api)).toHaveLength(1);
  pending.release(result(['101']));
  await expect(page.getByTestId('organize-result')).toBeVisible();
});


test('ETK admission history is the source and an episode submits only its own ID', async ({ page, api }) => {
  const episode = { id: '00000000-0000-4000-8000-000000000005', item_id: 'episode-5', name: '新的开始', item_type: 'Episode', series_id: 'series-1', series_name: '最近入库电视剧', season_number: 2, episode_number: 5, status: 'success', created_at: '2026-11-10T02:00:00Z' };
  api.respond('GET', listPath, list([episode]));
  api.queueTask('POST', path, result(['episode-5']), { id: 'organize-episode', status: 200, state: 'completed', polls: 0 });
  await page.goto('/media-organize');
  await expect(page.getByTestId('organize-items')).toContainText('最近入库电视剧');
  await expect(page.getByTestId('organize-items')).toContainText('S02E05');
  expect(api.requests.some(request => request.path === '/api/media_center/libraries' || request.path.startsWith('/api/media_center/libraries/'))).toBe(false);
  await expect(page.getByTestId('organize-library')).toHaveCount(0);
  await selectRow(page);
  await page.getByTestId('organize-selected').click();
  await expect(page.locator('.v-overlay--active').filter({ has: page.getByTestId('organize-confirm') })).toContainText('剧集只处理所选单集');
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-result')).toContainText('媒体重新整理完成');
  expect(posts(api)[0].body).toEqual({ item_ids: ['episode-5'], history_ids: ['00000000-0000-4000-8000-000000000005'] });
});


test('mobile history cards preserve real status and disable unfinished records', async ({ page, api }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  api.respond('GET', listPath, list([{ ...first, status: 'processing', message: '媒体信息正在提取' }, { ...second, item_id: null, status: 'failed', message: '未能匹配到媒体' }]));
  await page.goto('/media-organize');
  const cards = page.getByTestId('organize-items-mobile');
  await expect(cards).toBeVisible();
  await expect(cards).toContainText('正在处理');
  await expect(cards).toContainText('未能匹配到媒体');
  for (const checkbox of await cards.getByRole('checkbox').all()) await expect(checkbox).toBeDisabled();
  await expect(page.getByTestId('organize-selected')).toBeDisabled();
  expect(posts(api)).toHaveLength(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('history filters use real status and do not fall back to Emby recent items', async ({ page, api }) => {
  api.respond('GET', listPath, request => list(request.query.status === 'failed' ? [{ ...first, status: 'failed', message: '原生回读失败' }] : [first]));
  await page.goto('/media-organize');
  await page.locator('.media-organize-page').getByRole('combobox').click();
  await page.getByRole('option', { name: '失败', exact: true }).click();
  await expect(page.getByTestId('organize-items')).toContainText('原生回读失败');
  expect(api.requests.find(request => request.path === listPath && request.query.status === 'failed').query.page).toBe('1');
  expect(api.requests.some(request => request.path === '/api/dashboard/media' || request.path.startsWith('/api/media_center/libraries'))).toBe(false);
});

test('empty history shows its tracking start without claiming past records exist', async ({ page, api }) => {
  api.respond('GET', listPath, { items: [], total: 0, tracking_since: '2026-11-10T03:00:00Z', legacy_records_available: false });
  await page.goto('/media-organize');
  await expect(page.getByTestId('organize-tracking')).toContainText('此前入库的历史记录不可用');
  await expect(page.getByTestId('organize-items')).toContainText('当前条件下没有入库处理记录');
  await expect(page.getByTestId('organize-selected')).toBeDisabled();
  expect(api.requests.filter(request => request.path === listPath)).toHaveLength(1);
});

test('confirmed organize reloads history status and preserves the selected record', async ({ page, api }) => {
  let completed = false;
  api.respond('GET', listPath, () => list([{ ...first, status: completed ? 'warning' : 'failed', message: completed ? '媒体已整理，图片待确认' : '首次处理失败' }]));
  api.respond('POST', path, request => { completed = true; return result(request.body.item_ids); });
  await page.goto('/media-organize');
  await selectRow(page);
  await page.getByTestId('organize-selected').click();
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-items')).toContainText('媒体已整理，图片待确认');
  await expect(page.getByTestId('organize-selection')).toContainText('有提醒');
  await expect(page.getByTestId('organize-selection')).toContainText('已选择 1 条记录');
  expect(posts(api)[0].body).toEqual({ item_ids: ['101'], history_ids: ['00000000-0000-4000-8000-000000000101'] });
});

test('repeated historical events for one movie submit all record IDs and one media ID', async ({ page, api }) => {
  const repeated = { ...first, id: '00000000-0000-4000-8000-000000000111', created_at: '2026-11-09T01:00:00Z' };
  api.respond('GET', listPath, list([first, repeated], 2));
  api.respond('POST', path, result(['101']));
  await page.goto('/media-organize');
  const rows = page.getByTestId('organize-items').locator('tbody tr');
  await rows.nth(0).getByRole('checkbox').check();
  await rows.nth(1).getByRole('checkbox').check();
  await expect(page.getByTestId('organize-selection')).toContainText('已选择 2 条记录，对应 1 项媒体');
  await page.getByTestId('organize-selected').click();
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-result')).toBeVisible();
  expect(posts(api)[0].body).toEqual({ item_ids: ['101'], history_ids: ['00000000-0000-4000-8000-000000000101', '00000000-0000-4000-8000-000000000111'] });
});
