import { test, expect } from './fixtures.js';

const historyPath = '/api/media_organize/history';
const actionPath = '/api/actions/reorganize';
const records = [
  { id: 'record-2', item_id: 'episode-2', name: '第二集', series_name: '整理验证剧集', item_type: 'Episode', season_number: 1, episode_number: 2, status: 'success' },
  { id: 'record-1', item_id: 'movie-1', name: '排序验证电影', item_type: 'Movie', status: 'success' },
];
const rows = page => page.getByTestId('organize-items').locator('.organize-record');
const receipt = request => ({ mode: request.body.mode, queued_count: request.body.item_ids.length, duplicate_count: 0,
  queue_ids: request.body.item_ids.map(id => `queue-${id}`),
  items: request.body.item_ids.map(item_id => ({ id: `queue-${item_id}`, item_id, mode: request.body.mode, status: 'queued' })),
});

test('MP header, virtual record cards, compact selection and single-page pagination remain separate', async ({ page, api }, testInfo) => {
  await page.setViewportSize({ width: 1280, height: 720 });
  api.respond('GET', historyPath, { items: records, total: 2 });
  await page.goto('/media-organize');
  await expect(page.getByRole('heading', { name: '媒体整理', exact: true })).toBeVisible();
  await expect(page.getByTestId('organize-items').locator('thead')).toHaveCount(0);
  await expect(rows(page)).toHaveCount(2);
  await expect(page.getByTestId('organize-selection')).toHaveCount(0);
  await expect(page.getByTestId('organize-pagination')).toBeVisible();
  await expect(page.getByTestId('organize-page-info')).toHaveText('1 - 2 / 2');
  await rows(page).first().getByRole('checkbox').check();
  const selection = page.getByTestId('organize-selection');
  await expect(selection.getByRole('button', { name: '批量重新整理', exact: true })).toBeVisible();
  await expect(selection.getByRole('button', { name: '批量删除', exact: true })).toBeVisible();
  await expect(selection.getByRole('button', { name: '取消全选', exact: true })).toBeVisible();
  await expect(selection.getByRole('checkbox', { name: '全选本页', exact: true })).toBeVisible();
  await expect(selection).toContainText('1/2 项');
  await expect(selection.getByRole('button')).toHaveCount(3);
  await expect(selection.locator('.v-chip')).toHaveCount(0);
  await expect(selection.getByRole('button', { name: '已选记录', exact: true })).toHaveCount(0);
  await expect(page.getByTestId('organize-footer')).toBeInViewport({ ratio: 1 });
  const footer = await page.getByTestId('organize-footer').boundingBox();
  const queue = await page.getByTestId('organize-queue-open').boundingBox();
  expect(queue.y + queue.height).toBeLessThanOrEqual(footer.y - 20);
  expect(1280 - queue.x - queue.width).toBeGreaterThanOrEqual(30);
  await page.screenshot({ path: testInfo.outputPath('media-organize-mp-desktop.png') });
  await selection.getByRole('checkbox', { name: '全选本页', exact: true }).check();
  await expect(page.getByTestId('organize-selection-count')).toHaveText('2/2 项');
  await selection.getByRole('button', { name: '取消全选', exact: true }).click();
  await expect(selection).toHaveCount(0);
  for (const checkbox of await rows(page).getByRole('checkbox').all()) await expect(checkbox).not.toBeChecked();
  expect(api.requests.filter(request => request.method === 'POST' && request.path === actionPath)).toHaveLength(0);
});

for (const mode of ['reprocess', 'rescrape']) {
  test(`the ${mode} selected action opens the matching MP-style form and submits the exact episode`, async ({ page, api }) => {
    api.respond('GET', historyPath, { items: records, total: 2 });
    api.respond('POST', actionPath, receipt, 202);
    await page.goto('/media-organize');
    await rows(page).first().getByRole('checkbox').check();
    await page.getByTestId('organize-selected').click();
    if (mode === 'rescrape') {
      await page.getByTestId('organize-mode').click();
      await page.locator('.v-overlay--active').getByRole('option', { name: '重新刮削', exact: true }).click();
    }
    const dialog = page.locator('.v-overlay--active').filter({ has: page.getByTestId('organize-confirm') });
    await expect(dialog.locator('.v-card-title')).toHaveText(mode === 'reprocess' ? '重新整理' : '重新刮削');
    await expect(dialog.locator('[data-native-dialog-close]')).toHaveAttribute('aria-label', '关闭整理确认');
    await expect(dialog.getByTestId('organize-confirm')).toHaveText('加入队列');
    await expect(dialog.getByTestId('organize-confirm')).toHaveClass(/v-btn--variant-tonal/);
    await expect(dialog.locator('.v-card-actions')).toHaveClass(/app-dialog-actions/);
    await expect(dialog.locator('.v-card-text')).toHaveCSS('padding', '24px');
    await expect(dialog.locator('.v-card-subtitle')).toHaveText('1 条记录，1 项媒体');
    await expect(dialog.getByTestId('organize-mode')).toContainText(mode === 'reprocess' ? '重新整理入库' : '重新刮削');
    await expect(dialog).toContainText(mode === 'reprocess' ? '重新执行完整入库流程' : '保留已有媒体信息、音轨和章节');
    await expect(dialog).toContainText('剧集只处理所选单集');
    await dialog.getByTestId('organize-confirm').click();
    await expect(page.getByTestId('organize-queue-dialog')).toBeVisible();
    await expect(page.getByTestId('organize-result')).toHaveCount(0);
    const posts = api.requests.filter(request => request.method === 'POST' && request.path === actionPath);
    expect(posts).toHaveLength(1);
    expect(posts[0].body).toEqual({ item_ids: ['episode-2'], history_ids: ['record-2'], mode });
  });
}

