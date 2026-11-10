import { test, expect } from './fixtures.js';

const library = { id: 'movies', name: '电影', collection_type: 'movies' };
const movies = [
  { Id: '301', Name: '库内甲', Type: 'Movie', ImageTags: { Primary: 'fixture' } },
  { Id: '302', Name: '库内乙', Type: 'Movie', ImageTags: { Primary: 'fixture' } },
];
function libraryResponse(request) {
  const items = movies.filter(item => item.Name.includes(request.query.search || ''));
  return { library, items, total: items.length, start_index: 0, anchor_index: 0 };
}
async function searchInput(page, label, mobile = false) {
  if (mobile) await page.getByRole('button', { name: `打开${label}`, exact: true }).click();
  const input = page.getByRole('searchbox', { name: label, exact: true });
  await expect(input).toBeVisible();
  return input;
}
const searches = api => api.requests.filter(request => request.path === '/api/search_emby_library');
const librarySearches = api => api.requests.filter(request => request.path === '/api/media_center/libraries/movies/items' && request.query.search);

test('desktop topbar searches only the current library, and clearing restores that library', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 800 });
  api.respond('GET', '/api/media_center/libraries/movies/items', libraryResponse);
  await page.goto('/library/movies');
  const input = await searchInput(page, '搜索当前媒体库');
  await expect(page.locator('.media-center-sticky-controls input')).toHaveCount(0);
  await input.fill('库内甲');
  await input.press('Enter');
  await expect(page.locator('.media-card').filter({ hasText: '库内甲' })).toBeVisible();
  await expect(page.locator('.media-card').filter({ hasText: '库内乙' })).toHaveCount(0);
  expect(librarySearches(api).at(-1).query.search).toBe('库内甲');
  expect(searches(api)).toEqual([]);
  await expect(page).toHaveURL(/\/library\/movies$/);
  await page.getByRole('button', { name: '清空搜索', exact: true }).click();
  await expect(input).toHaveValue('');
  await expect(page.locator('.media-card')).toHaveCount(2);
  expect(searches(api)).toEqual([]);
});

test('topbar admission search uses ETK processing history and preserves its failure result', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 800 });
  const records = [
    { id: '8a7a0439-ae54-44e6-8a09-6d8348c81761', item_id: '401', name: '入库甲', item_type: 'Movie', status: 'failed', message: '媒体提取失败', created_at: '2026-11-10T01:00:00Z' },
    { id: '8a7a0439-ae54-44e6-8a09-6d8348c81762', item_id: '402', name: '入库乙', item_type: 'Movie', status: 'success', message: '入库完成', created_at: '2026-11-10T02:00:00Z' },
  ];
  api.respond('GET', '/api/media_organize/history', request => {
    const items = records.filter(item => item.name.includes(request.query.search || ''));
    return { items, total: items.length, page: 1, page_size: 24 };
  });
  await page.goto('/media-organize');
  const input = await searchInput(page, '搜索入库记录');
  await input.fill('入库甲');
  await input.press('Enter');
  await expect(page.getByText('入库甲', { exact: true })).toBeVisible();
  await expect(page.getByText('入库乙', { exact: true })).toHaveCount(0);
  await expect(page.getByText('媒体提取失败', { exact: true })).toBeVisible();
  expect(api.requests.filter(request => request.path === '/api/media_organize/history').at(-1).query.search).toBe('入库甲');
  expect(searches(api)).toEqual([]);
  await expect(page).toHaveURL(/\/media-organize$/);
  await page.getByRole('button', { name: '清空搜索', exact: true }).click();
  await expect(page.getByText('入库乙', { exact: true })).toBeVisible();
});

test('topbar user search filters Emby users locally and does not search media', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 800 });
  await page.goto('/user-management');
  const input = await searchInput(page, '搜索用户');
  await expect(page.locator('.user-card')).toHaveCount(2);
  const reads = api.requests.filter(request => request.path === '/api/portal/users').length;
  await input.fill('media member');
  await input.press('Enter');
  await expect(page.locator('.user-card')).toHaveCount(1);
  await expect(page.locator('.user-card')).toContainText('Media Member');
  expect(api.requests.filter(request => request.path === '/api/portal/users')).toHaveLength(reads);
  expect(searches(api)).toEqual([]);
  await expect(page).toHaveURL(/\/user-management$/);
  await page.getByRole('button', { name: '清空搜索', exact: true }).click();
  await expect(page.locator('.user-card')).toHaveCount(2);
});

