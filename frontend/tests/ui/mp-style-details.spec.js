import { mkdirSync, writeFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';

const artifactRoot = '/root/emby-toolkit-mp-followup-verification/style';
mkdirSync(artifactRoot, { recursive: true });
const themes = [
  { name: 'plain-light', theme: 'light', glassEnabled: false },
  { name: 'plain-dark', theme: 'dark', glassEnabled: false },
  { name: 'glass-light', theme: 'light', glassEnabled: true },
  { name: 'glass-dark', theme: 'dark', glassEnabled: true },
];
const title = '媒体悬浮详情长名称与字号验证'.repeat(8);
const overview = '长简介应按三行截断，文字留在海报内并维持可读对比度。'.repeat(12);

async function prepare(page, theme, width = 1440) {
  await page.setViewportSize({ width, height: 1000 });
  await page.emulateMedia({ colorScheme: theme.theme, reducedMotion: 'reduce' });
  await page.addInitScript(settings => {
    localStorage.setItem('etk-wallpaper-settings', JSON.stringify({ mode: 'none' }));
    localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ ...settings,
      glassQuality: 'css', glassAppearance: settings.glassAppearance || 'clear', glassUIStyle: settings.glassUIStyle || 'adaptive', radius: 'extra', layout: 'vertical' }));
  }, theme);
}

async function readable(page, id, probes) {
  for (const probe of probes) await expect(probe.locator, probe.role).toBeInViewport({ ratio: 1 });
  const readings = await measureReadingContrast(page, probes);
  writeFileSync(`${artifactRoot}/${id}.json`, JSON.stringify(readings, null, 2));
  await page.screenshot({ path: `${artifactRoot}/${id}.png`, animations: 'disabled' });
  for (const reading of readings) expect(reading.contrast, `${id}: ${reading.role}`).toBeGreaterThanOrEqual(4.5);
}

async function insideArtwork(artwork, children) {
  const bounds = await children.evaluateAll((elements, artworkSelector) => elements.map(element => {
    const art = element.closest(artworkSelector).getBoundingClientRect();
    const box = element.getBoundingClientRect();
    return { top: box.top - art.top, bottom: art.bottom - box.bottom,
      left: box.left - art.left, right: art.right - box.right };
  }), artwork);
  for (const box of bounds) for (const distance of Object.values(box)) expect(distance).toBeGreaterThanOrEqual(-1);
}

for (const width of [390, 1440]) for (const theme of themes) for (const color of ['#ffffff', '#080d14']) {
  const id = `${width}-${theme.name}-${color === '#ffffff' ? 'white' : 'dark'}`;
  test(`${id} poster details and badges retain MP typography, readable text and clipping`, async ({ page, api }) => {
    await prepare(page, theme, width);
    await page.route('**/image_proxy/**', route => route.fulfill({ contentType: 'image/svg+xml',
      body: `<svg xmlns="http://www.w3.org/2000/svg" width="120" height="180"><rect width="120" height="180" fill="${color}"/></svg>` }));
    api.respond('GET', '/api/media_center/libraries/movies/items', {
      library: { id: 'movies', name: '电影', collection_type: 'movies' }, total: 1, start_index: 0, anchor_index: 0,
      items: [{ Id: '101', Name: title, Type: 'Movie', ProductionYear: 2026, CommunityRating: 8.2,
        Overview: overview, ImageTags: { Primary: 'fixture' } }],
    });
    api.respond('GET', '/api/dashboard/media', {
      statistics: { movie_count: { count: 1 }, tv_count: { count: 1 }, episode_count: { count: 0 }, music_count: { count: 0 } },
      recent: ['Movie', 'Series'].map((item_type, index) => ({ id: String(101 + index), name: title,
        item_type, production_year: 2026, image_tag: 'fixture' })),
      libraries: [{ id: 'movies', name: '电影', collection_type: 'movies', item_count: 1, image_tag: 'fixture' }], errors: {},
    });
    await page.goto('/library/movies');
    const card = page.locator('[data-media-index="0"] .media-card');
    await expect.poll(() => card.locator('img.v-img__img').evaluate(image => image.complete && image.naturalWidth > 0)).toBe(true);
    await page.locator('[data-media-index="0"]').hover();
    const detail = card.locator('.media-card-detail');
    await expect(detail).toBeVisible();
    await expect(detail.locator('.media-detail-title')).toHaveCSS('font-size', '18px');
    await expect(detail.locator('.media-detail-title')).toHaveCSS('font-weight', '700');
    await expect(detail.locator('.media-detail-title')).toHaveCSS('color', 'rgb(255, 255, 255)');
    await expect(detail.locator('.media-detail-title')).toHaveCSS('-webkit-line-clamp', '2');
    await expect(detail.locator('.media-detail-year')).toHaveCSS('font-size', '14px');
    await expect(detail.locator('.media-detail-year')).toHaveCSS('font-weight', '600');
    await expect(detail.locator('.media-overview')).toHaveCSS('font-size', '14px');
    await expect(detail.locator('.media-overview')).toHaveCSS('-webkit-line-clamp', '3');
    for (const selector of ['.media-detail-title', '.media-overview']) {
      const lines = await detail.locator(selector).evaluate(element => {
        const height = element.getBoundingClientRect().height, lineHeight = parseFloat(getComputedStyle(element).lineHeight);
        return { height, completeLinesHeight: Math.round(height / lineHeight) * lineHeight };
      });
      expect(lines.height, `${selector} must clip at a complete line`).toBeCloseTo(lines.completeLinesHeight, 0);
    }
    await insideArtwork('.media-poster-wrap', detail.locator(':scope > *'));
    await insideArtwork('.media-poster-wrap', card.locator('.media-rating'));
    await readable(page, `${id}-library`, [
      { role: 'hover year', locator: detail.locator('.media-detail-year') },
      { role: 'hover title', locator: detail.locator('.media-detail-title') },
      { role: 'hover overview', locator: detail.locator('.media-overview') },
      { role: 'Emby rating', locator: card.locator('.media-rating .v-chip__content') },
    ]);
    await page.goto('/dashboard');
    await expect(page.locator('.dashboard-media-card')).toHaveCount(2);
    for (const index of [0, 1]) {
      const poster = page.locator('.dashboard-media-card').nth(index);
      await expect.poll(() => poster.locator('img.v-img__img').evaluate(image => image.complete && image.naturalWidth > 0)).toBe(true);
      await poster.hover();
      await expect(poster.locator('.dashboard-poster-overlay')).toHaveCSS('opacity', '1');
      await insideArtwork('.dashboard-media-card', poster.locator('.dashboard-poster-title, .dashboard-poster-subtitle, .dashboard-media-type'));
      await readable(page, `${id}-dashboard-${index}`, [
        { role: 'dashboard hover title', locator: poster.locator('.dashboard-poster-title') },
        { role: 'dashboard hover year', locator: poster.locator('.dashboard-poster-subtitle') },
        { role: 'dashboard type', locator: poster.locator('.dashboard-media-type .v-chip__content') },
      ]);
    }
    const caption = page.locator('.dashboard-library-title').first();
    await expect(caption).toHaveCSS('font-size', '12px');
    await expect(caption).toHaveCSS('font-weight', '700');
    await caption.scrollIntoViewIfNeeded();
    await readable(page, `${id}-dashboard-caption`, [{ role: 'Emby library caption', locator: caption }]);
  });
}

