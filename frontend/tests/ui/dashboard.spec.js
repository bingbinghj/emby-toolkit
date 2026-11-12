import { test, expect } from './fixtures.js';

const dashboard = {
  statistics: { movie_count: { count: 772 }, tv_count: { count: 208 }, episode_count: { count: 13781 } },
  recent: [{ id: '101', name: '最新入库电影', item_type: 'Movie', production_year: 2026, image_tag: 'poster-tag' }],
  libraries: [{ id: 'movies', name: '我的电影', collection_type: 'movies', image_tag: 'library-composite', item_count: 772 }], errors: {},
};
const space = { status: 'success', data: { valid: true, user_info: { rt_space_info: { all_total: { size: 1099511627776 }, all_use: { size: 549755813888 }, all_remain: { size: 549755813888 } } } } };
const modules = page => page.locator('[data-dashboard-module]');
const openSettings = async page => {
  await page.getByRole('button', { name: '调整布局', exact: true }).hover();
  await page.getByRole('button', { name: '仪表盘设置', exact: true }).click();
};
test('dashboard shows actual 115 space, Emby statistics and navigable Emby cover cards', async ({ page, api }) => {
  api.respond('GET', '/api/dashboard/media', { ...dashboard, recent: [...dashboard.recent, { id: 'episode-1', name: '最新剧集', item_type: 'Episode', image_tag: 'episode-screenshot', series_id: 'series-1', series_image_tag: 'series-poster' }] });
  api.respond('GET', '/api/p115/status', space);
  await page.goto('/dashboard');
  await expect(page.getByText('1 TB', { exact: true })).toBeVisible();
  await expect(page.getByText('已使用 50.0%', { exact: true })).toBeVisible();
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('772');
  await expect(page.getByTestId('dashboard-episode_count')).toHaveText('13,781');
  await expect(page.getByTestId('dashboard-music_count')).toHaveCount(0);
  await expect(page.locator('[data-dashboard-module="statistics"]').getByText('音乐', { exact: true })).toHaveCount(0);
  const recent = page.getByRole('link', { name: '编辑最新入库电影', exact: true });
  await expect(recent).toHaveAttribute('href', '/edit-media/101');
  await expect(recent.locator('img')).toHaveAttribute('src', /\/image_proxy\/Items\/101\/Images\/Primary.*poster-tag/);
  const library = page.getByRole('link', { name: '打开我的电影', exact: true });
  await expect(library).toHaveAttribute('href', '/library/movies');
  await library.scrollIntoViewIfNeeded();
  await expect(library.locator('img')).toHaveAttribute('src', /\/image_proxy\/Items\/movies\/Images\/Primary.*library-composite/);
  await expect.poll(() => library.locator('img').evaluate(image => image.complete && image.naturalWidth > 0)).toBe(true);
  const art = await library.locator('.v-responsive').first().boundingBox();
  expect(art.width / art.height).toBeCloseTo(16 / 9, 1);
  const episode = page.getByRole('link', { name: '编辑最新剧集', exact: true });
  await expect(episode).toHaveAttribute('href', '/edit-media/episode-1');
  await expect(episode.locator('img')).toHaveAttribute('src', /\/image_proxy\/Items\/series-1\/Images\/Primary.*series-poster/);
  await recent.click();
  await expect(page).toHaveURL(/\/edit-media\/101$/);
});

test('MP content settings save visibility after confirmation and default layout restores it', async ({ page }) => {
  await page.goto('/dashboard');
  await expect(modules(page)).toHaveCount(4);
  await openSettings(page);
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('button', { name: '媒体统计', exact: true }).click();
  await expect(dialog.getByRole('button', { name: '媒体统计', exact: true })).toHaveAttribute('aria-pressed', 'false');
  await expect(modules(page)).toHaveCount(4);
  await dialog.getByRole('button', { name: '保存', exact: true }).click();
  await expect(modules(page)).toHaveCount(3);
  await expect(page.locator('[data-dashboard-module="statistics"]')).toHaveCount(0);
  await page.reload();
  await expect(modules(page)).toHaveCount(3);
  await page.getByRole('button', { name: '调整布局', exact: true }).click();
  await page.getByRole('button', { name: '完成调整', exact: true }).hover();
  await page.getByRole('button', { name: '恢复默认布局', exact: true }).click();
  await page.getByRole('button', { name: '完成调整', exact: true }).click();
  await expect(modules(page)).toHaveCount(4);
});

