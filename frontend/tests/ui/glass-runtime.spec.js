import { test, expect } from './fixtures.js';

test.describe.configure({ timeout: 60_000 });

const THEME_KEY = 'moviepilot-theme-customizer';
const WALLPAPER_KEY = 'etk-wallpaper-settings';

async function initializeGlass(page, { appearance = 'clear', quality = 'balanced', wallpaper = 'none' } = {}) {
  await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
  await page.addInitScript(({ themeKey, wallpaperKey, appearance, quality, wallpaper }) => {
    localStorage.setItem(themeKey, JSON.stringify({
      theme: 'dark', glassEnabled: true, glassAppearance: appearance, glassQuality: quality,
    }));
    localStorage.setItem(wallpaperKey, JSON.stringify({ mode: wallpaper, presetId: 'dusk', rotationInterval: 0 }));
  }, { themeKey: THEME_KEY, wallpaperKey: WALLPAPER_KEY, appearance, quality, wallpaper });
  await page.goto('/user-management', { waitUntil: 'domcontentloaded' });
  await page.bringToFront();
  await expect(page.locator('.user-management-page')).toBeVisible();
}

async function expectConnectedDefinitions(page) {
  const shell = page.locator('.etk-default-layout');
  await expect(shell).toHaveAttribute('data-glass-navigation-refraction', 'chromium');
  await expect(shell.locator('svg.glass-navbar-refraction-defs')).toHaveCount(1);
  await expect(shell.locator('svg.glass-panel-refraction-defs')).toHaveCount(1);
  await expect.poll(() => page.evaluate(() => {
    const shell = document.querySelector('.etk-default-layout');
    const panel = document.querySelector('svg.glass-panel-refraction-defs');
    return panel?.closest('.layout-wrapper') === shell;
  })).toBe(true);
  await expect.poll(() => page.evaluate(() => {
    const shell = document.querySelector('.etk-default-layout');
    const navbar = shell?.querySelector('.layout-navbar');
    return navbar?.hasAttribute('data-glass-panel-refraction')
      || shell?.getAttribute('data-glass-navbar-refraction-ready') === 'true';
  })).toBe(true);
}

test('one global wallpaper instance owns the initial display preload and shares tone decoding', async ({ page }) => {
  await page.emulateMedia({ colorScheme: 'light', reducedMotion: 'reduce' });
  await page.addInitScript(() => {
    const OriginalImage = window.Image;
    const source = Object.getOwnPropertyDescriptor(HTMLImageElement.prototype, 'src');
    window.__wallpaperPreloads = [];
    window.Image = function (...args) {
      const image = new OriginalImage(...args);
      Object.defineProperty(image, 'src', {
        get: () => source.get.call(image),
        set: value => {
          window.__wallpaperPreloads.push({ url: value, crossOrigin: image.crossOrigin });
          source.set.call(image, value);
        },
      });
      return image;
    };
    window.Image.prototype = OriginalImage.prototype;
  });
  await page.goto('/user-management', { waitUntil: 'domcontentloaded' });
  await expect(page.locator('.user-management-page')).toBeVisible();
  await expect(page.locator('.wallpaper-background')).toHaveCount(1);
  await expect.poll(() => page.evaluate(() => window.__wallpaperPreloads.filter(image => image.url === '/wallpapers/dusk.svg' && image.crossOrigin !== 'anonymous').length)).toBe(1);
});

