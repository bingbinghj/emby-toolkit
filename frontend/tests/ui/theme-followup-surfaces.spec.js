import { mkdirSync, writeFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';
import { installFrameCaptureGate, stepCaptureFrame, resumeCaptureFrames } from './frame-capture.js';
import { navbarInteractionRegion } from './navbar-interaction.js';

test.describe.configure({ timeout: 180_000 });
test.use({ trace: { mode: 'retain-on-failure', screenshots: false, snapshots: false, sources: true } });
const artifactRoot = '/root/emby-toolkit-nov11-verification/theme';
mkdirSync(artifactRoot, { recursive: true });
const liveBody = '用户同步失败，请检查服务器连接。';
const historyBody = '用户权限同步失败，请检查 Emby 连接。';
const cases = ['clear', 'tinted', 'frosted'].flatMap(appearance => ['adaptive', 'clear'].map(style => ({
  scheme: 'dark', enabled: true, appearance, style, quality: 'css', dynamics: 'off',
})));
cases.push(
  ...['light', 'dark'].map(scheme => ({ scheme, enabled: false, appearance: 'clear', style: 'adaptive', quality: 'css', dynamics: 'off' })),
  { scheme: 'auto', enabled: true, appearance: 'clear', style: 'adaptive', quality: 'css', dynamics: 'off' },
  { scheme: 'dark', enabled: true, appearance: 'tinted', style: 'adaptive', quality: 'balanced', dynamics: 'fluid' },
  { scheme: 'dark', enabled: true, appearance: 'frosted', style: 'clear', quality: 'high', dynamics: 'ripple' },
);

async function prepare(page, options) {
  if (options.quality !== 'css') await installFrameCaptureGate(page, { paused: true });
  await page.setViewportSize(options.quality === 'css' ? { width: 1440, height: 1000 } : { width: 1281, height: 800 });
  await page.emulateMedia({ colorScheme: options.scheme === 'light' ? 'light' : 'dark', reducedMotion: options.dynamics === 'off' ? 'reduce' : 'no-preference' });
  await page.addInitScript(options => {
    const canvas = document.createElement('canvas'); canvas.width = canvas.height = 64;
    const context = canvas.getContext('2d');
    for (let x = 0; x < 64; x += 8) for (let y = 0; y < 64; y += 8) {
      context.fillStyle = (x + y) % 16 ? '#fff' : '#080d14'; context.fillRect(x, y, 8, 8);
    }
    localStorage.setItem('etk-wallpaper-settings', JSON.stringify({ mode: 'custom', customImage: canvas.toDataURL(), rotationInterval: 0 }));
    localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme: options.scheme, glassEnabled: options.enabled,
      glassAppearance: options.appearance, glassQuality: options.quality, glassUIStyle: options.style,
      glassDynamicsMode: options.dynamics, layout: 'vertical' }));
    const getContext = HTMLCanvasElement.prototype.getContext;
    window.__followupGlassContexts = [];
    HTMLCanvasElement.prototype.getContext = function (type, ...args) {
      const result = getContext.call(this, type, ...args);
      if (type === 'webgl2') window.__followupGlassContexts.push(Boolean(result));
      return result;
    };
  }, options);
}

async function settle(page, options) {
  if (options.quality !== 'css') {
    await stepCaptureFrame(page);
    await stepCaptureFrame(page);
  }
  await page.waitForTimeout(250);
}

async function openPage(page, options, path, scheme) {
  await page.goto(path, { waitUntil: 'domcontentloaded' });
  await page.bringToFront();
  await expect(page.locator('html')).toHaveAttribute('data-glass-color-scheme', scheme);
  await expect(page.locator('html')).toHaveAttribute('data-glass-quality', options.quality);
  await expect(page.locator('html')).toHaveAttribute('data-glass-ui-style', options.style);
  if (options.enabled && scheme === 'dark') await expect(page.locator('.wallpaper-image').last()).toBeVisible();
  if (scheme === 'light') await expect(page.locator('.wallpaper-layer').last()).toBeHidden();
  if (options.quality !== 'css') {
    await expect(page.locator('.etk-default-layout')).toHaveAttribute('data-shell-mode', 'desktop');
    await expect.poll(async () => {
      await stepCaptureFrame(page);
      await stepCaptureFrame(page);
      return page.locator('html').getAttribute('data-glass-renderer-state');
    }, { timeout: 180_000 }).toMatch(/ready|fallback/);
  }
  const rendering = await page.evaluate(() => ({ state: document.documentElement.dataset.glassRendererState ?? 'css',
    contexts: window.__followupGlassContexts, dynamics: document.documentElement.dataset.glassDynamicsMode,
    effectiveDynamics: document.documentElement.dataset.glassDynamicsEffectiveMode,
    viewport: { width: innerWidth, height: innerHeight, devicePixelRatio } }));
  if (rendering.contexts.some(Boolean)) expect(rendering.state).toBe('ready');
  if (rendering.state === 'ready') expect(rendering.effectiveDynamics).toBe(options.dynamics);
  return rendering;
}

async function captureInteraction(page, options, id, rendering) {
  if (options.quality === 'css') return;
  const region = await navbarInteractionRegion(page);
  const { clip, start, end } = region;
  const before = await page.screenshot({ clip, animations: 'disabled' });
  await page.mouse.move(start.x, start.y);
  await page.mouse.down();
  await page.mouse.move(end.x, end.y, { steps: 6 });
  rendering.inputFrame = await stepCaptureFrame(page);
  const after = await page.screenshot({ clip, animations: 'disabled' });
  await page.mouse.up();
  writeFileSync(`${artifactRoot}/${id}-interaction-before.png`, before);
  writeFileSync(`${artifactRoot}/${id}-interaction-after.png`, after);
  rendering.interaction = { requested: options.dynamics, effective: rendering.effectiveDynamics, changed: !before.equals(after), region };
  if (rendering.state === 'ready') expect(rendering.interaction.changed).toBe(true);
}