test('MoviePilot-style reload failures preserve media snapshots and report unknown 115 space with retry notices', async ({ page, api }) => {
  api.respond('GET', '/api/dashboard/media', dashboard);
  api.respond('GET', '/api/p115/status', space);
  await page.goto('/dashboard');
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('772');
  api.respond('GET', '/api/dashboard/media', { error: 'Emby 暂不可用' }, 503);
  api.respond('GET', '/api/p115/status', { data: { valid: false, msg: '115 凭证失效' } });
  await page.reload();
  await expect(page.locator('[data-dashboard-module="recent"]')).toContainText('Emby 暂不可用');
  await expect(page.locator('[data-dashboard-module="recent"]')).toContainText('显示上次成功数据');
  await expect(page.locator('[data-dashboard-module="libraries"]')).toContainText('显示上次成功数据');
  await expect(page.locator('[data-dashboard-module="storage"]')).toContainText('115 凭证失效');
  await expect(page.getByText('1 TB', { exact: true })).toHaveCount(0);
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('772');
  await expect(page.getByTestId('dashboard-episode_count')).toHaveText('13,781');
  await expect(page.getByRole('link', { name: '编辑最新入库电影', exact: true })).toHaveCount(1);
  await expect(page.getByRole('link', { name: '打开我的电影', exact: true })).toHaveCount(1);
  await expect(page.getByRole('button', { name: '重试最新入库 - Emby', exact: true })).toHaveCount(1);
  await expect(page.getByText('暂无最近入库记录', { exact: true })).toHaveCount(0);
  await expect(page.getByText('暂无媒体库数据', { exact: true })).toHaveCount(0);
  await page.reload();
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('772');
  await expect(page.getByRole('link', { name: '打开我的电影', exact: true })).toHaveCount(1);
  await expect(page.locator('[data-dashboard-module="storage"]')).toContainText('空间信息未知');
  await expect(page.getByText('1 TB', { exact: true })).toHaveCount(0);
});

test('a failed widget preserves its whole snapshot while independent successful widgets update atomically', async ({ page, api }) => {
  api.respond('GET', '/api/dashboard/media', dashboard);
  await page.goto('/dashboard');
  await expect(page.getByTestId('dashboard-episode_count')).toHaveText('13,781');
  await expect(page.getByRole('link', { name: '编辑最新入库电影', exact: true })).toHaveCount(1);
  const fresh = { ...dashboard, statistics: { ...dashboard.statistics, movie_count: { count: 773 }, episode_count: { count: null, error: '剧集统计读取失败' } }, recent: null, libraries: [{ ...dashboard.libraries[0], name: '已刷新的媒体库', image_tag: 'new-library-composite' }], errors: { recent: '最新入库读取失败' } };
  api.respond('GET', '/api/dashboard/media', fresh);
  await page.reload();
  await expect(page.getByTestId('dashboard-episode_count')).toHaveText('13,781');
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('772');
  await expect(page.locator('[data-dashboard-module="statistics"]')).toContainText('剧集统计读取失败');
  await expect(page.getByRole('link', { name: '编辑最新入库电影', exact: true })).toHaveCount(1);
  await expect(page.getByRole('link', { name: '打开我的电影', exact: true })).toHaveCount(0);
  await expect(page.getByRole('link', { name: '打开已刷新的媒体库', exact: true })).toHaveCount(1);
  const nextRecent = [{ ...dashboard.recent[0], name: '刷新后的入库电影' }];
  api.respond('GET', '/api/dashboard/media', { ...fresh, statistics: { ...dashboard.statistics, movie_count: { count: 773 } }, recent: nextRecent, libraries: null, errors: { libraries: '媒体库读取失败' } });
  await page.reload();
  await expect(page.getByRole('link', { name: '打开已刷新的媒体库', exact: true })).toHaveCount(1);
  await expect(page.locator('[data-dashboard-module="libraries"]')).toContainText('媒体库读取失败');
  await expect(page.getByRole('link', { name: '编辑刷新后的入库电影', exact: true })).toHaveCount(1);
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('773');
  api.respond('GET', '/api/dashboard/media', { ...dashboard, statistics: null, libraries: [{ ...dashboard.libraries[0], name: '再次刷新的媒体库' }], errors: { statistics: '媒体统计读取失败' } });
  await page.reload();
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('773');
  await expect(page.getByTestId('dashboard-tv_count')).toHaveText('208');
  await expect(page.locator('[data-dashboard-module="statistics"]')).toContainText('媒体统计读取失败');
  await expect(page.getByRole('link', { name: '编辑最新入库电影', exact: true })).toHaveCount(1);
  await expect(page.getByRole('link', { name: '打开再次刷新的媒体库', exact: true })).toHaveCount(1);
});