test('record operations freeze only that record while retaining unrelated batch selections', async ({ page, api }) => {
  api.respond('GET', historyPath, { items: records, total: 2 });
  api.respond('POST', actionPath, receipt, 202);
  await page.goto('/media-organize');
  await expect(rows(page)).toHaveCount(records.length);
  await rows(page).last().getByRole('checkbox').check();
  await expect(page.getByTestId('organize-selection-count')).toHaveText('1/2 项');
  await rows(page).first().getByRole('button', { name: '整理验证剧集 操作', exact: true }).click();
  await page.locator('.v-overlay--active').getByText('重新刮削', { exact: true }).click();
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-queue-dialog')).toBeVisible();
  const post = api.requests.find(request => request.method === 'POST' && request.path === actionPath);
  expect(post.body).toEqual({ item_ids: ['episode-2'], history_ids: ['record-2'], mode: 'rescrape' });
  await page.getByRole('button', { name: '关闭整理队列', exact: true }).click();
  await expect(rows(page).last().getByRole('checkbox')).toBeChecked();
  await expect(page.getByTestId('organize-selection-count')).toHaveText('1/2 项');
});

test('desktop records start at the list top without a default table spacer row', async ({ page, api }) => {
  api.respond('GET', historyPath, { items: records, total: 2 });
  await page.goto('/media-organize');
  await expect(rows(page).first()).toBeVisible();
  const list = await page.getByTestId('organize-items').locator('.v-table__wrapper').boundingBox();
  const first = await rows(page).first().boundingBox();
  expect(first.y - list.y).toBeLessThanOrEqual(4);
});

test('a new query resets list scroll while a refresh of the same page preserves the reading position', async ({ page, api }) => {
  const history = Array.from({ length: 100 }, (_, index) => ({ ...records[1], id: `history-${index + 1}`, item_id: `media-${index + 1}`, name: `记录${index + 1}` }));
  api.respond('GET', historyPath, request => ({ items: history.slice((Number(request.query.page) - 1) * 50, Number(request.query.page) * 50), total: 100 }));
  await page.goto('/media-organize');
  let scroll = page.getByTestId('organize-items').locator('.v-table__wrapper');
  await scroll.evaluate(element => { element.scrollTop = element.scrollHeight; });
  await expect(page.getByTestId('organize-items').getByRole('link', { name: '记录50', exact: true })).toBeVisible();
  const position = await scroll.evaluate(element => element.scrollTop);
  await page.getByRole('button', { name: '刷新入库记录', exact: true }).click();
  await expect(page.getByRole('button', { name: '刷新入库记录', exact: true })).not.toHaveClass(/v-btn--loading/);
  expect(await scroll.evaluate(element => element.scrollTop)).toBeCloseTo(position, 0);
  await page.getByTestId('organize-pagination').locator('.v-pagination__item').filter({ hasText: /^2$/ }).getByRole('button').click();
  await expect(page.getByTestId('organize-page-info')).toHaveText('51 - 100 / 100');
  scroll = page.getByTestId('organize-items').locator('.v-table__wrapper');
  expect(await scroll.evaluate(element => element.scrollTop)).toBe(0);
  await expect(page.getByTestId('organize-items').getByRole('link', { name: '记录51', exact: true })).toBeVisible();
  expect(api.requests.filter(request => request.path === historyPath).map(request => request.query.page)).toEqual(['1', '1', '2']);
});

const deleteDialog = page => page.locator('.v-overlay--active').filter({ has: page.getByTestId('organize-delete-confirm') });
const deletes = api => api.requests.filter(request => request.method === 'DELETE' && request.path === historyPath);

