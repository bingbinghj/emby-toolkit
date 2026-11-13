import { mkdirSync, writeFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';
import { installFrameCaptureGate, stepCaptureFrame, resumeCaptureFrames } from './frame-capture.js';

test.describe.configure({ timeout: 180_000 });
test.use({ trace: { mode: 'retain-on-failure', screenshots: false, snapshots: false, sources: true } });
const artifactRoot = '/root/emby-toolkit-nov11-verification/theme';
mkdirSync(artifactRoot, { recursive: true });

for (const [appearance, quality] of [['clear', 'css'], ['tinted', 'css'], ['frosted', 'css'], ['clear', 'balanced']]) {
  test(`dark ${appearance}/${quality} wallpaper keeps panel, input, navbar and menu text readable`, async ({ page }) => {
    if (quality !== 'css') {
      test.setTimeout(300_000);
      await installFrameCaptureGate(page, { paused: true });
    }
    await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'no-preference' });
    await page.addInitScript(({ appearance, quality }) => {
      const canvas = document.createElement('canvas');
      canvas.width = canvas.height = 32;
      const context = canvas.getContext('2d');
      context.fillStyle = '#fff'; context.fillRect(0, 0, 32, 32);
      localStorage.setItem('etk-wallpaper-settings', JSON.stringify({ mode: 'custom', customImage: canvas.toDataURL(), rotationInterval: 0 }));
      localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme: 'dark', glassEnabled: true, glassAppearance: appearance, glassQuality: quality, glassDynamicsMode: 'ripple' }));
      const getContext = HTMLCanvasElement.prototype.getContext;
      window.__readingGlassContexts = [];
      HTMLCanvasElement.prototype.getContext = function (type, ...args) {
        const result = getContext.call(this, type, ...args);
        if (type === 'webgl2') window.__readingGlassContexts.push(Boolean(result));
        return result;
      };
    }, { appearance, quality });
    await page.setViewportSize(quality === 'css' ? { width: 1440, height: 1000 } : { width: 1281, height: 640 });
    try {
      await page.goto('/settings/general', { waitUntil: 'domcontentloaded' });
      await page.bringToFront();
      await expect(page.locator('.wallpaper-image').last()).toBeVisible();
      if (quality === 'css') await expect.poll(() => page.locator('.wallpaper-layer-active').evaluate(element => getComputedStyle(element).filter)).toMatch(/brightness\(0\.\d+\)/);
      else await expect.poll(async () => {
        await stepCaptureFrame(page);
        await stepCaptureFrame(page);
        return (await page.locator('html').getAttribute('data-glass-renderer-state')) ?? '';
      }, { timeout: 180000 }).toMatch(/ready|fallback/);
      const rendering = await page.evaluate(() => ({ state: document.documentElement.dataset.glassRendererState ?? 'css',
        contexts: window.__readingGlassContexts, dynamics: document.documentElement.dataset.glassDynamicsMode,
        effectiveDynamics: document.documentElement.dataset.glassDynamicsEffectiveMode,
        viewport: { width: innerWidth, height: innerHeight, devicePixelRatio }, capture: window.__etkTestFrameCapture?.status() }));
      if (rendering.contexts.some(Boolean)) expect(rendering.state).toBe('ready');
      if (rendering.state === 'ready') {
        expect(rendering.dynamics).toBe('ripple');
        expect(rendering.effectiveDynamics).toBe('ripple');
      }
      const input = page.locator('[data-field="delay_between_items_sec"] input').first();
      await expect(input).toHaveAttribute('type', 'number');
      const readings = await measureReadingContrast(page, [
        { role: 'panel hint', locator: page.locator('[data-field="media_info_retry_interval"] .settings-hint .text-medium-emphasis').first() },
        { role: 'input value', locator: input },
        { role: 'input label', locator: page.locator('[data-field="delay_between_items_sec"] .v-field-label--floating').first() },
        { role: 'navbar search shortcut', locator: page.locator('.navbar-search-desktop .search-shortcut-badge') },
      ]);
      await page.getByRole('button', { name: 'UI Test 的用户菜单', exact: true }).click();
      if (quality !== 'css') {
        await stepCaptureFrame(page);
        await stepCaptureFrame(page);
        await page.waitForTimeout(250);
        await stepCaptureFrame(page);
      }
      const menu = page.locator('.v-menu.v-overlay--active');
      readings.push(...await measureReadingContrast(page, [
        { role: 'menu title', locator: menu.locator('.user-profile-name') },
        { role: 'menu subtitle', locator: menu.locator('.user-profile-role') },
      ]));
      const id = `legacy-${appearance}-${quality}`;
      writeFileSync(`${artifactRoot}/${id}.json`, JSON.stringify({ appearance, quality, rendering, readings }, null, 2));
      await page.screenshot({ path: `${artifactRoot}/${id}.png`, animations: 'disabled' });
      for (const reading of readings) expect(reading.contrast, `${id}: ${reading.role}`).toBeGreaterThanOrEqual(4.5);
      console.log(JSON.stringify({ id, rendering, readings }));
    } finally {
      if (quality !== 'css' && !page.isClosed()) await resumeCaptureFrames(page);
    }
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
