import { mkdirSync, writeFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';
import { installFrameCaptureGate, pauseCaptureFrames, stepCaptureFrame, resumeCaptureFrames } from './frame-capture.js';
import { navbarInteractionRegion } from './navbar-interaction.js';

test.describe.configure({ timeout: 180_000 });
test.use({ trace: { mode: 'retain-on-failure', screenshots: false, snapshots: false, sources: true } });
const artifactRoot = '/root/emby-toolkit-mp-followup-verification/theme';
mkdirSync(artifactRoot, { recursive: true });
const appearances = ['clear', 'tinted', 'frosted'];
const qualities = ['css', 'balanced', 'high'];
const styles = ['adaptive', 'clear'];
const walls = ['bright', 'dark', 'busy'];
const capturesFrames = options => options.enabled && options.quality !== 'css';

async function waitForRenderer(page, options) {
  if (!options.enabled || options.scheme === 'light' || options.quality === 'css') return;
  await expect.poll(async () => {
    await stepCaptureFrame(page);
    await stepCaptureFrame(page);
    return page.locator('html').getAttribute('data-glass-renderer-state');
  }, { timeout: 180000 }).toMatch(/ready|fallback/);
}

async function settleCaptureFrames(page, options) {
  if (!capturesFrames(options)) return;
  await stepCaptureFrame(page);
  await stepCaptureFrame(page);
  await page.waitForTimeout(250);
  await stepCaptureFrame(page);
}

async function initialize(page, options) {
  const startedAt = Date.now();
  if (capturesFrames(options)) await installFrameCaptureGate(page, { paused: true });
  await page.emulateMedia({ colorScheme: options.scheme === 'light' ? 'light' : 'dark', reducedMotion: options.dynamics === 'off' ? 'reduce' : 'no-preference' });
  await page.addInitScript(options => {
    const canvas = document.createElement('canvas'); canvas.width = canvas.height = 64;
    const context = canvas.getContext('2d');
    context.fillStyle = options.wallpaper === 'dark' ? '#101820' : '#fff'; context.fillRect(0, 0, 64, 64);
    if (options.wallpaper === 'busy') for (let x = 0; x < 64; x += 8) for (let y = 0; y < 64; y += 8) {
      context.fillStyle = (x + y) % 16 ? '#fff' : '#080d14'; context.fillRect(x, y, 8, 8);
    }
    localStorage.setItem('etk-wallpaper-settings', JSON.stringify({ mode: 'custom', customImage: canvas.toDataURL(), rotationInterval: 0 }));
    localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme: options.scheme, glassEnabled: options.enabled,
      glassAppearance: options.appearance, glassQuality: options.quality, glassUIStyle: options.style,
      glassDynamicsMode: options.dynamics, layout: options.layout }));
    const original = HTMLCanvasElement.prototype.getContext;
    window.__glassContexts = [];
    HTMLCanvasElement.prototype.getContext = function (type, ...args) {
      const result = original.call(this, type, ...args);
      if (type === 'webgl2') window.__glassContexts.push({ available: Boolean(result), maxTexture: result?.getParameter(result.MAX_TEXTURE_SIZE) ?? 0 });
      return result;
    };
  }, options);
  await page.setViewportSize({ width: options.representative ? 1281 : options.quality === 'css' ? 1440 : 800,
    height: options.representative ? 640 : options.quality === 'css' ? 1000 : 800 });
  await page.goto('/settings/general', { waitUntil: 'domcontentloaded' });
  await page.bringToFront();
  await expect(page.locator('.setting-card').first()).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-glass-color-scheme', options.scheme === 'auto' ? 'dark' : options.scheme);
  if (options.enabled && options.scheme !== 'light') {
    await expect(page.locator('.wallpaper-image').last()).toBeVisible();
  } else if (options.scheme === 'light') await expect(page.locator('.wallpaper-layer').last()).toBeHidden();
  await waitForRenderer(page, options);
  if (capturesFrames(options) && options.scheme === 'light') await settleCaptureFrames(page, options);
  await expect(page.locator('[data-field="delay_between_items_sec"] input').first()).toHaveAttribute('type', 'number');
  const rendering = await page.evaluate(() => ({ state: document.documentElement.dataset.glassRendererState ?? 'css', contexts: window.__glassContexts,
    dynamics: document.documentElement.dataset.glassDynamicsMode, effectiveDynamics: document.documentElement.dataset.glassDynamicsEffectiveMode,
    capture: window.__etkTestFrameCapture?.status(),
    viewport: { width: innerWidth, height: innerHeight, devicePixelRatio } }));
  rendering.preparationMs = Date.now() - startedAt;
  if (rendering.contexts.some(context => context.available)) expect(rendering.state).toBe('ready');
  if (rendering.state === 'ready') expect(rendering.effectiveDynamics).toBe(options.dynamics);
  if (options.enabled && options.scheme !== 'light') {
    rendering.wallpaper = await page.locator('.wallpaper-layer-active').evaluate(element => ({
      filter: getComputedStyle(element).filter,
      exposureInput: Number.parseFloat(element.style.getPropertyValue('--glass-wallpaper-brightness')),
    }));
    expect(rendering.wallpaper.exposureInput).toBeGreaterThan(0);
    expect(rendering.wallpaper.exposureInput).toBeLessThan(1);
    if (rendering.state !== 'ready') expect(rendering.wallpaper.filter).toMatch(/brightness\(0\.\d+\)/);
  }
  if (options.representative) {
    const shell = page.locator('.etk-default-layout');
    await expect(shell).toHaveAttribute('data-shell-mode', 'desktop');
    if (options.layout === 'horizontal') await expect(shell).toHaveClass(/layout-horizontal-nav-active/);
    else if (options.layout === 'collapsed') await expect(shell).toHaveClass(/layout-vertical-nav-collapsed/);
    else await expect(page.locator('.layout-vertical-nav')).toBeVisible();
  }
  return rendering;
}