test('unknown capacities and failed individual statistics stay unknown', async ({ page, api }) => {
  api.respond('GET', '/api/p115/status', { data: { valid: true, user_info: { rt_space_info: { all_remain: { size_format: '3 TB' } } } } });
  api.respond('GET', '/api/dashboard/media', { ...dashboard, statistics: { ...dashboard.statistics, episode_count: { count: null, error: '统计未返回' } } });
  await page.goto('/dashboard');
  await expect(page.getByTestId('dashboard-episode_count')).toHaveText('—');
  await expect(page.locator('[data-dashboard-module="storage"]')).toContainText('可用 3 TB / 总容量 未知');
  await expect(page.locator('[data-dashboard-module="storage"]')).toContainText('使用比例未知');
});

test('a first read failure shows failure and retry instead of confirmed empty media', async ({ page, api }) => {
  api.respond('GET', '/api/dashboard/media', { error: 'Emby 首次读取失败' }, 503);
  await page.goto('/dashboard');
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('—');
  await expect(page.locator('[data-dashboard-module="recent"]')).toContainText('尚未成功读取，请重试');
  await expect(page.getByText('暂无最近入库记录', { exact: true })).toHaveCount(0);
  api.respond('GET', '/api/dashboard/media', dashboard);
  await page.getByRole('button', { name: '重试最新入库 - Emby', exact: true }).click();
  await expect(page.getByRole('link', { name: '编辑最新入库电影', exact: true })).toHaveCount(1);
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('772');
  await expect(page.locator('[data-dashboard-module="recent"] .v-alert')).toHaveCount(0);
});

test('confirmed empty snapshots survive a failed reload and expire after 24 hours', async ({ page, api }) => {
  api.respond('GET', '/api/dashboard/media', { ...dashboard, recent: [], libraries: [] });
  await page.goto('/dashboard');
  await expect(page.getByText('暂无最近入库记录', { exact: true })).toBeVisible();
  await expect(page.getByText('暂无媒体库数据', { exact: true })).toHaveCount(1);
  api.respond('GET', '/api/dashboard/media', { error: '刷新失败' }, 503);
  await page.reload();
  await expect(page.locator('[data-dashboard-module="recent"]')).toContainText('显示上次成功数据');
  await expect(page.getByText('暂无最近入库记录', { exact: true })).toHaveCount(1);
  await expect(page.getByText('暂无媒体库数据', { exact: true })).toHaveCount(1);
  await page.evaluate(() => {
    for (const key of Object.keys(localStorage).filter(key => key.startsWith('ETK_DASHBOARD_SNAPSHOT_V1:'))) {
      const snapshot = JSON.parse(localStorage.getItem(key));
      snapshot.savedAt = Date.now() - 24 * 60 * 60 * 1000 - 1000;
      localStorage.setItem(key, JSON.stringify(snapshot));
    }
  });
  await page.reload();
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('—');
  await expect(page.locator('[data-dashboard-module="recent"]')).toContainText('尚未成功读取，请重试');
  await expect(page.getByText('暂无最近入库记录', { exact: true })).toHaveCount(0);
  await expect(page.getByText('暂无媒体库数据', { exact: true })).toHaveCount(0);
});

