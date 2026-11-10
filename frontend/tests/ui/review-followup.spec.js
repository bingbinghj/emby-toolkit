import { test, expect } from './fixtures.js';

const movie = { item_id: '101', item_name: '浏览器验证电影', item_type: 'Movie', failed_at: '2026-11-10T02:00:00Z', reason: '媒体恢复回读失败', score: 1 };
const lastGlyphUnclipped = locator => locator.evaluate(element => {
  const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
  let node, last;
  while ((node = walker.nextNode())) if (node.textContent.trim()) last = node;
  if (!last) return false;
  const index = last.textContent.trimEnd().length - 1, range = document.createRange();
  range.setStart(last, index); range.setEnd(last, index + 1);
  const glyph = range.getBoundingClientRect();
  for (let ancestor = element; ancestor && ancestor !== document.body; ancestor = ancestor.parentElement) {
    const style = getComputedStyle(ancestor);
    if (/(hidden|clip|auto|scroll)/.test(style.overflowX + style.overflowY)) {
      const clip = ancestor.getBoundingClientRect();
      if (glyph.left < clip.left - 1 || glyph.right > clip.right + 1 || glyph.top < clip.top - 1 || glyph.bottom > clip.bottom + 1) return false;
    }
  }
  return glyph.width > 0 && glyph.height > 0;
});

for (const width of [390, 1440]) {
  test(`${width}px manual processing has complete failure text, no scores, and functional MP type filtering`, async ({ page, api }) => {
    await page.setViewportSize({ width, height: 900 });
    const reason = ('恢复文件存在，但原始媒体 JSON 与当前 Emby 章节回读尚未一致，请核对完整备份。\n').repeat(5) + '最终原因：保留备份等待重新处理。';
    api.respond('GET', '/api/review_items', { items: [{ ...movie, reason }], total_items: 1 });
    api.respond('POST', '/api/actions/reprocess_item/101', { message: '媒体恢复失败', failed: 1, failures: ['101：回读仍未通过'] }, 207);
    await page.goto('/review');
    const content = page.locator('.review-page');
    await expect(content.getByRole('heading')).toHaveCount(0);
    await expect(content.getByRole('columnheader', { name: '评分', exact: true })).toHaveCount(0);
    const copy = content.locator(width < 600 ? '.review-item-reason' : '.review-table-reason').first();
    await expect(copy).toContainText('最终原因：保留备份等待重新处理。');
    expect(await copy.evaluate(element => element.scrollHeight <= element.clientHeight + 1)).toBe(true);
    expect(await lastGlyphUnclipped(copy)).toBe(true);
    await content.getByRole('button', { name: '按媒体类型筛选', exact: true }).click();
    await page.locator('.v-overlay--active').getByText('电影', { exact: true }).click();
    await expect.poll(() => api.requests.some(request => request.path === '/api/review_items' && request.query.media_type === 'Movie')).toBe(true);
    expect(api.requests.some(request => request.method === 'POST')).toBe(false);
    await content.getByRole('button', { name: '重新处理', exact: true }).first().click();
    await page.locator('.v-overlay--active').getByRole('button', { name: '确认', exact: true }).click();
    await expect(content.locator('.v-alert')).toContainText('回读仍未通过');
    await expect(content.locator('.v-alert')).toContainText('媒体恢复失败');
    expect(api.requests.filter(request => request.path === '/api/actions/reprocess_item/101')).toHaveLength(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}

test('manual search follows MP delayed input, exposes real library results, and clears back to failures', async ({ page, api }) => {
  api.respond('GET', '/api/review_items', { items: [movie], total_items: 1 });
  api.respond('GET', '/api/search_emby_library', { items: [{ item_id: '202', item_name: '最近入库的电视剧', item_type: 'Episode', season_number: 2, episode_number: 5, library_name: '电视剧' }], total_items: 1 });
  await page.goto('/review');
  const content = page.locator('.review-page');
  await content.getByRole('textbox', { name: '搜索媒体', exact: true }).fill('最近入库');
  await expect(content).toContainText('最近入库的电视剧');
  expect(api.requests.filter(request => request.path === '/api/search_emby_library')).toHaveLength(1);
  expect(api.requests.find(request => request.path === '/api/search_emby_library').query.query).toBe('最近入库');
  await content.getByRole('button', { name: '搜索结果排序', exact: true }).click();
  await page.locator('.v-overlay--active').getByText('季 / 集数：从大到小', { exact: true }).click();
  await content.getByRole('textbox', { name: '搜索媒体', exact: true }).fill('');
  await expect(content).toContainText('媒体恢复回读失败');
  await expect(content).not.toContainText('最近入库的电视剧');
  expect(api.requests.some(request => request.method === 'POST')).toBe(false);
});
