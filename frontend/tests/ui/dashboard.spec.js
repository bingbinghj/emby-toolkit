import { test, expect } from './fixtures.js';

const dashboard = {
  statistics: { movie_count: { count: 772 }, tv_count: { count: 208 }, episode_count: { count: 13781 }, music_count: { count: 3 } },
  recent: [{ id: '101', name: '最新入库电影', item_type: 'Movie', production_year: 2026, image_tag: 'poster-tag' }],
  libraries: [{ id: 'movies', name: '我的电影', collection_type: 'movies', image_tag: 'library-composite' }], errors: {},
};
const space = { status: 'success', data: { valid: true, user_info: { rt_space_info: { all_total: { size: 1099511627776 }, all_use: { size: 549755813888 }, all_remain: { size: 549755813888 } } } } };
const modules = page => page.locator('[data-dashboard-module]');

test('dashboard shows actual 115 space, Emby statistics and navigable Emby cover cards', async ({ page, api }) => {
  api.respond('GET', '/api/dashboard/media', { ...dashboard, recent: [...dashboard.recent, { id: 'episode-1', name: '最新剧集', item_type: 'Episode', image_tag: 'episode-screenshot', series_id: 'series-1', series_image_tag: 'series-poster' }] });
  api.respond('GET', '/api/p115/status', space);
  await page.goto('/dashboard');
  await expect(page.getByText('1 TB', { exact: true })).toBeVisible();
  await expect(page.getByText('已使用 50%', { exact: true })).toBeVisible();
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('772');
  await expect(page.getByTestId('dashboard-episode_count')).toHaveText('13,781');
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

test('visibility and order changes persist after reload and can be reset', async ({ page }) => {
  await page.goto('/dashboard');
  await expect(modules(page)).toHaveCount(4);
  await page.getByRole('button', { name: '显示模块', exact: true }).click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByText('仪表盘模块', { exact: true }) });
  await dialog.getByLabel('媒体统计', { exact: true }).uncheck();
  await dialog.getByRole('button', { name: '上移我的媒体库', exact: true }).click();
  await dialog.getByRole('button', { name: '上移我的媒体库', exact: true }).click();
  await dialog.getByRole('button', { name: '完成', exact: true }).click();
  await expect(modules(page)).toHaveCount(3);
  await expect(modules(page).nth(1)).toHaveAttribute('data-dashboard-module', 'libraries');
  await page.reload();
  await expect(modules(page)).toHaveCount(3);
  await expect(modules(page).nth(1)).toHaveAttribute('data-dashboard-module', 'libraries');
  await page.getByRole('button', { name: '显示模块', exact: true }).click();
  await page.getByRole('button', { name: '恢复默认布局', exact: true }).click();
  await page.getByRole('button', { name: '完成', exact: true }).click();
  await expect(modules(page)).toHaveCount(4);
  await expect(modules(page).nth(1)).toHaveAttribute('data-dashboard-module', 'statistics');
});

