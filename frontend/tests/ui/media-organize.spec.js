import { test, expect } from './fixtures.js';

const path = '/api/actions/reorganize';
const first = { id: '00000000-0000-4000-8000-000000000101', item_id: '101', name: '第一页电影', item_type: 'Movie', status: 'success', created_at: '2026-11-10T02:00:00Z', image_tag: 'fixture' };
const second = { id: '00000000-0000-4000-8000-000000000202', item_id: '202', name: '第二页电影', item_type: 'Movie', status: 'success', created_at: '2026-11-09T02:00:00Z', image_tag: 'fixture' };
const listPath = '/api/media_organize/history';
const list = (items, total = items.length) => ({ items, total });
const result = ids => ({ selected_count: ids.length, updated: ids.length, failed: 0, items: ids.map(item_id => ({ item_id, status: 'completed', report: { updated: 1 } })), message: '媒体重新整理完成' });
const selectRow = page => page.getByTestId('organize-items').locator('tbody tr').filter({ has: page.getByRole('checkbox') }).first().getByRole('checkbox').check();
const posts = api => api.requests.filter(request => request.method === 'POST' && request.path === path);
const selectPage = async page => {
  if (await page.getByTestId('organize-select-page').count() === 0) await selectRow(page);
  await page.getByTestId('organize-select-page').getByRole('checkbox').check();
};
const changePageSize = async (page, size) => {
  await page.getByTestId('organize-page-size').click();
  await page.locator('.v-overlay--active').getByText(String(size), { exact: true }).click();
};
const historyRecords = (count = 1000, start = 1) => Array.from({ length: count }, (_, index) => ({ ...first,
  id: `00000000-0000-4000-8000-${String(start + index).padStart(12, '0')}`, item_id: `media-${start + index}`, name: `记录${start + index}`, image_tag: null,
}));
const acceptedQueue = ids => ({ mode: 'reprocess', queued_count: ids.length, duplicate_count: 0,
  queue_ids: ids.map(id => `queue-${id}`), items: ids.map(item_id => ({ id: `queue-${item_id}`, item_id, mode: 'reprocess', status: 'queued' })),
  message: '媒体已加入整理队列',
});