test('topbar settings results remain directly clickable and locate the field across categories', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/settings/general');
  const input = await searchInput(page, '搜索设置项');
  await input.fill('TMDB API Key');
  const result = page.getByRole('button', { name: '打开设置：TMDB API Key', exact: true });
  await expect(result).toBeVisible();
  await expect(page.locator('.navbar-search-popover')).toBeHidden();
  await result.click();
  await expect(page.getByRole('tab', { name: '数据源', exact: true })).toHaveAttribute('aria-selected', 'true');
  const target = page.locator('[data-field="tmdb_api_key"]').first();
  await expect(target).toBeVisible();
  await expect(target).toHaveClass(/settings-search-highlight/);
  expect(searches(api)).toEqual([]);
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/config')).toEqual([]);
  await page.getByRole('button', { name: '清空搜索', exact: true }).click();
  await expect(page.locator('.settings-search-results')).toHaveCount(0);
});

test('notification settings share the topbar catalog and open the real Telegram field', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/settings/notifications');
  const input = await searchInput(page, '搜索设置项');
  await input.fill('Bot Token');
  await page.getByRole('button', { name: '打开设置：Bot Token', exact: true }).click();
  await expect(page.getByRole('tab', { name: '通知', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByLabel('Bot Token', { exact: true })).toBeVisible();
  expect(searches(api)).toEqual([]);
  expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
});

for (const [path, mobile] of [['/dashboard', false], ['/edit-media/101', false], ['/dashboard', true]]) {
  test(`${mobile ? 'mobile' : 'desktop'} ${path} searches all media and carries its keyword into the result route`, async ({ page, api }) => {
    await page.setViewportSize({ width: mobile ? 390 : 1440, height: 900 });
    api.respond('GET', '/api/search_emby_library', { items: [{ item_id: '501', item_name: '全库甲', item_type: 'Movie', library_name: '电影' }], total_items: 1 });
    await page.goto(path);
    const input = await searchInput(page, '搜索全部媒体', mobile);
    await input.fill('先清空');
    await page.getByRole('button', { name: '清空搜索', exact: true }).click();
    await expect(input).toHaveValue('');
    expect(searches(api)).toEqual([]);
    await input.fill('全库甲');
    await input.press('Enter');
    await expect(page).toHaveURL(/\/review\?q=/);
    expect(new URL(page.url()).searchParams.get('q')).toBe('全库甲');
    await expect(page.getByText('全库甲', { exact: true }).first()).toBeVisible();
    expect(searches(api).at(-1).query.query).toBe('全库甲');
    if (!mobile) await expect(page.getByRole('searchbox', { name: '搜索全部媒体', exact: true })).toHaveValue('全库甲');
  });
}

test('SPA navigation replaces search scope without reusing the departed page keyword', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  api.respond('GET', '/api/media_center/libraries/movies/items', libraryResponse);
  await page.goto('/library/movies');
  await (await searchInput(page, '搜索当前媒体库')).fill('库内甲');
  const drawer = page.locator('.layout-vertical-nav');
  await drawer.getByText('用户管理', { exact: true }).click();
  const users = await searchInput(page, '搜索用户');
  await expect(users).toHaveValue('');
  await users.fill('Media Member');
  await expect(page.locator('.user-card')).toHaveCount(1);
  await drawer.locator('.app-brand').click();
  await expect(await searchInput(page, '搜索全部媒体')).toHaveValue('');
  expect(searches(api)).toEqual([]);
});

for (const mobile of [false, true]) {
  test(`${mobile ? 'mobile' : 'desktop'} topbar composition and clearing never submit partial words or escape the library scope`, async ({ page, api }) => {
    await page.setViewportSize({ width: mobile ? 390 : 1440, height: 900 });
    api.respond('GET', '/api/media_center/libraries/movies/items', libraryResponse);
    await page.goto('/library/movies');
    const input = await searchInput(page, '搜索当前媒体库', mobile);
    await page.clock.install();
    await input.dispatchEvent('compositionstart');
    await input.fill('kunei');
    await input.press('Enter');
    await page.clock.runFor(500);
    expect(librarySearches(api)).toEqual([]);
    await expect(page).toHaveURL(/\/library\/movies$/);
    await input.evaluate(element => {
      element.value = '库内甲';
      element.dispatchEvent(new CompositionEvent('compositionend', { data: '库内甲', bubbles: true }));
    });
    await input.press('Enter');
    await expect(page.locator('.media-card').filter({ hasText: '库内甲' })).toBeVisible();
    await page.clock.runFor(1200);
    expect(librarySearches(api).map(request => request.query.search)).toEqual(['库内甲']);
    expect(searches(api)).toEqual([]);
    if (mobile) await page.getByRole('button', { name: '打开搜索当前媒体库', exact: true }).click();
    await page.getByRole('button', { name: '清空搜索', exact: true }).click();
    await expect(input).toHaveValue('');
    if (mobile) await page.getByRole('button', { name: '提交搜索', exact: true }).click();
    await expect(page.locator('.media-card')).toHaveCount(2);
    expect(searches(api)).toEqual([]);
  });
}

