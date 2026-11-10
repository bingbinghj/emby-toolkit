import { test, expect } from './fixtures.js';

const themes = [
  { name: 'light', theme: 'light', glassEnabled: false },
  { name: 'dark', theme: 'dark', glassEnabled: false },
  { name: 'clear adaptive', theme: 'dark', glassEnabled: true, glassAppearance: 'clear', glassUIStyle: 'adaptive' },
  { name: 'clear transparent', theme: 'dark', glassEnabled: true, glassAppearance: 'clear', glassUIStyle: 'clear' },
  { name: 'tinted', theme: 'dark', glassEnabled: true, glassAppearance: 'tinted', glassUIStyle: 'adaptive' },
  { name: 'frosted', theme: 'dark', glassEnabled: true, glassAppearance: 'frosted', glassUIStyle: 'adaptive' },
];

const mediaResponse = request => {
  const start = request.query.letter ? 90 : Number(request.query.start_index || 0);
  return {
    library: { id: 'movies', name: '电影', collection_type: 'movies' }, total: 240,
    start_index: start, anchor_index: request.query.letter ? 92 : start,
    items: Array.from({ length: 30 }, (_, offset) => ({
      Id: String(start + offset), Name: `电影海报与圆角文字可读性 ${start + offset}`,
      Type: 'Movie', ProductionYear: 2026, ImageTags: { Primary: 'fixture' },
    })),
  };
};

for (const width of [1440, 390]) for (const theme of themes) {
  test(`${width}px ${theme.name} shows loaded Emby posters and keeps library controls fixed`, async ({ page, api }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme.theme, reducedMotion: 'reduce' });
    await page.addInitScript(settings => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({
      ...settings, glassQuality: 'css', radius: 'extra', layout: 'vertical',
    })), theme);
    api.respond('GET', '/api/media_center/libraries/movies/items', mediaResponse);
    await page.goto('/library/movies');
    const card = page.locator('[data-media-index="0"] .media-card');
    await expect.poll(() => card.locator('img.v-img__img').evaluate(image => image.complete && image.naturalWidth > 0)).toBe(true);
    await expect(card.locator('img.v-img__img')).toHaveCSS('opacity', '1');
    const geometry = await card.evaluate(element => {
      const card = element.getBoundingClientRect();
      const title = element.querySelector('.media-card-title').getBoundingClientRect();
      const meta = element.querySelector('.media-card-meta').getBoundingClientRect();
      return { radius: parseFloat(getComputedStyle(element).borderBottomLeftRadius),
        left: title.left - card.left, right: card.right - title.right, bottom: card.bottom - meta.bottom };
    });
    expect(geometry.left).toBeGreaterThanOrEqual(geometry.radius - 1);
    expect(geometry.right).toBeGreaterThanOrEqual(geometry.radius - 1);
    expect(geometry.bottom).toBeGreaterThanOrEqual(geometry.radius - 1);
    const controls = page.locator('.media-center-sticky-controls');
    const alphabet = page.getByRole('navigation', { name: '按首字母定位' });
    await expect(controls).toHaveCSS('position', 'fixed');
    await expect(alphabet).toHaveCSS('position', 'fixed');
    const beforeControls = await controls.boundingBox(), beforeAlphabet = await alphabet.boundingBox();
    await page.evaluate(() => window.scrollTo(0, 700));
    await expect.poll(() => page.evaluate(() => scrollY)).toBeGreaterThan(600);
    const afterControls = await controls.boundingBox(), afterAlphabet = await alphabet.boundingBox();
    expect(afterControls.y).toBeCloseTo(beforeControls.y, 0);
    expect(afterAlphabet.y).toBeCloseTo(beforeAlphabet.y, 0);
    await page.getByRole('button', { name: '定位首字母 M', exact: true }).click();
    const target = page.locator('[data-media-index="92"]');
    await expect(target).toBeInViewport();
    const targetBox = await target.boundingBox(), controlsBox = await controls.boundingBox();
    expect(targetBox.y).toBeGreaterThanOrEqual(controlsBox.y + controlsBox.height - 1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
  });
}

test('an Emby image failure keeps the error placeholder visible in glass mode', async ({ page, api }) => {
  await page.addInitScript(() => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({
    theme: 'dark', glassEnabled: true, glassQuality: 'css', glassAppearance: 'clear',
  })));
  api.respond('GET', '/api/media_center/libraries/movies/items', mediaResponse);
  await page.route('**/image_proxy/Items/0/**', route => route.fulfill({ status: 404, body: '' }));
  await page.goto('/library/movies');
  const card = page.locator('[data-media-index="0"] .media-card');
  await expect(card.locator('.poster-fallback .v-icon')).toBeVisible();
  await expect(card).not.toHaveClass(/media-card--image-loaded/);
  await expect(card.locator('.media-card-title')).toBeVisible();
});

test('dashboard omits the page banner and normal refresh buttons, retaining failure retry', async ({ page, api }) => {
  await page.goto('/dashboard');
  await expect(page.locator('.dashboard-card-title').first()).toBeVisible();
  await expect(page.getByRole('heading', { name: '仪表盘', exact: true })).toHaveCount(0);
  await expect(page.getByText('115 网盘与 Emby 媒体概览', { exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: /^刷新/ })).toHaveCount(0);
  api.respond('GET', '/api/dashboard/media', { error: 'Emby 暂不可用' }, 503);
  await page.reload();
  await expect(page.getByRole('button', { name: '重试最新入库 - Emby', exact: true })).toBeVisible();
});

