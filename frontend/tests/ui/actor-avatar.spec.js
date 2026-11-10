import { test, expect, mediaDetails } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';

const evidence = '/root/emby-toolkit-mp-followup-verification/actor-avatar';
const portrait = '<svg xmlns="http://www.w3.org/2000/svg" width="120" height="180"><rect width="120" height="60" fill="#9c3b35"/><rect y="60" width="120" height="60" fill="#2e7967"/><rect y="120" width="120" height="60" fill="#385f9d"/></svg>';

async function loadActors(page, api, mode, width) {
  await page.setViewportSize({ width, height: 900 });
  await page.addInitScript(({ mode }) => {
    localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme: mode === 'light' ? 'light' : 'dark', glassEnabled: ['clear', 'frosted'].includes(mode), glassQuality: 'css', glassAppearance: mode === 'frosted' ? 'frosted' : 'clear' }));
    localStorage.setItem('etk-wallpaper-settings', JSON.stringify({ mode: 'custom', customImage: 'data:image/svg+xml;base64,' + btoa('<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><rect width="32" height="32" fill="white"/></svg>'), rotationInterval: 0 }));
  }, { mode });
  api.respond('GET', '/api/media_for_editing/101', { ...mediaDetails, current_emby_cast: mediaDetails.current_emby_cast.map(actor => ({ ...actor, imageUrl: 'https://example.invalid/portrait.svg' })) });
  await page.route('**/api/image_proxy?*', route => route.fulfill({ contentType: 'image/svg+xml', body: portrait }));
  await page.goto('/edit-media/101', { waitUntil: 'domcontentloaded' });
  await expect(page.locator('.actor-edit-card')).toHaveCount(2);
  const avatar = page.locator('.actor-avatar-image').first();
  await avatar.scrollIntoViewIfNeeded();
  await expect.poll(() => avatar.locator('img.v-img__img').evaluate(image => image.complete && image.naturalWidth === 120 && image.naturalHeight === 180)).toBe(true);
  return avatar;
}

async function metrics(avatar) {
  return avatar.evaluate(element => {
    const box = element.getBoundingClientRect(), image = element.querySelector('img.v-img__img');
    const content = element.closest('.actor-card-content').getBoundingClientRect();
    const card = element.closest('.actor-edit-card').getBoundingClientRect();
    const css = getComputedStyle(element), fit = getComputedStyle(image).objectFit;
    const imageScale = Math.max(box.width / image.naturalWidth, box.height / image.naturalHeight);
    return {
      width: box.width, height: box.height, flexGrow: css.flexGrow, objectFit: fit,
      naturalWidth: image.naturalWidth, naturalHeight: image.naturalHeight,
      visibleFraction: box.width * box.height / (image.naturalWidth * image.naturalHeight * imageScale * imageScale),
      withinContent: box.left >= content.left - .5 && box.right <= content.right + .5 && box.top >= content.top - .5 && box.bottom <= content.bottom + .5,
      withinCard: box.left >= card.left && box.right <= card.right && box.top >= card.top && box.bottom <= card.bottom,
      unobscured: [.2, .5, .8].every(position => element.contains(document.elementFromPoint(box.left + box.width / 2, box.top + box.height * position))),
    };
  });
}

test('the old growing VImg reproduces the half-portrait crop while the fixed avatar prevents it', async ({ page, api }) => {
  const avatar = await loadActors(page, api, 'plain', 1440);
  await avatar.evaluate(element => { element.style.flex = '1 0 auto'; });
  const before = await metrics(avatar);
  expect(before.width).toBeGreaterThan(72 * 1.5);
  expect(before.visibleFraction).toBeLessThan(.6);
  await page.screenshot({ path: `${evidence}/flex-reproduction-before.png` });
  await avatar.evaluate(element => { element.style.removeProperty('flex'); });
  const after = await metrics(avatar);
  expect(after.width).toBeCloseTo(72, 1);
  expect(after.height).toBeCloseTo(96, 1);
  expect(after.visibleFraction).toBeGreaterThanOrEqual(.85);
  await page.screenshot({ path: `${evidence}/flex-reproduction-fixed.png` });
  console.log(JSON.stringify({ avatarFlexReproduction: { before, after } }));
});

for (const mode of ['light', 'plain', 'clear', 'frosted']) {
  for (const width of [390, 1440]) {
    test(`${mode} ${width}px actor portraits load completely and stay inside their cards`, async ({ page, api }) => {
      const avatar = await loadActors(page, api, mode, width);
      const layout = await metrics(avatar);
      expect(layout.width).toBeCloseTo(72, 1); expect(layout.height).toBeCloseTo(96, 1);
      expect(layout.flexGrow).toBe('0'); expect(layout.objectFit).toBe('cover');
      expect(layout.naturalWidth).toBe(120); expect(layout.naturalHeight).toBe(180);
      expect(layout.visibleFraction).toBeGreaterThanOrEqual(.85);
      expect(layout.withinContent).toBe(true); expect(layout.withinCard).toBe(true); expect(layout.unobscured).toBe(true);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      const card = page.locator('.actor-edit-card').first();
      const contrast = await measureReadingContrast(page, [
        { role: 'actor name', locator: card.locator('.actor-name-title') },
        { role: 'actor input', locator: card.getByLabel('演员', { exact: true }) },
      ]);
      for (const reading of contrast) expect(reading.contrast).toBeGreaterThanOrEqual(4.5);
      await page.screenshot({ path: `${evidence}/${mode}-${width}.png` });
      console.log(JSON.stringify({ actorPortrait: { mode, width, layout, contrast } }));
      expect(api.requests.some(request => request.method !== 'GET')).toBe(false);
    });
  }
}