test('MoviePilot-style refresh failures preserve successful snapshots and current-page 115 space with retry notices', async ({ page, api }) => {
  api.respond('GET', '/api/dashboard/media', dashboard);
  api.respond('GET', '/api/p115/status', space);
  await page.goto('/dashboard');
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('772');
  api.respond('GET', '/api/dashboard/media', { error: 'Emby 暂不可用' }, 503);
  api.respond('GET', '/api/p115/status', { data: { valid: false, msg: '115 凭证失效' } });
  await page.getByRole('button', { name: '刷新', exact: true }).click();
  await expect(page.locator('[data-dashboard-module="recent"]')).toContainText('Emby 暂不可用');
  await expect(page.locator('[data-dashboard-module="recent"]')).toContainText('显示上次成功数据');
  await expect(page.locator('[data-dashboard-module="libraries"]')).toContainText('显示上次成功数据');
  await expect(page.locator('[data-dashboard-module="storage"]')).toContainText('115 凭证失效');
  await expect(page.getByText('1 TB', { exact: true })).toBeVisible();
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('772');
  await expect(page.getByTestId('dashboard-episode_count')).toHaveText('13,781');
  await expect(page.getByRole('link', { name: '编辑最新入库电影', exact: true })).toHaveCount(1);
  await expect(page.getByRole('link', { name: '打开我的电影', exact: true })).toHaveCount(1);
  await expect(page.getByRole('button', { name: '重试最新入库 · Emby', exact: true })).toHaveCount(1);
  await expect(page.getByText('暂无入库媒体', { exact: true })).toHaveCount(0);
  await expect(page.getByText('Emby 暂无媒体库', { exact: true })).toHaveCount(0);
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
  await page.getByRole('button', { name: '刷新', exact: true }).click();
  await expect(page.getByTestId('dashboard-episode_count')).toHaveText('13,781');
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('772');
  await expect(page.locator('[data-dashboard-module="statistics"]')).toContainText('剧集统计读取失败');
  await expect(page.getByRole('link', { name: '编辑最新入库电影', exact: true })).toHaveCount(1);
  await expect(page.getByRole('link', { name: '打开我的电影', exact: true })).toHaveCount(0);
  await expect(page.getByRole('link', { name: '打开已刷新的媒体库', exact: true })).toHaveCount(1);
  const nextRecent = [{ ...dashboard.recent[0], name: '刷新后的入库电影' }];
  api.respond('GET', '/api/dashboard/media', { ...fresh, statistics: { ...dashboard.statistics, movie_count: { count: 773 } }, recent: nextRecent, libraries: null, errors: { libraries: '媒体库读取失败' } });
  await page.getByRole('button', { name: '刷新', exact: true }).click();
  await expect(page.getByRole('link', { name: '打开已刷新的媒体库', exact: true })).toHaveCount(1);
  await expect(page.locator('[data-dashboard-module="libraries"]')).toContainText('媒体库读取失败');
  await expect(page.getByRole('link', { name: '编辑刷新后的入库电影', exact: true })).toHaveCount(1);
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('773');
  api.respond('GET', '/api/dashboard/media', { ...dashboard, statistics: null, libraries: [{ ...dashboard.libraries[0], name: '再次刷新的媒体库' }], errors: { statistics: '媒体统计读取失败' } });
  await page.getByRole('button', { name: '刷新', exact: true }).click();
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
  await expect(page.locator('[data-dashboard-module="storage"]')).toContainText('可用 3 TB / 已用 未知');
  await expect(page.locator('[data-dashboard-module="storage"]')).toContainText('使用比例未知');
});

test('a first read failure shows failure and retry instead of confirmed empty media', async ({ page, api }) => {
  api.respond('GET', '/api/dashboard/media', { error: 'Emby 首次读取失败' }, 503);
  await page.goto('/dashboard');
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('—');
  await expect(page.locator('[data-dashboard-module="recent"]')).toContainText('尚未成功读取，请重试');
  await expect(page.getByText('暂无入库媒体', { exact: true })).toHaveCount(0);
  api.respond('GET', '/api/dashboard/media', dashboard);
  await page.getByRole('button', { name: '重试最新入库 · Emby', exact: true }).click();
  await expect(page.getByRole('link', { name: '编辑最新入库电影', exact: true })).toHaveCount(1);
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('772');
  await expect(page.locator('[data-dashboard-module="recent"] .v-alert')).toHaveCount(0);
});

test('confirmed empty snapshots survive a failed reload and expire after 24 hours', async ({ page, api }) => {
  api.respond('GET', '/api/dashboard/media', { ...dashboard, recent: [], libraries: [] });
  await page.goto('/dashboard');
  await expect(page.getByText('暂无入库媒体', { exact: true })).toBeVisible();
  await expect(page.getByText('Emby 暂无媒体库', { exact: true })).toHaveCount(1);
  api.respond('GET', '/api/dashboard/media', { error: '刷新失败' }, 503);
  await page.reload();
  await expect(page.locator('[data-dashboard-module="recent"]')).toContainText('显示上次成功数据');
  await expect(page.getByText('暂无入库媒体', { exact: true })).toHaveCount(1);
  await expect(page.getByText('Emby 暂无媒体库', { exact: true })).toHaveCount(1);
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
  await expect(page.getByText('暂无入库媒体', { exact: true })).toHaveCount(0);
  await expect(page.getByText('Emby 暂无媒体库', { exact: true })).toHaveCount(0);
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
  await page.getByRole('link', { name: '搜索媒体', exact: true }).click();
  await expect(page).toHaveURL(/\/review$/);
  const received = page.waitForResponse('**/api/dashboard/media');
  pending.release(dashboard);
  await received;
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(resolve)));
  expect(await page.evaluate(() => Object.keys(localStorage).filter(key => key.startsWith('ETK_DASHBOARD_SNAPSHOT_V1:')))).toEqual([]);
});

