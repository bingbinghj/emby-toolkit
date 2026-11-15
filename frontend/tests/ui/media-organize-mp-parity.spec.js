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

test('MP header, virtual record cards, compact selection and single-page pagination remain separate', async ({ page, api }) => {
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
  await expect(selection.getByRole('button', { name: '重新整理', exact: true })).toBeVisible();
  await expect(selection.getByRole('button', { name: '重新刮削', exact: true })).toBeVisible();
  await expect(selection.getByRole('checkbox', { name: '全选本页', exact: true })).toBeVisible();
  await expect(selection.locator('.v-chip')).toHaveCount(0);
  await selection.getByRole('button', { name: '已选记录', exact: true }).click();
  await expect(selection).toContainText('整理验证剧集');
  await selection.getByRole('button', { name: '已选记录', exact: true }).click();
  await expect(page.getByTestId('organize-footer')).toBeInViewport({ ratio: 1 });
  const footer = await page.getByTestId('organize-footer').boundingBox();
  const queue = await page.getByTestId('organize-queue-open').boundingBox();
  expect(queue.y + queue.height).toBeLessThanOrEqual(footer.y - 20);
  expect(1280 - queue.x - queue.width).toBeGreaterThanOrEqual(30);
  await page.screenshot({ path: '/root/emby-toolkit-followup-20261115/ui/media-organize-mp-desktop.png' });
  expect(api.requests.filter(request => request.method === 'POST' && request.path === actionPath)).toHaveLength(0);
});

for (const mode of ['reprocess', 'rescrape']) {
  test(`the ${mode} selected action opens the matching MP-style form and submits the exact episode`, async ({ page, api }) => {
    api.respond('GET', historyPath, { items: records, total: 2 });
    api.respond('POST', actionPath, receipt, 202);
    await page.goto('/media-organize');
    await rows(page).first().getByRole('checkbox').check();
    await page.getByTestId(mode === 'reprocess' ? 'organize-selected' : 'organize-selected-rescrape').click();
    const dialog = page.locator('.v-overlay--active').filter({ has: page.getByTestId('organize-confirm') });
    await expect(dialog.locator('.v-card-subtitle')).toHaveText('1 条记录，1 项媒体');
    await expect(dialog.getByTestId('organize-mode')).toContainText(mode === 'reprocess' ? '重新整理入库' : '重新刮削');
    await expect(dialog).toContainText(mode === 'reprocess' ? '重新执行完整入库流程' : '保留已有媒体信息、音轨和章节');
    await expect(dialog).toContainText('剧集只处理所选单集');
    await dialog.getByTestId('organize-confirm').click();
    await expect(page.getByTestId('organize-queue-dialog')).toBeVisible();
    const posts = api.requests.filter(request => request.method === 'POST' && request.path === actionPath);
    expect(posts).toHaveLength(1);
    expect(posts[0].body).toEqual({ item_ids: ['episode-2'], history_ids: ['record-2'], mode });
  });
}

test('record operations freeze only that record while retaining unrelated batch selections', async ({ page, api }) => {
  api.respond('GET', historyPath, { items: records, total: 2 });
  api.respond('POST', actionPath, receipt, 202);
  await page.goto('/media-organize');
  await rows(page).last().getByRole('checkbox').check();
  await rows(page).first().getByRole('button', { name: '整理验证剧集 操作', exact: true }).click();
  await page.locator('.v-overlay--active').getByText('重新刮削', { exact: true }).click();
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-queue-dialog')).toBeVisible();
  const post = api.requests.find(request => request.method === 'POST' && request.path === actionPath);
  expect(post.body).toEqual({ item_ids: ['episode-2'], history_ids: ['record-2'], mode: 'rescrape' });
  await page.getByRole('button', { name: '关闭整理队列', exact: true }).click();
  await expect(rows(page).last().getByRole('checkbox')).toBeChecked();
  await expect(page.getByTestId('organize-selection')).toContainText('已选择 1 条记录');
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
