import { test, expect, mediaDetails } from './fixtures.js';

for (const [kind, width] of [['Movie', 1440], ['Series', 1024], ['Season', 390], ['Episode', 768]]) {
  test(`current ${kind} rescrape remains readable at ${width}px and uses the shared queue`, async ({ page, api }) => {
    await page.setViewportSize({ width, height: 900 });
    api.respond('GET', '/api/media_for_editing/101', { ...mediaDetails, item_type: kind });
    api.respond('GET', '/api/media/101/extract_credits', { available: false });
    const row = { id: 'queue-current', item_id: '101', mode: 'rescrape', status: 'queued', item_json: { item_id: '101', name: '当前媒体' } };
    api.respond('POST', '/api/actions/reorganize', { mode: 'rescrape', queued_count: 1, duplicate_count: 0, queue_ids: [row.id], items: [row] }, 202);
    api.respond('GET', '/api/media_organize/queue', { items: [row], total: 1, page: 1, page_size: 100, queued_count: 1, running_count: 0 });
    await page.goto('/edit-media/101');
    const button = page.getByTestId('rescrape-current-media');
    await expect(button).toBeVisible();
    const bounds = await button.evaluate(element => {
      const content = element.querySelector('.v-btn__content');
      const range = document.createRange(); range.selectNodeContents(content);
      const text = range.getBoundingClientRect(), rect = element.getBoundingClientRect();
      return { textLeft: text.left, textRight: text.right, left: rect.left, right: rect.right, viewport: innerWidth, page: document.documentElement.scrollWidth };
    });
    expect(bounds.textLeft).toBeGreaterThanOrEqual(bounds.left);
    expect(bounds.textRight).toBeLessThanOrEqual(bounds.right);
    expect(bounds.page).toBeLessThanOrEqual(bounds.viewport);
    await button.click();
    const confirm = page.getByRole('dialog').filter({ hasText: '保留已有中文翻译' });
    await expect(confirm).toBeVisible();
    await expect(confirm.getByRole('button', { name: '取消', exact: true })).toBeVisible();
    await confirm.getByRole('button', { name: '确认', exact: true }).click();
    await expect(confirm).toHaveCount(0);
    await expect(page.getByTestId('organize-queue-dialog')).toBeVisible();
    expect(api.requests.filter(request => request.path === '/api/actions/reorganize').map(request => request.body)).toEqual([{ item_ids: ['101'], mode: 'rescrape' }]);
  });
}