for (const viewport of [{ width: 1280, height: 720 }, { width: 1100, height: 450 }]) {
  test(`the default 50-record page keeps its footer visible while records scroll at ${viewport.width}x${viewport.height}`, async ({ page, api }) => {
    await page.setViewportSize(viewport);
    const records = historyRecords(50);
    api.respond('GET', listPath, list(records, 100));
    await page.goto('/media-organize');
    const table = page.getByTestId('organize-items'), footer = page.getByTestId('organize-footer');
    await expect(table).toContainText('记录1');
    await expect(footer).toBeInViewport({ ratio: 1 });
    expect(api.requests.find(request => request.path === listPath).query.page_size).toBe('50');
    const scroll = table.locator('.v-table__wrapper');
    expect(await scroll.evaluate(element => element.scrollHeight > element.clientHeight)).toBe(true);
    await scroll.evaluate(element => { element.scrollTop = element.scrollHeight; });
    const lastRecord = table.getByRole('link', { name: '记录50', exact: true });
    await expect(lastRecord).toHaveAttribute('href', '/edit-media/media-50');
    await expect(lastRecord).toBeVisible();
    await expect(footer).toBeInViewport({ ratio: 1 });
    await selectPage(page);
    await expect(page.getByTestId('organize-selection-count')).toHaveText('50/50 项');
    await expect(footer).toBeInViewport({ ratio: 1 });
    await scroll.evaluate(element => { element.scrollTop = element.scrollHeight; });
    await expect(lastRecord).toBeVisible();
    await table.locator('.organize-record').filter({ has: page.getByRole('link', { name: '记录50', exact: true }) }).getByRole('checkbox').uncheck();
    await expect(page.getByTestId('organize-selection-count')).toHaveText('49/50 项');
    await expect(footer).toBeInViewport({ ratio: 1 });
    const geometry = await footer.evaluate(element => ({ bottom: element.getBoundingClientRect().bottom,
      dockTop: document.querySelector('.footer-nav-container')?.getBoundingClientRect().top ?? innerHeight,
      documentHeight: document.documentElement.scrollHeight, viewportHeight: innerHeight,
    }));
    expect(geometry.bottom).toBeLessThanOrEqual(geometry.dockTop);
    expect(geometry.documentHeight).toBeLessThanOrEqual(geometry.viewportHeight + 2);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}

test('MP page-size choices and first-last controls show the requested record range and total', async ({ page, api }) => {
  api.respond('GET', listPath, list([first], 123));
  await page.goto('/media-organize');
  const info = page.getByTestId('organize-page-info');
  await expect(info).toHaveText('1 - 50 / 123');
  expect(api.requests.find(request => request.path === listPath).query.page_size).toBe('50');
  await page.getByTestId('organize-page-size').click();
  for (const size of [25, 50, 100, 500, 1000]) await expect(page.locator('.v-overlay--active').getByText(String(size), { exact: true })).toBeVisible();
  await page.locator('.v-overlay--active').getByText('50', { exact: true }).click();
  await page.getByTestId('organize-pagination').locator('.v-pagination__last button').click();
  await expect(info).toHaveText('101 - 123 / 123');
  await page.getByTestId('organize-pagination').locator('.v-pagination__first button').click();
  await expect(info).toHaveText('1 - 50 / 123');
  expect(api.requests.filter(request => request.path === listPath).map(request => request.query.page)).toEqual(['1', '3', '1']);
});

test('1000 returned history rows render as a virtual table and all are accepted in one queue submission', async ({ page, api }) => {
  const records = historyRecords();
  api.respond('GET', listPath, request => list(records.slice(0, Number(request.query.page_size)), records.length));
  api.respond('POST', path, request => acceptedQueue(request.body.item_ids), 202);
  await page.goto('/media-organize');
  await changePageSize(page, 1000);
  const table = page.getByTestId('organize-items');
  await expect(page.getByTestId('organize-page-info')).toHaveText('1 - 1000 / 1000');
  expect(api.requests.filter(request => request.path === listPath).at(-1).query).toMatchObject({ page: '1', page_size: '1000' });
  expect(await table.locator('tbody tr').count()).toBeLessThan(100);
  const firstRowCheckbox = table.locator('tbody tr').filter({ has: page.getByRole('checkbox') }).first().getByRole('checkbox');
  await firstRowCheckbox.focus();
  await firstRowCheckbox.press('Space');
  await expect(page.getByTestId('organize-selection-count')).toHaveText('1/1000 项');
  await selectPage(page);
  await expect(page.getByTestId('organize-selection-count')).toHaveText('1000/1000 项');
  await expect(page.getByTestId('organize-selection').locator('.v-chip')).toHaveCount(0);
  await page.getByTestId('organize-selected').click();
  const review = page.locator('.organize-confirmation-card');
  await expect(review.locator('.v-card-subtitle')).toHaveText('共 1000 项');
  await expect(review.locator('.v-list-item')).toHaveCount(0);
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-queue-dialog')).toBeVisible();
  await expect(page.getByTestId('organize-result')).toHaveCount(0);
  expect(posts(api)).toHaveLength(1);
  expect(posts(api)[0].body.item_ids).toEqual(records.map(item => item.item_id));
  expect(posts(api)[0].body.history_ids).toEqual(records.map(item => item.id));
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('a virtual selection draft can revise its 1000th row before a batch is scoped to the current page', async ({ page, api }) => {
  const records = historyRecords(2000);
  api.respond('GET', listPath, request => {
    const size = Number(request.query.page_size), offset = (Number(request.query.page) - 1) * size;
    return list(records.slice(offset, offset + size), records.length);
  });
  api.respond('POST', path, request => acceptedQueue(request.body.item_ids), 202);
  await page.goto('/media-organize');
  await changePageSize(page, 1000);
  await selectPage(page);
  await expect(page.getByTestId('organize-selection-count')).toHaveText('1000/1000 项');
  await page.getByTestId('organize-pagination').locator('.v-pagination__item').filter({ hasText: /^2$/ }).getByRole('button').click();
  await expect(page.getByTestId('organize-page-info')).toHaveText('1001 - 2000 / 2000');
  await expect(page.getByTestId('organize-selection-count')).toHaveText('0/1000 项');
  await expect(page.getByTestId('organize-selected')).toBeDisabled();
  await expect(page.getByTestId('organize-selected-delete')).toBeDisabled();
  await page.getByTestId('organize-pagination').locator('.v-pagination__item').filter({ hasText: /^1$/ }).getByRole('button').click();
  const table = page.getByTestId('organize-items');
  await table.locator('.v-table__wrapper').evaluate(element => { element.scrollTop = element.scrollHeight; });
  const lastRecord = table.locator('.organize-record').filter({ has: page.getByRole('link', { name: '记录1000', exact: true }) });
  await expect(lastRecord).toBeVisible();
  await lastRecord.getByRole('checkbox').uncheck();
  await expect(page.getByTestId('organize-selection-count')).toHaveText('999/1000 项');
  await page.getByTestId('organize-pagination').locator('.v-pagination__item').filter({ hasText: /^2$/ }).getByRole('button').click();
  await expect(page.getByTestId('organize-selection-count')).toHaveText('0/1000 项');
  await selectRow(page);
  await expect(page.getByTestId('organize-selection-count')).toHaveText('1/1000 项');
  await page.getByTestId('organize-selected').click();
  await expect(page.locator('.v-overlay--active').filter({ has: page.getByTestId('organize-confirm') }).locator('.v-card-subtitle')).toHaveText('共 1 项');
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-queue-dialog')).toBeVisible();
  await expect(page.getByTestId('organize-result')).toHaveCount(0);
  expect(posts(api)[0].body.item_ids).toEqual([records[1000].item_id]);
  expect(posts(api)[0].body.history_ids).toEqual([records[1000].id]);
  expect(posts(api)[0].body.history_ids).not.toContain(records[999].id);
});

test('mobile history appends 25 records on scroll and resets when filtering', async ({ page, api }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const records = historyRecords(31);
  api.respond('GET', listPath, ({ query }) => query.status === 'failed' ? list([]) : list(records.slice((query.page - 1) * 25, query.page * 25), 31));
  await page.goto('/media-organize');
  await expect(page.getByRole('link', {name:'记录1', exact:true})).toBeVisible();
  await page.getByText('加载更多', {exact:true}).scrollIntoViewIfNeeded();
  await expect.poll(() => api.requests.some(r => r.path === listPath && r.query.page === '2')).toBe(true);
  await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
  await expect(page.getByRole('link', {name:'记录31', exact:true})).toBeVisible();
  await page.getByTestId('organize-status-filter').click();
  await page.locator('.v-overlay--active').getByText('失败', {exact:true}).click();
  await expect(page.getByTestId('organize-items-mobile')).toContainText('当前条件下没有入库处理记录');
  expect(api.requests.filter(r => r.path === listPath).at(-1).query).toMatchObject({ page:'1', page_size:'25', status:'failed' });
});

test('a selected movie runs the full background operation after scope confirmation', async ({ page, api }) => {
  api.respond('GET', listPath, list([first]));
  api.queueTask('POST', path, result(['101']), { id: 'organize-complete', status: 200, state: 'completed', polls: 0 });
  await page.goto('/media-organize');
  await selectRow(page);
  await page.getByTestId('organize-selected').click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByTestId('organize-confirm') });
  await expect(dialog.locator('.v-card-subtitle')).toHaveText('共 1 项');
  await expect(dialog).toContainText('仅处理所选媒体文件，剧集只处理所选单集');
  await dialog.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-result')).toContainText('媒体重新整理完成');
  expect(posts(api)).toHaveLength(1);
  expect(posts(api)[0].body).toEqual({ item_ids: ['101'], history_ids: ['00000000-0000-4000-8000-000000000101'], mode: 'reprocess' });
  expect(api.requests.some(request => request.path === '/api/tasks/result/organize-complete')).toBe(true);
});

test('current-page batch reorganization prunes earlier-page drafts and submits only the visible IDs', async ({ page, api }) => {
  api.respond('GET', listPath, request => list(Number(request.query.page) >= 2 ? [second] : [first], 100));
  api.queueTask('POST', path, result(['202']), { id: 'organize-two', status: 200, state: 'completed', polls: 0 });
  await page.goto('/media-organize');
  await selectRow(page);
  await page.locator('.v-pagination__item').filter({ hasText: /^2$/ }).getByRole('button').click();
  await expect(page.getByTestId('organize-items')).toContainText('第二页电影');
  await selectRow(page);
  await expect(page.getByTestId('organize-selection-count')).toHaveText('1/1 项');
  await page.getByTestId('organize-selected').click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByTestId('organize-confirm') });
  await expect(dialog).not.toContainText('第一页电影');
  await expect(dialog.locator('.v-card-subtitle')).toHaveText('共 1 项');
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-result')).toBeVisible();
  expect(posts(api)[0].body).toEqual({ item_ids: ['202'], history_ids: ['00000000-0000-4000-8000-000000000202'], mode: 'reprocess' });
  expect(api.requests.find(request => request.path === listPath && request.query.page === '2').query.page).toBe('2');
});