test('a different login cannot read another users media snapshots', async ({ page, api }) => {
  await page.addInitScript(() => {
    localStorage.setItem('ETK_DASHBOARD_SNAPSHOT_V1:other-user:recent', JSON.stringify({ savedAt: Date.now(), value: [{ id: 'other-media', name: '其他账户媒体' }] }));
  });
  api.respond('GET', '/api/dashboard/media', { error: '读取失败' }, 503);
  await page.goto('/dashboard');
  await expect(page.locator('[data-dashboard-module="recent"]')).toContainText('尚未成功读取，请重试');
  await expect(page.getByText('其他账户媒体', { exact: true })).toHaveCount(0);
});

test('a late response from an unmounted dashboard cannot write a successful snapshot', async ({ page, api }) => {
  const pending = api.defer('GET', '/api/dashboard/media');
  await page.goto('/dashboard');
  await pending.started;
  const drawer = page.locator('.layout-vertical-nav');
  const review = drawer.getByText('手动处理', { exact: true });
  if (!await review.isVisible()) await drawer.getByText('整理', { exact: true }).click();
  await review.click();
  await expect(page).toHaveURL(/\/review$/);
  const received = page.waitForResponse('**/api/dashboard/media');
  pending.release(dashboard);
  await received;
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(resolve)));
  expect(await page.evaluate(() => Object.keys(localStorage).filter(key => key.startsWith('ETK_DASHBOARD_SNAPSHOT_V1:')))).toEqual([]);
});

test('mobile dashboard and MP content settings remain within the viewport', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/dashboard');
  await expect(modules(page)).toHaveCount(4);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await openSettings(page);
  await page.getByRole('dialog').getByRole('button', { name: '媒体统计', exact: true }).click();
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await expect(page.locator('[data-dashboard-module="statistics"]')).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});

test('my media wraps in rows with at most seven Emby covers and actual count badges', async ({ page, api }) => {
  api.respond('GET', '/api/dashboard/media', { ...dashboard, libraries: Array.from({ length: 11 }, (_, index) => ({ id: `library-${index}`, name: `Emby 媒体库 ${index + 1}`, collection_type: 'movies', image_tag: `composite-${index}`, item_count: index === 0 ? 0 : 100 + index })) });
  for (const [width, columns] of [[1920, 7], [1440, 5], [390, 2]]) {
    await page.setViewportSize({ width, height: 1000 });
    await page.goto('/dashboard');
    const row = page.getByRole('region', { name: '我的媒体库列表', exact: true });
    await expect(row.getByRole('link')).toHaveCount(11);
    const metrics = await row.evaluate(element => {
      const cards = [...element.querySelectorAll('.dashboard-library-card')];
      const art = cards[0].querySelector('.v-responsive').getBoundingClientRect();
      const title = getComputedStyle(cards[0].querySelector('.dashboard-library-title'));
      return { scrollWidth: element.scrollWidth, clientWidth: element.clientWidth, ratio: art.width / art.height, rowSize: cards.filter(card => Math.abs(card.getBoundingClientRect().top - cards[0].getBoundingClientRect().top) < 1).length, secondTop: cards.at(-1).getBoundingClientRect().top, firstTop: cards[0].getBoundingClientRect().top, weight: title.fontWeight };
    });
    expect(metrics.ratio).toBeCloseTo(16 / 9, 1);
    expect(metrics.rowSize).toBe(columns);
    expect(metrics.secondTop).toBeGreaterThan(metrics.firstTop);
    expect(metrics.weight).toBe('700');
    expect(metrics.scrollWidth).toBeLessThanOrEqual(metrics.clientWidth + 1);
    await expect(row.locator('.dashboard-library-count').first()).toHaveText('0');
    await expect(row.locator('.dashboard-library-count').nth(1)).toHaveText('101');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  }
});