test('glass definitions bind the real shell and reacquire navigation after layout changes', async ({ page, api }) => {
  api.respond('GET', '/api/portal/users', { users: Array.from({ length: 24 }, (_, index) => ({
    id: index === 0 ? 'ui-test-user' : index.toString(16).padStart(32, '0'),
    name: index === 0 ? 'UI Test' : `Scrolling Member ${index}`,
    is_administrator: index === 0, is_disabled: false,
    notes: '', telegram_chat_id: '', profile_image_tag: null,
    last_activity_date: '2026-11-10T01:00:00Z', last_login_date: '2026-11-10T00:00:00Z',
  })) });
  await initializeGlass(page);
  await expect(page.locator('.user-card')).toHaveCount(24);
  await expectConnectedDefinitions(page);
  await expect.poll(() => page.locator('svg.glass-panel-refraction-defs filter').count()).toBeGreaterThan(0);

  await page.getByRole('button', { name: '主题与壁纸', exact: true }).click();
  const panel = page.locator('.theme-customizer-panel-host');
  await expect(panel).toBeVisible();
  await panel.getByRole('button', { name: '水平', exact: true }).click();
  await expect(page.locator('.etk-default-layout')).toHaveClass(/layout-horizontal-nav-active/);
  await panel.getByLabel('关闭', { exact: true }).click();
  await expectConnectedDefinitions(page);
  await expect.poll(() => page.locator('.layout-navbar').evaluate(navbar => {
    const styles = getComputedStyle(navbar);
    const pixels = name => {
      const value = styles.getPropertyValue(name).trim();
      return Number.parseFloat(value) * (value.endsWith('rem') ? Number.parseFloat(getComputedStyle(document.documentElement).fontSize) : 1);
    };
    return { inset: pixels('--shell-floating-navbar-inset'), radius: pixels('--shell-floating-navbar-radius') };
  })).toEqual({ inset: 16, radius: 16 });

  await page.setViewportSize({ width: 1440, height: 650 });
  await page.evaluate(() => window.scrollTo(0, 200));
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThanOrEqual(200);
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(12);
  await expect(page.locator('.etk-default-layout')).toHaveClass(/layout-navbar-away-from-top/);
  await expectConnectedDefinitions(page);
  await page.evaluate(() => window.scrollTo(0, 0));
  await expect(page.locator('.etk-default-layout')).not.toHaveClass(/layout-navbar-away-from-top/);
  await expectConnectedDefinitions(page);

  await page.getByRole('button', { name: '主题与壁纸', exact: true }).click();
  await panel.getByRole('button', { name: '垂直', exact: true }).click();
  await panel.getByLabel('关闭', { exact: true }).click();
  await expect(page.locator('.layout-vertical-nav')).toBeVisible();
  await expectConnectedDefinitions(page);
  await expect.poll(() => page.evaluate(() => {
    const sidebar = document.querySelector('.layout-vertical-nav');
    const shell = document.querySelector('.etk-default-layout');
    return sidebar?.hasAttribute('data-glass-panel-refraction')
      || shell?.getAttribute('data-glass-sidebar-refraction-ready') === 'true';
  })).toBe(true);
});

test('frosted desktop consumes the fixed wallpaper backplate and releases it for mobile', async ({ page }) => {
  await initializeGlass(page, { appearance: 'frosted', quality: 'css', wallpaper: 'preset' });
  const shell = page.locator('.etk-default-layout');
  await expect(shell).toHaveAttribute('data-shell-mode', 'desktop');
  await expect(shell).toHaveAttribute('data-shell-navbar-attachment', 'connected');
  await expect(shell).toHaveClass(/layout-fixed-shell-backplate-active/);
  await expect(shell.locator('.glass-fixed-shell-backplate--main')).toHaveCount(1);
  await expect.poll(() => page.evaluate(() => Boolean(
    document.querySelector('.glass-fixed-shell-backplate--main')?.closest('.layout-wrapper'),
  ))).toBe(true);

  await page.setViewportSize({ width: 390, height: 844 });
  await expect(shell).toHaveAttribute('data-shell-mode', 'app');
  await expect(page.locator('.glass-fixed-shell-backplate--main')).toHaveCount(0);
  await expect(shell).not.toHaveClass(/layout-fixed-shell-backplate-active/);
  await page.setViewportSize({ width: 1440, height: 900 });
  await expect(shell).toHaveAttribute('data-shell-mode', 'desktop');
  await expect(shell.locator('.glass-fixed-shell-backplate--main')).toHaveCount(1);
});
