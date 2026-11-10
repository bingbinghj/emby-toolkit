import { test, expect } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';

test.describe.configure({ timeout: 180_000 });
test.use({ trace: { mode: 'retain-on-failure', screenshots: false, snapshots: true, sources: true } });

async function readingContrast(page, locator, name) {
  const [reading] = await measureReadingContrast(page, [{ role: name, locator }]);
  console.log(JSON.stringify(reading));
  return reading.contrast;
}

for (const [appearance, quality] of [['clear', 'css'], ['tinted', 'css'], ['frosted', 'css'], ['clear', 'balanced']]) {
  test(`dark ${appearance}/${quality} wallpaper keeps panel, input, navbar and menu text readable`, async ({ page }) => {
    await page.addInitScript(({ appearance, quality }) => {
      const canvas = document.createElement('canvas');
      canvas.width = canvas.height = 32;
      const context = canvas.getContext('2d');
      context.fillStyle = '#fff'; context.fillRect(0, 0, 32, 32);
      localStorage.setItem('etk-wallpaper-settings', JSON.stringify({ mode: 'custom', customImage: canvas.toDataURL(), rotationInterval: 0 }));
      localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme: 'dark', glassEnabled: true, glassAppearance: appearance, glassQuality: quality }));
    }, { appearance, quality });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto('/settings/general', { waitUntil: 'domcontentloaded' });
    await page.bringToFront();
    await expect(page.locator('.wallpaper-image').last()).toBeVisible();
    if (quality === 'css') await expect.poll(() => page.locator('.wallpaper-layer-active').evaluate(element => getComputedStyle(element).filter)).toMatch(/brightness\(0\.\d+\)/);
    else await expect.poll(() => page.locator('html').getAttribute('data-glass-renderer-state'), { timeout: 60000 }).toMatch(/ready|fallback/);
    const hint = page.locator('[data-field="media_info_retry_interval"] .text-medium-emphasis').first();
    const input = page.locator('[data-field="delay_between_items_sec"] input').first();
    const search = page.locator('.navbar-search').first();
    expect(await readingContrast(page, hint, `${appearance}/${quality} panel hint`)).toBeGreaterThanOrEqual(4.5);
    expect(await readingContrast(page, input, `${appearance}/${quality} input`)).toBeGreaterThanOrEqual(4.5);
    expect(await readingContrast(page, page.locator('[data-field="delay_between_items_sec"] .v-field-label--floating').first(), `${appearance}/${quality} field label`)).toBeGreaterThanOrEqual(4.5);
    expect(await readingContrast(page, search, `${appearance}/${quality} navbar`)).toBeGreaterThanOrEqual(4.5);
    await page.getByRole('button', { name: 'UI Test 的用户菜单', exact: true }).click();
    const menu = page.locator('.v-menu.v-overlay--active .v-list-item-title').first();
    expect(await readingContrast(page, menu, `${appearance}/${quality} menu`)).toBeGreaterThanOrEqual(4.5);
    expect(await readingContrast(page, page.locator('.v-menu.v-overlay--active .v-list-item-subtitle').first(), `${appearance}/${quality} menu subtitle`)).toBeGreaterThanOrEqual(4.5);
    await page.screenshot({ path: `/root/emby-toolkit-mp-migration-verification/theme-reading-${appearance}-${quality}.png` });
  });
}

test('light theme leaves the wallpaper disabled and keeps the MP light palette', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme: 'light', glassEnabled: true })));
  await page.goto('/settings/general');
  await expect(page.locator('.setting-card').first()).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-glass-color-scheme', 'light');
  await expect(page.locator('.wallpaper-layer').last()).toBeHidden();
  const values = await page.locator('.v-application').evaluate(element => {
    const css = getComputedStyle(element);
    return { foreground: css.getPropertyValue('--v-theme-on-surface').replace(/\s/g, ''), surface: css.getPropertyValue('--v-theme-surface').replace(/\s/g, '') };
  });
  expect(values.foreground).toBe('0,0,0');
  expect(values.surface).toBe('255,255,255');
});