test('single-record deletion freezes that history while preserving an unrelated batch selection', async ({ page, api }) => {
  let current = records;
  api.respond('GET', historyPath, () => ({ items: current, total: current.length }));
  api.respond('DELETE', historyPath, request => {
    current = current.filter(item => !request.body.history_ids.includes(item.id));
    return { deleted_count: request.body.history_ids.length, deleted_ids: request.body.history_ids, message: '整理记录已删除' };
  });
  await page.goto('/media-organize');
  await rows(page).last().getByRole('checkbox').check();
  await rows(page).first().getByRole('button', { name: '整理验证剧集 操作', exact: true }).click();
  await page.locator('.v-overlay--active').getByText('删除', { exact: true }).click();
  const dialog = deleteDialog(page);
  await expect(dialog.locator('.v-card-title')).toHaveText('确认删除 1 条记录？');
  await expect(dialog.getByRole('button', { name: '仅删除整理记录', exact: true })).toBeVisible();
  await expect(dialog.locator('.v-list-item')).toHaveCount(0);
  await expect(dialog).not.toContainText('整理验证剧集');
  await expect(dialog).not.toContainText('排序验证电影');
  await dialog.getByTestId('organize-delete-confirm').click();
  await expect(rows(page)).toHaveCount(1);
  await expect(rows(page).first().getByRole('checkbox')).toBeChecked();
  await expect(page.getByTestId('organize-selection-count')).toHaveText('1/1 项');
  expect(deletes(api)).toHaveLength(1);
  expect(deletes(api)[0].body).toEqual({ history_ids: ['record-2'] });
  expect(api.requests.filter(request => request.method !== 'GET' && request.path !== historyPath)).toHaveLength(0);
});

test('batch deletion confirms the current-page count and preserves the earlier page history', async ({ page, api }) => {
  let deleted = false;
  api.respond('GET', historyPath, request => ({ items: deleted ? [records[0]] : [Number(request.query.page) === 1 ? records[0] : records[1]], total: deleted ? 1 : 100 }));
  api.respond('DELETE', historyPath, request => {
    deleted = true;
    return { deleted_count: 1, deleted_ids: request.body.history_ids };
  });
  await page.goto('/media-organize');
  await rows(page).first().getByRole('checkbox').check();
  await page.getByTestId('organize-pagination').locator('.v-pagination__item').filter({ hasText: /^2$/ }).getByRole('button').click();
  await expect(page.getByTestId('organize-selection-count')).toHaveText('0/1 项');
  await expect(page.getByTestId('organize-selected-delete')).toBeDisabled();
  await rows(page).first().getByRole('checkbox').check();
  await expect(page.getByTestId('organize-selection-count')).toHaveText('1/1 项');
  await page.getByTestId('organize-selected-delete').click();
  const dialog = deleteDialog(page);
  await expect(dialog.locator('.v-card-title')).toHaveText('确认删除 1 条记录？');
  await expect(dialog.locator('.v-list-item')).toHaveCount(0);
  await expect(dialog).not.toContainText('整理验证剧集');
  await expect(dialog).not.toContainText('排序验证电影');
  await dialog.getByTestId('organize-delete-confirm').click();
  await expect(page.getByTestId('organize-selection')).toHaveCount(0);
  await expect(page.getByTestId('organize-page-info')).toHaveText('1 - 1 / 1');
  await expect(rows(page).first()).toContainText('整理验证剧集');
  await expect(rows(page).first().getByRole('checkbox')).not.toBeChecked();
  expect(deletes(api)).toHaveLength(1);
  expect(deletes(api)[0].body).toEqual({ history_ids: ['record-1'] });
});

