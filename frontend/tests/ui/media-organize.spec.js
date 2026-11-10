import { test, expect } from './fixtures.js';

const path = '/api/actions/reorganize';
const first = { Id: '101', Name: '第一页电影', Type: 'Movie', ProductionYear: 2026, ImageTags: { Primary: 'fixture' } };
const second = { Id: '202', Name: '第二页电影', Type: 'Movie', ProductionYear: 2025, ImageTags: { Primary: 'fixture' } };
const list = (items, total = 1) => ({ library: { id: 'movies', name: '电影' }, items, total });
const result = ids => ({ selected_count: ids.length, updated: ids.length, failed: 0, items: ids.map(item_id => ({ item_id, status: 'completed', report: { updated: 1 } })), message: '媒体重新整理完成' });
const selectRow = page => page.getByTestId('organize-items').locator('tbody tr').first().getByRole('checkbox').check();
const posts = api => api.requests.filter(request => request.method === 'POST' && request.path === path);

test('a selected movie runs the full background operation after scope confirmation', async ({ page, api }) => {
  api.respond('GET', '/api/media_center/libraries/movies/items', list([first]));
  api.queueTask('POST', path, result(['101']), { id: 'organize-complete', status: 200, state: 'completed', polls: 0 });
  await page.goto('/media-organize');
  await selectRow(page);
  await page.getByTestId('organize-selected').click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByTestId('organize-confirm') });
  await expect(dialog).toContainText('第一页电影');
  await expect(dialog).toContainText('电影包含关联版本，电视剧包含当前在库季集');
  await dialog.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-result')).toContainText('媒体重新整理完成');
  expect(posts(api)).toHaveLength(1);
  expect(posts(api)[0].body).toEqual({ item_ids: ['101'] });
  expect(api.requests.some(request => request.path === '/api/tasks/result/organize-complete')).toBe(true);
});

test('cross page selections remain visible and only their IDs are submitted', async ({ page, api }) => {
  api.respond('GET', '/api/media_center/libraries/movies/items', request => list(Number(request.query.start_index) >= 24 ? [second] : [first], 48));
  api.queueTask('POST', path, result(['101', '202']), { id: 'organize-two', status: 200, state: 'completed', polls: 0 });
  await page.goto('/media-organize');
  await selectRow(page);
  await page.locator('.v-pagination__item').filter({ hasText: /^2$/ }).getByRole('button').click();
  await expect(page.getByTestId('organize-items')).toContainText('第二页电影');
  await selectRow(page);
  await expect(page.getByTestId('organize-selection')).toContainText('已选择 2 项');
  await expect(page.getByTestId('organize-selection')).toContainText('第一页电影');
  await page.getByTestId('organize-selected').click();
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-result')).toBeVisible();
  expect(posts(api)[0].body).toEqual({ item_ids: ['101', '202'] });
  expect(api.requests.find(request => request.path.endsWith('/items') && request.query.start_index === '24').query.page).toBe('2');
});

test('a failed library page retains the explicit prior selection and submits no work automatically', async ({ page, api }) => {
  api.respond('GET', '/api/media_center/libraries/movies/items', list([first]));
  await page.goto('/media-organize');
  await selectRow(page);
  api.respond('GET', '/api/media_center/libraries/movies/items', { error: 'Emby 媒体读取失败' }, 403);
  await page.getByRole('button', { name: '搜索', exact: true }).click();
  await expect(page.getByTestId('organize-load-error')).toContainText('Emby 媒体读取失败');
  await expect(page.getByTestId('organize-selection')).toContainText('已选择 1 项');
  expect(posts(api)).toHaveLength(0);
});

test('partial failure and warnings from the final job stay visible', async ({ page, api }) => {
  api.respond('GET', '/api/media_center/libraries/movies/items', list([first]));
  api.queueTask('POST', path, { ...result(['101']), updated: 0, failed: 1, failures: ['101：原生回读未完成'], warning_details: ['101：原图备份已保留'] }, { id: 'organize-failed', status: 207, state: 'failed', polls: 0 });
  await page.goto('/media-organize');
  await selectRow(page);
  await page.getByTestId('organize-selected').click();
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-result')).toContainText('原生回读未完成');
  await expect(page.getByTestId('organize-result')).toContainText('原图备份已保留');
  await expect(page.getByTestId('organize-selection')).toContainText('已选择 1 项');
});

test('a bare or incomplete success response cannot claim the selected work finished', async ({ page, api }) => {
  api.respond('GET', '/api/media_center/libraries/movies/items', list([first]));
  api.respond('POST', path, { ...result(['101']), items: [] });
  await page.goto('/media-organize');
  await selectRow(page);
  await page.getByTestId('organize-selected').click();
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-result')).toContainText('媒体整理结果尚未确认');
  await expect(page.getByTestId('organize-result')).toContainText('尚未取得完整任务结果');
});

test('rapid confirmation clicks do not start another organize job', async ({ page, api }) => {
  api.respond('GET', '/api/media_center/libraries/movies/items', list([first]));
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