test('a failed recent admission page retains the explicit prior selection and submits no work automatically', async ({ page, api }) => {
  api.respond('GET', listPath, list([first]));
  await page.goto('/media-organize');
  await selectRow(page);
  api.respond('GET', listPath, { error: 'Emby 媒体读取失败' }, 403);
  await page.getByRole('button', { name: '搜索', exact: true }).click();
  await expect(page.getByTestId('organize-load-error')).toContainText('Emby 媒体读取失败');
  await expect(page.getByTestId('organize-selection-count')).toHaveText('1/1 项');
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
  await expect(page.getByTestId('organize-selection-count')).toHaveText('1/1 项');
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
  expect(posts(api)[0].body).toEqual({ item_ids: ['episode-5'], history_ids: ['00000000-0000-4000-8000-000000000005'], mode: 'reprocess' });
});


test('mobile history cards preserve real status and disable unfinished records', async ({ page, api }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  api.respond('GET', listPath, list([{ ...first, status: 'processing', message: '媒体信息正在提取' }, { ...second, item_id: null, status: 'failed', message: '未能匹配到媒体' }]));
  await page.goto('/media-organize');
  const cards = page.getByTestId('organize-items-mobile');
  await expect(cards).toBeVisible();
  await expect(cards).toContainText('正在处理');
  await expect(cards).not.toContainText('未能匹配到媒体');
  await cards.getByRole('button', { name: /^失败，查看/ }).click();
  await expect(page.getByTestId('organize-detail-dialog')).toContainText('未能匹配到媒体');
  await expect(page.getByTestId('organize-detail-dialog').getByRole('button', { name: '重新整理', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: '关闭整理详情', exact: true }).click();
  await page.getByRole('button', { name: '批量选择', exact: true }).click();
  await expect(cards.getByRole('checkbox').first()).toBeDisabled();
  await expect(cards.getByRole('checkbox').last()).toBeEnabled();
  await cards.getByRole('checkbox').last().check();
  await expect(page.getByTestId('organize-selected')).toBeDisabled();
  await expect(page.getByTestId('organize-selected-delete')).toBeEnabled();
  expect(posts(api)).toHaveLength(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('history filters use real status and do not fall back to Emby recent items', async ({ page, api }) => {
  api.respond('GET', listPath, request => list(request.query.status === 'failed' ? [{ ...first, status: 'failed', message: '原生回读失败' }] : [first]));
  await page.goto('/media-organize');
  await page.getByTestId('organize-status-filter').click();
  await page.locator('.v-overlay--active').getByText('失败', { exact: true }).click();
  await expect(page.getByTestId('organize-items')).not.toContainText('原生回读失败');
  await page.getByTestId('organize-items').getByRole('button', { name: /^失败，查看/ }).click();
  await expect(page.getByTestId('organize-detail-dialog')).toContainText('原生回读失败');
  expect(api.requests.find(request => request.path === listPath && request.query.status === 'failed').query.page).toBe('1');
  expect(api.requests.some(request => request.path === '/api/dashboard/media' || request.path.startsWith('/api/media_center/libraries'))).toBe(false);
});

test('empty history omits the retired tracking banner without inventing prior records', async ({ page, api }) => {
  api.respond('GET', listPath, { items: [], total: 0, tracking_since: '2026-11-10T03:00:00Z', legacy_records_available: false });
  await page.goto('/media-organize');
  await expect(page.getByTestId('organize-tracking')).toHaveCount(0);
  await expect(page.locator('.media-organize-page').getByRole('heading', { name: '媒体整理', exact: true })).toBeVisible();
  await expect(page.getByTestId('organize-items')).toContainText('当前条件下没有入库处理记录');
  await expect(page.getByTestId('organize-selected')).toHaveCount(0);
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
  await expect(page.getByTestId('organize-items')).not.toContainText('媒体已整理，图片待确认');
  await page.getByTestId('organize-items').getByRole('button', { name: /^成功，查看/ }).click();
  await expect(page.getByTestId('organize-detail-dialog')).toContainText('媒体已整理，图片待确认');
  await page.getByRole('button', { name: '关闭整理详情', exact: true }).click();
  await expect(page.getByTestId('organize-items').locator('.organize-record').first().getByRole('checkbox')).toBeChecked();
  await expect(page.getByTestId('organize-selection-count')).toHaveText('1/1 项');
  expect(posts(api)[0].body).toEqual({ item_ids: ['101'], history_ids: ['00000000-0000-4000-8000-000000000101'], mode: 'reprocess' });
});

test('repeated historical events for one movie submit all record IDs and one media ID', async ({ page, api }) => {
  const repeated = { ...first, id: '00000000-0000-4000-8000-000000000111', created_at: '2026-11-09T01:00:00Z' };
  api.respond('GET', listPath, list([first, repeated], 2));
  api.respond('POST', path, result(['101']));
  await page.goto('/media-organize');
  const rows = page.getByTestId('organize-items').locator('tbody tr').filter({ has: page.getByRole('checkbox') });
  await rows.nth(0).getByRole('checkbox').check();
  await rows.nth(1).getByRole('checkbox').check();
  await expect(page.getByTestId('organize-selection-count')).toHaveText('2/2 项');
  await page.getByTestId('organize-selected').click();
  await expect(page.locator('.v-overlay--active').filter({ has: page.getByTestId('organize-confirm') }).locator('.v-card-subtitle')).toHaveText('共 2 项');
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-result')).toBeVisible();
  expect(posts(api)[0].body).toEqual({ item_ids: ['101'], history_ids: ['00000000-0000-4000-8000-000000000101', '00000000-0000-4000-8000-000000000111'], mode: 'reprocess' });
});

test('typing a history query applies the MP delay and keeps real failure records available for retry', async ({ page, api }) => {
  api.respond('GET', listPath, request => list(request.query.search === '失败记录' ? [{ ...first, status: 'failed', message: '原生回读失败，恢复备份仍保留' }] : [first]));
  await page.goto('/media-organize');
  await page.getByRole('textbox', { name: '搜索入库记录', exact: true }).fill('失败记录');
  await page.getByTestId('organize-items').getByRole('button', { name: /^失败，查看/ }).click();
  await expect(page.getByTestId('organize-detail-dialog')).toContainText('原生回读失败，恢复备份仍保留');
  await page.getByRole('button', { name: '关闭整理详情', exact: true }).click();
  expect(api.requests.filter(request => request.path === listPath && request.query.search === '失败记录')).toHaveLength(1);
  await selectRow(page);
  await expect(page.getByTestId('organize-selected')).toBeEnabled();
  expect(posts(api)).toHaveLength(0);
  expect(api.requests.some(request => request.path === '/api/dashboard/media')).toBe(false);
});