test('module drag handles become available only during layout editing', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 1100 });
  await page.goto('/dashboard');
  await expect(page.locator('.dashboard-drag-handle')).toHaveCount(0);
  await page.getByRole('button', { name: '调整布局', exact: true }).click();
  await expect(page.locator('.dashboard-drag-handle')).toHaveCount(4);
  const library = page.getByRole('button', { name: '拖动我的媒体库', exact: true });
  const storage = page.locator('[data-dashboard-module="storage"]');
  const title = page.locator('[data-dashboard-module="libraries"] .dashboard-card-title');
  const titleBounds = await title.boundingBox();
  const originalY = await page.locator('[data-dashboard-module="libraries"]').getAttribute('gs-y');
  await page.mouse.move(titleBounds.x + 12, titleBounds.y + 12);
  await page.mouse.down();
  await page.mouse.move(titleBounds.x + 12, titleBounds.y - 80, { steps: 12 });
  await page.mouse.up();
  await expect(page.locator('[data-dashboard-module="libraries"]')).toHaveAttribute('gs-y', originalY);
  const source = await library.boundingBox();
  const target = await storage.boundingBox();
  await page.mouse.move(source.x + source.width / 2, source.y + source.height / 2);
  await page.mouse.down();
  await page.mouse.move(source.x + source.width / 2, source.y - 12, { steps: 3 });
  await page.mouse.move(source.x + source.width / 2, target.y + 20, { steps: 20 });
  await page.mouse.up();
  await expect.poll(() => page.locator('[data-dashboard-module="libraries"]').getAttribute('gs-y')).toBe('0');
  expect(await page.evaluate(() => localStorage.getItem('etk-dashboard-layout-v2:ui-test-user:desktop'))).toBeNull();
  await page.getByRole('button', { name: '完成调整', exact: true }).click();
  await expect.poll(() => page.evaluate(() => JSON.parse(localStorage.getItem('etk-dashboard-layout-v2:ui-test-user:desktop')).items.libraries.y)).toBe(0);
  await expect(page.locator('.dashboard-drag-handle')).toHaveCount(0);
});


test('latest series use theme colors and keep long titles inside safe rounded insets', async ({ page, api }, testInfo) => {
  const name = '这是完整的最新入库电视剧名称，需要两行显示以避免被圆角裁切';
  api.respond('GET', '/api/dashboard/media', { ...dashboard, recent: [{ id: 'latest-series', name, item_type: 'Series', production_year: 2026, image_tag: 'latest-series-poster' }] });
  await page.goto('/dashboard');
  const card = page.getByRole('link', { name: `编辑${name}`, exact: true });
  await expect(card).toHaveAttribute('href', '/edit-media/latest-series');
  await expect(card.locator('.dashboard-poster-title')).toHaveAttribute('title', name);
  await expect(card.locator('.dashboard-poster-subtitle')).toHaveText('2026');
  await expect(card.locator('.dashboard-media-type')).toHaveClass(/bg-primary/);
  await card.hover();
  await expect(card.locator('.dashboard-poster-overlay')).toHaveCSS('opacity', '1');
  const geometry = await card.evaluate(element => {
    const image = element.querySelector('.dashboard-poster-image').getBoundingClientRect();
    const overlay = element.querySelector('.dashboard-poster-overlay');
    const title = element.querySelector('.dashboard-poster-title').getBoundingClientRect();
    const bounds = element.getBoundingClientRect();
    const themeForeground = getComputedStyle(element).getPropertyValue('--v-theme-on-surface').split(',').map(value => value.trim()).join(', ');
    return { imageBottom: image.bottom, titleBottom: title.bottom, titleLeft: title.left, titleRight: title.right, cardBottom: bounds.bottom, cardLeft: bounds.left, cardRight: bounds.right, gradient: getComputedStyle(overlay).backgroundImage, color: getComputedStyle(element.querySelector('.dashboard-poster-title')).color, themeForeground: `rgb(${themeForeground})`, radius: getComputedStyle(element).borderRadius };
  });
  expect(geometry.titleBottom).toBeLessThanOrEqual(geometry.cardBottom - 20);
  expect(geometry.titleLeft).toBeGreaterThanOrEqual(geometry.cardLeft + 8);
  expect(geometry.titleRight).toBeLessThanOrEqual(geometry.cardRight - 8);
  expect(Math.abs(geometry.imageBottom - geometry.cardBottom)).toBeLessThanOrEqual(2);
  expect(geometry.gradient).toContain('linear-gradient');
  expect(geometry.color).toBe(geometry.themeForeground);
  expect(parseFloat(geometry.radius)).toBeGreaterThan(0);
  await page.screenshot({ path: testInfo.outputPath('latest-long-title.png'), fullPage: true });
  await expect(page.locator('.dashboard-fab-stack')).toHaveCSS('position', 'fixed');
  const fab = await page.getByRole('button', { name: '调整布局', exact: true }).boundingBox();
  const viewport = page.viewportSize();
  expect(viewport.width - fab.x - fab.width).toBeLessThanOrEqual(24);
  expect(viewport.height - fab.y - fab.height).toBeLessThanOrEqual(24);
});