for (const viewport of [{ name: 'desktop', width: 1280, height: 720 }, { name: 'mobile', width: 390, height: 844 }]) {
  test(`${viewport.name} batch deletion confirms 50 records without listing them and cancellation preserves selection`, async ({ page, api }, testInfo) => {
    await page.setViewportSize({ width: viewport.width, height: viewport.height });
    let current = Array.from({ length: 51 }, (_, index) => ({ ...records[1], id: `history-${index + 1}`, name: `批量验证电影${index + 1}` }));
    const ids = current.slice(0, 50).map(item => item.id);
    api.respond('GET', historyPath, request => ({ items: current.slice((Number(request.query.page) - 1) * 50, Number(request.query.page) * 50), total: current.length }));
    api.respond('DELETE', historyPath, request => {
      current = current.filter(item => !request.body.history_ids.includes(item.id));
      return { deleted_count: request.body.history_ids.length, deleted_ids: request.body.history_ids };
    });
    await page.goto('/media-organize');
    const items = viewport.name === 'mobile' ? page.getByTestId('organize-items-mobile') : page.getByTestId('organize-items');
    await items.getByRole('checkbox').first().check();
    await page.getByRole('checkbox', { name: '全选本页', exact: true }).check();
    await expect(page.getByTestId('organize-selection-count')).toHaveText('50/50 项');
    await page.getByTestId('organize-selected-delete').click();
    const dialog = deleteDialog(page);
    await expect(dialog.locator('.v-card-title')).toHaveText('确认删除 50 条记录？');
    await expect(dialog.locator('.v-list-item, .v-virtual-scroll')).toHaveCount(0);
    await expect(dialog.getByRole('button')).toHaveCount(2);
    await expect(dialog.getByRole('button', { name: '仅删除整理记录', exact: true })).toBeInViewport({ ratio: 1 });
    await expect(dialog.getByRole('button', { name: '关闭删除确认', exact: true })).toBeInViewport({ ratio: 1 });
    await expect(dialog).not.toContainText('批量验证电影');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`media-organize-delete-${viewport.name}.png`) });
    await dialog.getByRole('button', { name: '关闭删除确认', exact: true }).click();
    await expect(dialog).toHaveCount(0);
    expect(deletes(api)).toHaveLength(0);
    await expect(page.getByTestId('organize-selection-count')).toHaveText('50/50 项');
    await page.getByTestId('organize-selected-delete').click();
    await deleteDialog(page).getByTestId('organize-delete-confirm').click();
    await expect(page.getByTestId('organize-page-info')).toHaveText('1 - 1 / 1');
    await expect(page.getByTestId('organize-selection')).toHaveCount(0);
    await expect(items).toContainText('批量验证电影51');
    expect(deletes(api)).toHaveLength(1);
    expect(deletes(api)[0].body).toEqual({ history_ids: ids });
    expect(api.requests.filter(request => request.method !== 'GET' && request.path !== historyPath)).toHaveLength(0);
  });
}

test('mobile unbound failure offers history deletion while reorganization remains disabled', async ({ page, api }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const failed = { ...records[0], item_id: null, status: 'failed' };
  let current = [failed];
  api.respond('GET', historyPath, () => ({ items: current, total: current.length }));
  api.respond('DELETE', historyPath, request => {
    current = [];
    return { deleted_count: 1, deleted_ids: request.body.history_ids };
  });
  await page.goto('/media-organize');
  await page.getByTestId('organize-items-mobile').getByRole('checkbox').check();
  await expect(page.getByTestId('organize-selected')).toBeDisabled();
  await expect(page.getByTestId('organize-selected-delete')).toBeEnabled();
  await page.getByTestId('organize-selected-delete').click();
  await deleteDialog(page).getByTestId('organize-delete-confirm').click();
  await expect(page.getByTestId('organize-page-info')).toHaveText('0 - 0 / 0');
  expect(deletes(api)[0].body).toEqual({ history_ids: [failed.id] });
  expect(api.requests.some(request => request.method === 'POST')).toBe(false);
});

test('an active-task deletion conflict retains the selection and shows the server reason', async ({ page, api }) => {
  api.respond('GET', historyPath, { items: records, total: 2 });
  api.respond('DELETE', historyPath, { error: '所选记录关联正在执行或等待恢复的任务' }, 409);
  await page.goto('/media-organize');
  await rows(page).first().getByRole('checkbox').check();
  await page.getByTestId('organize-selected-delete').click();
  await deleteDialog(page).getByTestId('organize-delete-confirm').click();
  await expect(page.getByTestId('organize-result')).toContainText('所选记录关联正在执行或等待恢复的任务');
  await expect(rows(page)).toHaveCount(2);
  await expect(rows(page).first().getByRole('checkbox')).toBeChecked();
  await expect(page.getByTestId('organize-selection-count')).toHaveText('1/2 项');
  expect(api.requests.filter(request => request.method === 'GET' && request.path === historyPath)).toHaveLength(1);
});

test('an incomplete deletion receipt preserves selection and reports that its result is unconfirmed', async ({ page, api }) => {
  api.respond('GET', historyPath, { items: records, total: 2 });
  api.respond('DELETE', historyPath, { deleted_count: 1, deleted_ids: [] });
  await page.goto('/media-organize');
  await rows(page).first().getByRole('checkbox').check();
  await page.getByTestId('organize-selected-delete').click();
  await deleteDialog(page).getByTestId('organize-delete-confirm').click();
  await expect(page.getByTestId('organize-result')).toContainText('整理记录删除结果尚未确认');
  await expect(rows(page).first().getByRole('checkbox')).toBeChecked();
  expect(api.requests.filter(request => request.method === 'GET' && request.path === historyPath)).toHaveLength(1);
});