test('a narrow library poster keeps long hover titles and summaries inside the artwork', async ({ page, api }) => {
  const name = '窄卡片长名称完整提示验证'.repeat(20);
  const overview = '超长媒体简介应限制显示行数并保留标题及编辑入口。'.repeat(30);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({
    theme: 'dark', glassEnabled: true, glassQuality: 'css', radius: 'extra', glassAppearance: 'clear',
  })));
  api.respond('GET', '/api/media_center/libraries/movies/items', request => {
    const response = mediaResponse(request);
    response.items[0] = { ...response.items[0], Name: name, Overview: overview };
    return response;
  });
  await page.goto('/library/movies');
  const card = page.locator('[data-media-index="0"] .media-card');
  await expect(card.locator('img.v-img__img')).toHaveCSS('opacity', '1');
  await page.locator('[data-media-index="0"]').hover();
  const detail = card.locator('.media-card-detail');
  await expect(detail).toBeVisible();
  await expect(detail.locator('.media-detail-title')).toHaveAttribute('title', name);
  await expect(detail.locator('.media-detail-title')).toHaveCSS('-webkit-line-clamp', '2');
  const bounds = await detail.evaluate(element => {
    const artwork = element.getBoundingClientRect();
    return [...element.children].map(child => {
      const box = child.getBoundingClientRect();
      return { top: box.top - artwork.top, bottom: artwork.bottom - box.bottom,
        left: box.left - artwork.left, right: artwork.right - box.right };
    });
  });
  for (const box of bounds) {
    expect(box.top).toBeGreaterThanOrEqual(0);
    expect(box.bottom).toBeGreaterThanOrEqual(0);
    expect(box.left).toBeGreaterThanOrEqual(0);
    expect(box.right).toBeGreaterThanOrEqual(0);
  }
  await expect(detail.getByText('打开媒体编辑', { exact: true })).toBeVisible();
});

async function librarySearch(page, width) {
  if (width < 1281) await page.getByRole('button', { name: '打开搜索当前媒体库', exact: true }).click();
  const input = page.getByRole('searchbox', { name: '搜索当前媒体库', exact: true });
  await expect(input).toBeVisible();
  return input;
}

const searchableResponse = request => request.query.search ? {
  library: { id: 'movies', name: '电影', collection_type: 'movies' }, total: 1, start_index: 0, anchor_index: 0,
  items: [{ Id: 'found-in-movies', Name: `库内匹配 ${request.query.search}`, Type: 'Movie', ProductionYear: 2026, ImageTags: { Primary: 'fixture' } }],
} : mediaResponse(request);

for (const width of [1440, 390]) {
  test(`${width}px navbar search queries only the current library and clears its alphabet anchor`, async ({ page, api }) => {
    await page.setViewportSize({ width, height: 900 });
    api.respond('GET', '/api/media_center/libraries/movies/items', searchableResponse);
    await page.goto('/library/movies');
    await expect(page.locator('.media-center-sticky-controls input')).toHaveCount(0);
    await page.getByRole('button', { name: '定位首字母 M', exact: true }).click();
    await expect(page.locator('[data-media-index="92"]')).toBeInViewport();
    const input = await librarySearch(page, width);
    await input.fill('阿凡达');
    await input.press('Enter');
    await expect(page.locator('.media-card-title')).toHaveText('库内匹配 阿凡达');
    await expect(page.getByRole('button', { name: '返回开头', exact: true })).toHaveAttribute('aria-pressed', 'true');
    const request = api.requests.find(request => request.query.search === '阿凡达');
    expect(request.path).toBe('/api/media_center/libraries/movies/items');
    expect(request.query.letter).toBeUndefined();
    expect(api.requests.filter(request => request.path === '/api/search_emby_library')).toHaveLength(0);
    const clearedInput = await librarySearch(page, width);
    await expect(clearedInput).toHaveValue('阿凡达');
    await page.getByRole('button', { name: '清空搜索', exact: true }).click();
    await expect(clearedInput).toHaveValue('');
    if (width < 1281) await page.getByRole('button', { name: '关闭搜索', exact: true }).click();
    await expect(page.locator('[data-media-index="0"] .media-card-title')).toHaveText('电影海报与圆角文字可读性 0');
    expect(api.requests.filter(request => request.path === '/api/search_emby_library')).toHaveLength(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
  });
}

test('navbar library search debounces typing and Enter cancels the delayed duplicate query', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  api.respond('GET', '/api/media_center/libraries/movies/items', searchableResponse);
  await page.goto('/library/movies');
  await expect(page.locator('[data-media-index="0"] .media-card-title')).toBeVisible();
  const input = await librarySearch(page, 1440);
  await input.focus();
  await page.clock.install();
  await page.clock.pauseAt(new Date(Date.now() + 1000));
  await input.fill('电影');
  await page.clock.runFor(319);
  expect(api.requests.filter(request => request.query.search === '电影')).toHaveLength(0);
  await page.clock.runFor(1);
  await expect(page.locator('.media-card-title')).toHaveText('库内匹配 电影');
  expect(api.requests.filter(request => request.query.search === '电影')).toHaveLength(1);
  await input.fill('剧集');
  await input.press('Enter');
  await expect(page.locator('.media-card-title')).toHaveText('库内匹配 剧集');
  await page.clock.runFor(500);
  expect(api.requests.filter(request => request.query.search === '剧集')).toHaveLength(1);
});