for (const theme of themes) test(`${theme.name} confirmation text and settings hints retain MP sizing and contrast`, async ({ page, api }) => {
  await prepare(page, theme);
  await page.goto('/settings/general');
  const hint = page.locator('[data-field="media_info_retry_interval"] .settings-hint span');
  await expect(hint).toHaveCSS('font-size', '12px');
  await page.goto('/settings/notifications');
  await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
  const confirm = page.getByRole('button', { name: '确定', exact: true });
  await expect(confirm.locator('.v-btn__content')).toHaveCSS('font-size', '14px');
  await expect(confirm.locator('.v-btn__content')).toHaveCSS('font-family', /Inter.*Noto Sans SC/);
  await readable(page, `${theme.name}-notification-confirm`, [{ role: 'channel confirmation', locator: confirm.locator('.v-btn__content') }]);
  await page.getByRole('button', { name: '关闭通知渠道', exact: true }).click();
  api.respond('GET', '/api/media_organize/history', { items: [{ id: '00000000-0000-4000-8000-000000000101',
    item_id: '101', name: '确认按钮文字对比验证', item_type: 'Movie', status: 'success' }], total: 1 });
  await page.goto('/media-organize');
  await page.getByTestId('organize-items').locator('tbody tr').first().getByRole('checkbox').check();
  await page.getByTestId('organize-selected').click();
  const organize = page.getByTestId('organize-confirm');
  await expect(organize.locator('.v-btn__content')).toHaveCSS('font-size', '14px');
  await expect(organize.locator('.v-btn__content')).toHaveCSS('font-family', /Inter.*Noto Sans SC/);
  await readable(page, `${theme.name}-organize-confirm`, [{ role: 'organize confirmation', locator: organize.locator('.v-btn__content') }]);
  expect(api.requests.filter(request => request.method === 'POST' && request.path !== '/api/config/rename_preview' && !request.path.startsWith('/api/dashboard/layout/'))).toEqual([]);
});

for (const appearance of ['clear', 'tinted', 'frosted']) for (const style of ['adaptive', 'clear']) {
  test(`dark ${appearance}/${style} filled primary action remains readable`, async ({ page, api }) => {
    await prepare(page, { name: `${appearance}-${style}`, theme: 'dark', glassEnabled: true, glassAppearance: appearance, glassUIStyle: style });
    await page.goto('/settings/notifications');
    await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
    const confirm = page.getByRole('button', { name: '确定', exact: true });
    await expect(confirm).toHaveClass(/v-btn--variant-flat/);
    await readable(page, `dark-${appearance}-${style}-primary-action`, [{ role: 'primary confirmation', locator: confirm.locator('.v-btn__content') }]);
    expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
  });
}

test('page heading follows MP 640px typography breakpoint', async ({ page }) => {
  await prepare(page, themes[0], 600);
  await page.goto('/library/movies');
  const heading = page.locator('.mp-page-title__heading');
  for (const [width, size, lineHeight] of [[600, '24px', '28px'], [639, '24px', '28px'], [640, '30px', '36px']]) {
    await page.setViewportSize({ width, height: 1000 });
    await expect(heading).toHaveCSS('font-size', size);
    await expect(heading).toHaveCSS('line-height', lineHeight);
  }
});