function panelProbes(page) {
  const labelSelector = page.viewportSize().width < 960 ? '.app-responsive-input__label' : '.v-field-label--floating';
  const desktop = page.viewportSize().width > 1280;
  return [
    { role: 'panel hint', locator: page.locator('[data-field="media_info_retry_interval"] .text-medium-emphasis').first() },
    { role: 'panel title', locator: page.locator('.setting-card .v-card-title').first() },
    { role: 'card subtitle', locator: page.locator('.setting-card .v-card-subtitle').first() },
    { role: 'input value', locator: page.locator('[data-field="delay_between_items_sec"] input').first() },
    { role: 'input label', locator: page.locator(`[data-field="delay_between_items_sec"] ${labelSelector}`).first() },
    { role: 'settings navigation label', locator: page.getByRole('tab', { name: '存储 & 目录', exact: true }).locator('.v-btn__content') },
    { role: desktop ? 'navbar search shortcut' : 'app navigation label', locator: desktop
      ? page.locator('.navbar-search-desktop .search-shortcut-badge')
      : page.locator('.footer-nav-btn .btn-content > span').first() },
  ];
}

async function recordInteraction(page, options, rendering, id) {
  if (!options.representative) return;
  rendering.framesBefore = await pauseCaptureFrames(page);
  const region = await navbarInteractionRegion(page);
  const { clip, start, end } = region;
  const before = await page.screenshot({ clip, animations: 'disabled' });
  await page.mouse.move(start.x, start.y);
  await page.mouse.down();
  await page.mouse.move(end.x, end.y, { steps: 6 });
  rendering.framesAfter = await stepCaptureFrame(page);
  const after = await page.screenshot({ clip, animations: 'disabled' });
  await page.mouse.up();
  writeFileSync(`${artifactRoot}/${id}-interaction-before.png`, before);
  writeFileSync(`${artifactRoot}/${id}-interaction-after.png`, after);
  rendering.interaction = { requested: options.dynamics, effective: rendering.effectiveDynamics, changed: !before.equals(after), region };
  if (rendering.state === 'ready' && options.dynamics !== 'off') expect(rendering.interaction.changed).toBe(true);
}

async function inspectSurfaces(page, options, rendering, id) {
  const measuredAt = Date.now();
  if (options.representative) {
    for (const probe of panelProbes(page)) await expect(probe.locator, probe.role).toBeInViewport();
  }
  const readings = await measureReadingContrast(page, panelProbes(page));
  await page.getByRole('button', { name: 'UI Test 的用户菜单', exact: true }).click();
  await settleCaptureFrames(page, options);
  const menu = page.locator('.v-menu.v-overlay--active');
  readings.push(...await measureReadingContrast(page, [
    { role: 'menu title', locator: menu.locator('.user-profile-name') },
    { role: 'menu subtitle', locator: menu.locator('.user-profile-role') },
  ]));
  await menu.getByText('自定义导航', { exact: true }).click();
  await settleCaptureFrames(page, options);
  const dialog = page.locator('.v-dialog.v-overlay--active');
  const primaryAction = dialog.getByRole('button', { name: '保存并应用', exact: true });
  const primaryActionText = primaryAction.locator('.v-btn__content');
  await expect(primaryAction, 'dialog primary action button').toBeEnabled();
  await expect(primaryAction, 'dialog primary action button').toBeInViewport({ ratio: 1 });
  await expect(primaryActionText, 'dialog primary action text').toBeInViewport({ ratio: 1 });
  readings.push(...await measureReadingContrast(page, [
    { role: 'dialog title', locator: dialog.locator('.v-card-title').first() },
    { role: 'dialog subtitle', locator: dialog.locator('.v-card-subtitle').first() },
    { role: 'dialog primary action', locator: primaryActionText },
  ]));
  const result = { ...options, rendering, measurementMs: Date.now() - measuredAt, readings };
  writeFileSync(`${artifactRoot}/${id}.json`, JSON.stringify(result, null, 2));
  console.log(JSON.stringify({ id, rendering, readings: readings.map(({ role, contrast, opacity }) => ({ role, contrast: Number(contrast.toFixed(2)), opacity })) }));
  await page.screenshot({ path: `${artifactRoot}/${id}.png`, animations: 'disabled' });
  for (const reading of readings) expect(reading.contrast, `${id}: ${reading.role}`).toBeGreaterThanOrEqual(4.5);
  return result;
}