async function record(page, id, surface, rendering, probes) {
  for (const probe of probes) await expect(probe.locator, `${id}: ${probe.role}`).toBeInViewport({ ratio: 1 });
  const readings = await measureReadingContrast(page, probes, { gradientArtifactPrefix: surface === 'library' ? `${artifactRoot}/${id}-library-gradient` : undefined });
  writeFileSync(`${artifactRoot}/${id}-${surface}.json`, JSON.stringify({ id, surface, rendering, readings }, null, 2));
  await page.screenshot({ path: `${artifactRoot}/${id}-${surface}.png`, animations: 'disabled' });
  for (const reading of readings) expect(reading.contrast, `${id}: ${reading.role}`).toBeGreaterThanOrEqual(4.5);
  console.log(JSON.stringify({ id, surface, state: rendering.state, readings: readings.map(({ role, contrast }) => ({ role, contrast })) }));
}

for (const [index, options] of cases.entries()) {
  const id = `followup-${index + 1}-${options.scheme}-${options.enabled ? options.appearance : 'plain'}-${options.quality}-${options.style}-${options.dynamics}`;
  test(`${id} user cards, logs and library text stay readable`, async ({ page, api }) => {
    if (options.quality !== 'css') test.setTimeout(900_000);
    api.setTask({ is_running: false, state: 'failed', last_action: '用户同步', message: '请查看失败原因',
      logs: [`[12:30:43] [ERROR] ${liveBody}`] });
    api.respond('GET', '/api/logs/list', ['app.log']);
    api.respond('GET', '/api/logs/view', { file: 'app.log', lines: [`2026-11-10 12:30:43,000 - emby_toolkit::portal - ERROR - ${historyBody}`], page: 1, pages: 1, total_lines: 1 });
    await prepare(page, options);
    try {
      for (const scheme of options.scheme === 'auto' ? ['dark', 'light'] : [options.scheme]) {
        if (options.scheme === 'auto' && scheme === 'light') {
          await page.emulateMedia({ colorScheme: 'light' });
          await settle(page, options);
          await expect(page.locator('html')).toHaveAttribute('data-glass-color-scheme', 'light');
          await expect(page.locator('html')).toHaveAttribute('data-theme-preference', 'auto');
          await expect(page.locator('html')).toHaveAttribute('data-glass-ui-style', options.style);
          await expect(page.locator('.wallpaper-layer').last()).toBeHidden();
        }
        const captureId = options.scheme === 'auto' ? `${id}-${scheme}` : id;
        const userRendering = await openPage(page, options, '/user-management', scheme);
        await expect(page.locator('.user-card')).toHaveCount(2);
        await captureInteraction(page, options, captureId, userRendering);
        const current = page.locator('.user-card').filter({ has: page.getByText('UI Test', { exact: true }) });
        const disabled = page.locator('.user-card--disabled');
        await record(page, captureId, 'users', userRendering, [
          { role: 'current user name', locator: current.locator('.user-name') },
          { role: 'administrator role', locator: current.locator('.v-chip__content').nth(0) },
          { role: 'active user status', locator: current.locator('.v-chip__content').nth(1) },
          { role: 'disabled user name', locator: disabled.locator('.user-name') },
          { role: 'disabled user status', locator: disabled.locator('.v-chip__content').nth(1) },
          { role: 'user activity hint', locator: current.locator('.user-activity .text-medium-emphasis').first() },
        ]);
        await page.getByRole('button', { name: '实时日志', exact: true }).click();
        await settle(page, options);
        const live = page.locator('.notification-log .log-record').first();
        await expect(live.locator('.log-record-body')).toHaveText(liveBody);
        await record(page, captureId, 'live-logs', userRendering, [
          { role: 'live log body', locator: live.locator('.log-record-body') },
          { role: 'live log time', locator: live.locator('.log-record-time') },
          { role: 'live log error level', locator: live.locator('.log-record-level') },
        ]);
        await page.getByRole('tab', { name: '历史日志', exact: true }).click();
        await settle(page, options);
        const history = page.locator('.history-log-list .log-record').first();
        await expect(history.locator('.log-record-body')).toHaveText(historyBody);
        await record(page, captureId, 'history-logs', userRendering, [
          { role: 'history log body', locator: history.locator('.log-record-body') },
          { role: 'history log time', locator: history.locator('.log-record-time') },
          { role: 'history log error level', locator: history.locator('.log-record-level') },
          { role: 'history log source', locator: history.locator('.log-record-module') },
          { role: 'history log file', locator: history.locator('.log-record-file') },
        ]);
        await page.getByRole('button', { name: '关闭实时日志', exact: true }).click();
        const libraryRendering = await openPage(page, options, '/library/movies', scheme);
        await expect(page.locator('.media-card-title').first()).toHaveText('浏览器验证电影');
        await record(page, captureId, 'library', libraryRendering, [
          { role: 'library title', locator: page.locator('.media-center-sticky-controls .text-moviepilot') },
          { role: 'library count', locator: page.getByLabel('媒体数量 1', { exact: true }).locator('.v-chip__content') },
          { role: 'alphabet navigation', locator: page.getByRole('button', { name: '返回开头', exact: true }).locator('.v-btn__content') },
          { role: 'media card title', locator: page.locator('.media-card-title').first() },
          { role: 'media card year', locator: page.locator('.media-card-meta span').first() },
        ]);
      }
    } finally {
      if (options.quality !== 'css' && !page.isClosed()) await resumeCaptureFrames(page);
    }
  });
}