test.describe('touch dashboard controls', () => {
  test.use({ isMobile: true, hasTouch: true, viewport: { width: 390, height: 844 } });
  test('first tap expands the compact MP action stack before an action runs', async ({ page }) => {
    await page.goto('/dashboard');
    await expect(page.locator('.dashboard-poster-overlay').first()).toHaveCSS('opacity', '1');
    const edit = page.getByRole('button', { name: '调整布局', exact: true });
    await edit.tap();
    await expect(page.locator('.dashboard-fab-stack')).toHaveClass(/is-expanded/);
    await expect(page.locator('.dashboard-drag-handle')).toHaveCount(0);
    await page.getByRole('button', { name: '仪表盘设置', exact: true }).tap();
    await expect(page.getByRole('dialog')).toContainText('仪表盘设置');
    await page.getByRole('button', { name: '保存', exact: true }).tap();
    await expect(page.locator('.dashboard-fab-stack')).not.toHaveClass(/is-expanded/);
    await page.getByRole('link', { name: '编辑浏览器验证电影', exact: true }).tap();
    await expect(page).toHaveURL(/\/edit-media\/101$/);
  });
});


test('GridStack resizing persists width and height while device profiles keep independent layouts', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 1100 });
  await page.goto('/dashboard');
  const grid = page.locator('.dashboard-grid');
  const storage = page.locator('[data-dashboard-module="storage"]');
  await expect(grid).toHaveAttribute('data-layout-profile', 'desktop');
  await expect(storage).toHaveAttribute('gs-w', '4');
  await page.getByRole('button', { name: '调整布局', exact: true }).click();
  await storage.hover();
  const handle = storage.locator('.ui-resizable-se');
  await expect(handle).toBeVisible();
  const origin = await handle.boundingBox();
  await page.mouse.move(origin.x + origin.width / 2, origin.y + origin.height / 2);
  await page.mouse.down();
  await page.mouse.move(origin.x + origin.width / 2 + 190, origin.y + origin.height / 2 + 80, { steps: 20 });
  await page.mouse.up();
  await expect.poll(async () => Number(await storage.getAttribute('gs-w'))).toBeGreaterThan(4);
  const size = { w: await storage.getAttribute('gs-w'), h: await storage.getAttribute('gs-h') };
  await page.getByRole('button', { name: '完成调整', exact: true }).click();
  await page.reload();
  await expect(storage).toHaveAttribute('gs-w', size.w);
  await expect(storage).toHaveAttribute('gs-h', size.h);
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(grid).toHaveAttribute('data-layout-profile', 'mobile');
  await expect.poll(async () => Number(await storage.getAttribute('gs-w') || 1)).toBe(1);
  await expect.poll(async () => {
    const widgetBounds = await storage.boundingBox(), gridBounds = await grid.boundingBox();
    return Math.abs(widgetBounds.width - gridBounds.width);
  }).toBeLessThanOrEqual(1);
  await openSettings(page);
  await page.getByRole('dialog').getByRole('button', { name: '媒体统计', exact: true }).click();
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await expect(page.locator('[data-dashboard-module="statistics"]')).toHaveCount(0);
  await page.setViewportSize({ width: 1440, height: 1100 });
  await expect(grid).toHaveAttribute('data-layout-profile', 'desktop');
  await expect(storage).toHaveAttribute('gs-w', size.w);
  await expect(page.locator('[data-dashboard-module="statistics"]')).toHaveCount(1);
  await page.setViewportSize({ width: 800, height: 1100 });
  await expect(grid).toHaveAttribute('data-layout-profile', 'tablet');
  await expect(storage).toHaveAttribute('gs-w', '2');
});