const cases = [];
for (const scheme of ['light', 'dark']) for (const appearance of appearances) for (const quality of qualities) for (const style of styles) {
  cases.push({ scheme, appearance, quality, style, wallpaper: 'bright', enabled: true, dynamics: 'off', layout: 'vertical' });
}
for (const appearance of appearances) for (const quality of qualities) for (const wallpaper of ['dark', 'busy']) {
  cases.push({ scheme: 'dark', appearance, quality, style: quality === 'high' ? 'clear' : 'adaptive', wallpaper, enabled: true, dynamics: 'off', layout: 'vertical' });
}
for (const scheme of ['light', 'dark']) for (const wallpaper of walls) {
  cases.push({ scheme, appearance: 'clear', quality: 'css', style: 'adaptive', wallpaper, enabled: false, dynamics: 'off', layout: 'vertical' });
}
for (const [index, dynamics] of ['fluid', 'ripple', 'off'].entries()) for (const [layoutIndex, layout] of ['vertical', 'collapsed', 'horizontal'].entries()) {
  cases.push({ scheme: 'dark', appearance: appearances[(index + layoutIndex) % 3], quality: layoutIndex === 1 ? 'high' : 'balanced',
    style: index % 2 ? 'clear' : 'adaptive', wallpaper: 'busy', enabled: true, dynamics, layout, representative: true });
}
for (const [index, options] of cases.entries()) {
  const id = `${index + 1}-${options.scheme}-${options.enabled ? options.appearance : 'plain'}-${options.quality}-${options.style}-${options.wallpaper}-${options.dynamics}-${options.layout}`;
  test(`${id} surfaces stay readable`, async ({ page }) => {
    if (options.representative) test.setTimeout(300_000);
    try {
      const rendering = await initialize(page, options);
      if (options.representative) console.log(JSON.stringify({ id, stage: 'ready', rendering }));
      await recordInteraction(page, options, rendering, id);
      if (options.representative) console.log(JSON.stringify({ id, stage: 'interaction', rendering }));
      await inspectSurfaces(page, options, rendering, id);
    } finally {
      if (capturesFrames(options) && !page.isClosed()) await resumeCaptureFrames(page);
    }
  });
}

for (const quality of qualities) for (const style of styles) {
  const id = `auto-${quality}-${style}`;
  test(`${id} follows live system colors without changing material preferences`, async ({ page }) => {
    const options = { scheme: 'auto', appearance: 'clear', quality, style, wallpaper: 'busy', enabled: true, dynamics: 'off', layout: 'vertical' };
    try {
      const rendering = await initialize(page, options);
      const result = await inspectSurfaces(page, options, rendering, id);
      await page.keyboard.press('Escape');
      await page.emulateMedia({ colorScheme: 'light' });
      await settleCaptureFrames(page, options);
      await expect(page.locator('html')).toHaveAttribute('data-glass-color-scheme', 'light');
      await expect(page.locator('.wallpaper-layer').last()).toBeHidden();
      await expect(page.locator('.setting-card').first()).toBeVisible();
      await expect(page.locator('html')).toHaveAttribute('data-glass-quality', quality);
      await expect(page.locator('html')).toHaveAttribute('data-glass-ui-style', style);
      const lightReadings = await measureReadingContrast(page, panelProbes(page));
      for (const reading of lightReadings) expect(reading.contrast, `${id} system light: ${reading.role}`).toBeGreaterThanOrEqual(4.5);
      await page.emulateMedia({ colorScheme: 'dark' });
      await expect(page.locator('html')).toHaveAttribute('data-glass-color-scheme', 'dark');
      await expect(page.locator('.wallpaper-image').last()).toBeVisible();
      await waitForRenderer(page, options);
      if (rendering.contexts.some(context => context.available)) {
        await expect(page.locator('html')).toHaveAttribute('data-glass-renderer-state', 'ready');
        await expect(page.locator('html')).toHaveAttribute('data-glass-dynamics-effective-mode', options.dynamics);
      }
      const darkReadings = await measureReadingContrast(page, panelProbes(page));
      for (const reading of darkReadings) expect(reading.contrast, `${id} resumed dark: ${reading.role}`).toBeGreaterThanOrEqual(4.5);
      writeFileSync(`${artifactRoot}/${id}.json`, JSON.stringify({ ...result, systemTransitions: { light: lightReadings, dark: darkReadings } }, null, 2));
    } finally {
      if (capturesFrames(options) && !page.isClosed()) await resumeCaptureFrames(page);
    }
  });
}