for (const glassEnabled of [false, true]) {
  test(`${glassEnabled ? 'glass' : 'plain'} topbar and its search text stay fixed and inside the rounded shell while content scrolls`, async ({ page, api }) => {
    await page.setViewportSize({ width: 1440, height: 700 });
    await page.addInitScript(glassEnabled => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({
      theme: glassEnabled ? 'dark' : 'light', glassEnabled, glassQuality: 'css', glassAppearance: 'clear', layout: 'vertical',
    })), glassEnabled);
    api.respond('GET', '/api/media_center/libraries/movies/items', request => {
      const start = Number(request.query.start_index || 0);
      return { library, total: 90, start_index: start, anchor_index: start,
        items: Array.from({ length: 30 }, (_, offset) => ({ Id: String(600 + start + offset), Name: `媒体 ${start + offset}`, Type: 'Movie', ImageTags: { Primary: 'fixture' } })) };
    });
    await page.goto('/library/movies');
    const input = await searchInput(page, '搜索当前媒体库');
    const navbar = page.locator('.layout-navbar');
    const initial = await navbar.boundingBox();
    await page.evaluate(() => window.scrollTo(0, 500));
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThanOrEqual(300);
    await expect(navbar).toBeInViewport();
    await expect(input).toBeInViewport();
    expect(await navbar.evaluate(element => getComputedStyle(element).position)).toBe('fixed');
    expect(Math.abs((await navbar.boundingBox()).y - initial.y)).toBeLessThan(1);
    const geometry = await input.evaluate(element => {
      const box = element.getBoundingClientRect(), parent = element.closest('.navbar-search-desktop').getBoundingClientRect();
      const css = getComputedStyle(element);
      return { top: box.top, bottom: box.bottom, left: box.left, right: box.right,
        parentTop: parent.top, parentBottom: parent.bottom, parentLeft: parent.left, parentRight: parent.right,
        height: box.height, lineHeight: parseFloat(css.lineHeight), fontSize: parseFloat(css.fontSize) };
    });
    expect(geometry.height).toBeGreaterThanOrEqual(geometry.lineHeight);
    expect(geometry.lineHeight).toBeGreaterThanOrEqual(geometry.fontSize * 1.2);
    expect(geometry.top - geometry.parentTop).toBeGreaterThanOrEqual(4);
    expect(geometry.parentBottom - geometry.bottom).toBeGreaterThanOrEqual(4);
    expect(geometry.left - geometry.parentLeft).toBeGreaterThanOrEqual(24);
    expect(geometry.parentRight - geometry.right).toBeGreaterThanOrEqual(24);
  });
}

test('global search dropdown wraps a long keyword and keeps all text inside its rounded card', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 800 });
  await page.goto('/dashboard');
  const input = await searchInput(page, '搜索全部媒体');
  const keyword = '这是用于验证圆角可读性的较长中文搜索关键词'.repeat(8);
  await input.fill(keyword);
  const card = page.locator('.navbar-search-popover');
  const subtitle = card.locator('.v-list-item-subtitle');
  await expect(card).toBeVisible();
  await expect(subtitle).toHaveText(keyword);
  const geometry = await subtitle.evaluate(element => {
    const range = document.createRange(); range.selectNodeContents(element);
    const text = range.getBoundingClientRect(), card = element.closest('.v-card').getBoundingClientRect();
    return { scrollHeight: element.scrollHeight, clientHeight: element.clientHeight,
      textTop: text.top, textBottom: text.bottom, textLeft: text.left, textRight: text.right,
      cardTop: card.top, cardBottom: card.bottom, cardLeft: card.left, cardRight: card.right };
  });
  expect(geometry.scrollHeight).toBeLessThanOrEqual(geometry.clientHeight + 1);
  expect(geometry.textTop - geometry.cardTop).toBeGreaterThanOrEqual(4);
  expect(geometry.cardBottom - geometry.textBottom).toBeGreaterThanOrEqual(4);
  expect(geometry.textLeft - geometry.cardLeft).toBeGreaterThanOrEqual(8);
  expect(geometry.cardRight - geometry.textRight).toBeGreaterThanOrEqual(8);
});