test('legacy visibility and ordering migrate into the per-user GridStack profile', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('etk-dashboard-layout-v1', JSON.stringify({ order: ['libraries', 'storage', 'statistics', 'recent'], hidden: ['recent'] })));
  await page.goto('/dashboard');
  await expect(modules(page)).toHaveCount(3);
  await expect(modules(page).first()).toHaveAttribute('data-dashboard-module', 'libraries');
  await expect(modules(page).first()).toHaveAttribute('gs-y', '0');
  await openSettings(page);
  await page.getByRole('dialog').getByRole('button', { name: '媒体统计', exact: true }).click();
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await page.reload();
  await expect(modules(page)).toHaveCount(2);
  await expect(modules(page).first()).toHaveAttribute('data-dashboard-module', 'libraries');
  const saved = await page.evaluate(() => JSON.parse(localStorage.getItem('etk-dashboard-layout-v2:ui-test-user:desktop')));
  expect(saved.hidden.sort()).toEqual(['recent', 'statistics']);
});


const profile = (overrides = {}) => ({ order: ['storage', 'statistics', 'recent', 'libraries'], hidden: [], items: {}, updatedAt: 1, ...overrides });

test('server dashboard preferences load on a fresh browser without writing default preferences back', async ({ page, api }) => {
  api.respond('GET', '/api/dashboard/layout/desktop', { layout: profile({ order: ['libraries', 'storage', 'statistics', 'recent'], hidden: ['statistics'], items: { libraries: { x: 0, y: 0, w: 12, h: 20, manualHeight: false } } }) });
  await page.goto('/dashboard');
  await expect(modules(page)).toHaveCount(3);
  await expect(modules(page).first()).toHaveAttribute('data-dashboard-module', 'libraries');
  await expect(modules(page).first()).toHaveAttribute('gs-y', '0');
  expect(api.requests.filter(request => request.method === 'POST' && request.path.startsWith('/api/dashboard/layout/'))).toEqual([]);
});

test('newer confirmed local dashboard preferences synchronize instead of being overwritten by old server preferences', async ({ page, api }) => {
  await page.addInitScript(() => localStorage.setItem('etk-dashboard-layout-v2:ui-test-user:desktop', JSON.stringify({ order: ['storage', 'statistics', 'recent', 'libraries'], hidden: ['statistics'], items: {}, updatedAt: Date.now() })));
  api.respond('GET', '/api/dashboard/layout/desktop', { layout: profile() });
  await page.goto('/dashboard');
  await expect(page.locator('[data-dashboard-module="statistics"]')).toHaveCount(0);
  await expect.poll(() => api.requests.filter(request => request.method === 'POST' && request.path === '/api/dashboard/layout/desktop').length).toBeGreaterThan(0);
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/dashboard/layout/desktop').at(-1).body.layout.hidden).toEqual(['statistics']);
});

test('dashboard preference saves run serially and a failed read cannot replace a confirmed local layout', async ({ page, api }) => {
  const first = api.defer('POST', '/api/dashboard/layout/desktop');
  await page.goto('/dashboard');
  await openSettings(page);
  await page.getByRole('dialog').getByRole('button', { name: '媒体统计', exact: true }).click();
  await page.getByRole('button', { name: '保存', exact: true }).click();
  const firstRequest = await first.started;
  await expect(page.locator('[data-dashboard-module="statistics"]')).toHaveCount(0);
  await openSettings(page);
  await page.getByRole('dialog').getByRole('button', { name: '存储空间', exact: true }).click();
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await expect(modules(page)).toHaveCount(2);
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/dashboard/layout/desktop')).toHaveLength(1);
  api.respond('POST', '/api/dashboard/layout/desktop', request => ({ layout: request.body.layout }));
  first.release({ layout: firstRequest.body.layout });
  await expect.poll(() => api.requests.filter(request => request.method === 'POST' && request.path === '/api/dashboard/layout/desktop').length).toBe(2);
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/dashboard/layout/desktop').at(-1).body.layout.hidden.sort()).toEqual(['statistics', 'storage']);
  api.respond('GET', '/api/dashboard/layout/desktop', { error: '布局服务器暂不可用' }, 503);
  await page.reload();
  await expect(modules(page)).toHaveCount(2);
  await expect(page.getByText('布局读取失败：布局服务器暂不可用', { exact: false })).toBeVisible();
});

