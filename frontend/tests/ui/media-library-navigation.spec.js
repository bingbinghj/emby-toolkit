import { test, expect } from './fixtures.js';

const library = { id: 'movies', name: '电影', collection_type: 'movies' };
function pageResponse(request) {
  const start = request.query.letter ? 90 : Number(request.query.start_index || 0);
  return {
    library, total: 240, start_index: start, anchor_index: request.query.letter ? 92 : start,
    items: Array.from({ length: 30 }, (_, offset) => ({
      Id: String(start + offset), Name: `媒体 ${start + offset}`, Type: 'Movie', ProductionYear: 2026,
      ImageTags: { Primary: 'fixture' },
    })),
  };
}

for (const width of [1440, 390]) {
  test(`${width}px letter navigation scrolls the actual document to the selected media`, async ({ page, api }) => {
    await page.setViewportSize({ width, height: 900 });
    api.respond('GET', '/api/media_center/libraries/movies/items', pageResponse);
    await page.goto('/library/movies', { waitUntil: 'domcontentloaded' });
    await expect(page.getByRole('heading', { name: '电影', exact: true })).toBeVisible();
    await page.getByRole('button', { name: '定位首字母 M', exact: true }).click();
    const target = page.locator('[data-media-index="92"]');
    await expect(target).toBeInViewport();
    await expect(page.getByRole('button', { name: '定位首字母 M', exact: true })).toHaveAttribute('aria-pressed', 'true');
    await expect.poll(() => page.evaluate(() => document.scrollingElement.scrollTop)).toBeGreaterThan(300);
    const targetBox = await target.boundingBox();
    const controlsBox = await page.locator('.media-center-sticky-controls').boundingBox();
    expect(targetBox.y).toBeGreaterThanOrEqual(controlsBox.y + controlsBox.height - 2);
    await page.getByRole('button', { name: '返回开头', exact: true }).click();
    await expect(page.locator('[data-media-index="0"]')).toBeInViewport();
    await expect.poll(() => page.evaluate(() => document.scrollingElement.scrollTop)).toBeLessThan(100);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
  });
}

test('letter query failure is visible and is not retried as a successful first page', async ({ page, api }) => {
  api.respond('GET', '/api/media_center/libraries/movies/items', pageResponse);
  await page.goto('/library/movies', { waitUntil: 'domcontentloaded' });
  await expect(page.locator('[data-media-index="0"]')).toBeVisible();
  api.respond('GET', '/api/media_center/libraries/movies/items', { error: 'Emby 字母定位查询失败' }, 502);
  await page.getByRole('button', { name: '定位首字母 M', exact: true }).click();
  await expect(page.getByText('Emby 字母定位查询失败', { exact: true })).toBeVisible();
  await expect(page.locator('[data-media-index="0"]')).toHaveCount(0);
  expect(api.requests.filter(request => request.query.letter === 'M')).toHaveLength(1);
});

test('an absent letter reports the fallback and keeps the first page available', async ({ page, api }) => {
  api.respond('GET', '/api/media_center/libraries/movies/items', request => request.query.letter
    ? { ...pageResponse({ query: {} }), anchor_found: false }
    : pageResponse(request));
  await page.goto('/library/movies', { waitUntil: 'domcontentloaded' });
  await page.getByRole('button', { name: '定位首字母 Z', exact: true }).click();
  await expect(page.getByText('没有可定位到 Z 的媒体，已返回媒体库开头', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '返回开头', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('[data-media-index="0"]')).toBeInViewport();
});