test('mobile dashboard remains within the viewport and order controls are usable', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/dashboard');
  await expect(modules(page)).toHaveCount(4);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.getByRole('button', { name: '显示模块', exact: true }).click();
  await page.getByRole('button', { name: '上移媒体统计', exact: true }).click();
  await page.getByRole('button', { name: '完成', exact: true }).click();
  await expect(modules(page).first()).toHaveAttribute('data-dashboard-module', 'statistics');
});

test('my media uses a horizontally scrollable row of Emby covers at desktop and mobile widths', async ({ page, api }) => {
  api.respond('GET', '/api/dashboard/media', { ...dashboard, libraries: Array.from({ length: 11 }, (_, index) => ({ id: `library-${index}`, name: `Emby 媒体库 ${index + 1}`, collection_type: 'movies', image_tag: `composite-${index}` })) });
  for (const [width, expectedCardWidth, expectedGap] of [[1920, 245, 21], [1440, 201.6, 21], [390, 154, 11]]) {
    await page.setViewportSize({ width, height: 1000 });
    await page.goto('/dashboard');
    const row = page.getByRole('region', { name: '我的媒体库列表', exact: true });
    await expect(row.getByRole('link')).toHaveCount(11);
    const metrics = await row.evaluate(element => {
      const cards = [...element.querySelectorAll('.dashboard-library-card')];
      const art = cards[0].querySelector('.v-responsive').getBoundingClientRect();
      const title = getComputedStyle(cards[0].querySelector('.dashboard-library-title'));
      return { scrollWidth: element.scrollWidth, clientWidth: element.clientWidth, cardWidth: cards[0].getBoundingClientRect().width, ratio: art.width / art.height, gap: parseFloat(getComputedStyle(element).gap), topAligned: cards.every(card => Math.abs(card.getBoundingClientRect().top - cards[0].getBoundingClientRect().top) < 1), weight: title.fontWeight };
    });
    expect(metrics.cardWidth).toBeCloseTo(expectedCardWidth, 0);
    expect(metrics.gap).toBe(expectedGap);
    expect(metrics.ratio).toBeCloseTo(16 / 9, 1);
    expect(metrics.topAligned).toBe(true);
    expect(metrics.weight).toBe('400');
    expect(metrics.scrollWidth).toBeGreaterThan(metrics.clientWidth);
    await row.evaluate(element => { element.scrollLeft = 300; });
    await expect.poll(() => row.evaluate(element => element.scrollLeft)).toBeGreaterThan(0);
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
  const source = await library.boundingBox();
  const target = await storage.boundingBox();
  await page.mouse.move(source.x + source.width / 2, source.y + source.height / 2);
  await page.mouse.down();
  await page.mouse.move(source.x + source.width / 2, source.y - 12, { steps: 3 });
  await page.mouse.move(target.x + 20, target.y + 20, { steps: 20 });
  await page.mouse.up();
  await expect.poll(async () => (await modules(page).evaluateAll(items => items.map(item => item.dataset.dashboardModule))).indexOf('libraries')).toBeLessThan(3);
  await expect.poll(() => page.evaluate(() => JSON.parse(localStorage.getItem('etk-dashboard-layout-v1')).order.indexOf('libraries'))).toBeLessThan(3);
  await page.getByRole('button', { name: '完成调整', exact: true }).click();
  await expect(page.locator('.dashboard-drag-handle')).toHaveCount(0);
});