test('an unconfirmed layout write displays an error and retries the saved preferences', async ({ page, api }) => {
  api.respond('POST', '/api/dashboard/layout/desktop', { message: 'ok' });
  await page.goto('/dashboard');
  await openSettings(page);
  await page.getByRole('dialog').getByRole('button', { name: '媒体统计', exact: true }).click();
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await expect(page.getByText('布局同步失败：布局保存未确认', { exact: false })).toBeVisible();
  api.respond('POST', '/api/dashboard/layout/desktop', request => ({ layout: request.body.layout }));
  await page.getByRole('button', { name: '重试布局同步', exact: true }).click();
  await expect(page.getByText('布局同步失败：布局保存未确认', { exact: false })).toHaveCount(0);
  await expect(page.locator('[data-dashboard-module="statistics"]')).toHaveCount(0);
});


test('narrow GridStack summary widgets keep statistics and 115 capacities inside their cards', async ({ page, api }) => {
  api.respond('GET', '/api/dashboard/media', dashboard);
  api.respond('GET', '/api/p115/status', space);
  await page.setViewportSize({ width: 1440, height: 1400 });
  await page.goto('/dashboard');
  await expect(page.getByTestId('dashboard-episode_count')).toHaveText('13,781');
  await page.getByRole('button', { name: '调整布局', exact: true }).click();
  const gridBounds = await page.locator('.dashboard-grid').boundingBox();
  for (const id of ['statistics', 'storage']) {
    const widget = page.locator(`[data-dashboard-module="${id}"]`);
    await widget.hover();
    const handle = widget.locator('.ui-resizable-e');
    await expect(handle).toBeVisible();
    const initialWidth = Number(await widget.getAttribute('gs-w'));
    const bounds = await handle.boundingBox();
    await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
    await page.mouse.down();
    await page.mouse.move(bounds.x + bounds.width / 2 - (initialWidth - 2) * gridBounds.width / 12, bounds.y + bounds.height / 2, { steps: 20 });
    await page.mouse.up();
    await expect(widget).toHaveAttribute('gs-w', '2');
  }
  await page.getByRole('button', { name: '完成调整', exact: true }).click();
  const statistics = page.locator('[data-dashboard-module="statistics"] .dashboard-card');
  await expect.poll(async () => statistics.evaluate(card => {
    const rect = card.getBoundingClientRect();
    return [...card.querySelectorAll('.dashboard-stat-item .v-avatar, .dashboard-stat-copy')].every(element => {
      const bounds = element.getBoundingClientRect();
      return bounds.left >= rect.left && bounds.right <= rect.right && bounds.top >= rect.top && bounds.bottom <= rect.bottom;
    });
  })).toBe(true);
  await expect(statistics.locator('.v-avatar')).toHaveCount(3);
  for (const avatar of await statistics.locator('.v-avatar').all()) await expect(avatar).toBeVisible();
  const storage = page.locator('[data-dashboard-module="storage"] .dashboard-card');
  await expect(storage.locator('.storage-icon')).toBeHidden();
  await expect(storage.locator('.storage-number')).toHaveText('1 TB');
  expect(await storage.evaluate(card => {
    const rect = card.getBoundingClientRect();
    return [...card.querySelectorAll('.storage-number, .storage-caption')].every(element => {
      const bounds = element.getBoundingClientRect();
      return bounds.left >= rect.left && bounds.right <= rect.right && element.scrollWidth <= element.clientWidth;
    });
  })).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});
