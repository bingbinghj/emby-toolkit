import { test, expect } from './fixtures.js';

// Theme and wallpaper cases deliberately include full page reloads and image decoding.
test.describe.configure({ timeout: 60_000 });

const CUSTOMIZER_KEY = 'moviepilot-theme-customizer';
const WALLPAPER_KEY = 'etk-wallpaper-settings';
const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jfuoAAAAASUVORK5CYII=', 'base64');

async function openAppearance(page) {
  await page.getByRole('button', { name: '主题与壁纸', exact: true }).click();
  await expect(page.locator('.theme-customizer-panel-host')).toBeVisible();
  return page.locator('.theme-customizer-panel-host');
}

async function savedWallpaper(page) {
  return page.evaluate(key => JSON.parse(localStorage.getItem(key)), WALLPAPER_KEY);
}

async function uploadWallpaper(page, panel) {
  await panel.getByRole('button', { name: '自定义', exact: true }).click();
  await panel.getByLabel('上传壁纸图片', { exact: true }).setInputFiles({
    name: 'custom-wallpaper.png', mimeType: 'image/png', buffer: png,
  });
  await expect.poll(async () => (await savedWallpaper(page))?.customImage).toMatch(/^data:image\/jpeg;base64,/);
  return (await savedWallpaper(page)).customImage;
}

test.beforeEach(async ({ page, api }) => {
  await page.emulateMedia({ reducedMotion: 'reduce', colorScheme: 'light' });
  await page.goto('/user-management', { waitUntil: 'domcontentloaded' });
  await expect(page.getByRole('heading', { name: '用户管理', exact: true })).toBeVisible();
});

test('manual light and dark themes persist and ignore later system changes', async ({ page }) => {
  const panel = await openAppearance(page);
  const choices = panel.locator('.theme-customizer-option-grid--theme > button');
  await expect(choices).toHaveCount(3);

  await choices.nth(1).click();
  await expect(choices.nth(1)).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('html')).toHaveClass(/(?:^|\s)dark(?:\s|$)/);
  await expect.poll(() => page.evaluate(key => JSON.parse(localStorage.getItem(key))?.theme, CUSTOMIZER_KEY)).toBe('dark');
  await page.emulateMedia({ colorScheme: 'light' });
  await expect(page.locator('html')).toHaveClass(/(?:^|\s)dark(?:\s|$)/);
  await page.reload({ waitUntil: 'domcontentloaded' });
  await expect(page.locator('html')).toHaveClass(/(?:^|\s)dark(?:\s|$)/);

  const reopened = await openAppearance(page);
  await reopened.locator('.theme-customizer-option-grid--theme > button').nth(0).click();
  await expect(page.locator('html')).toHaveClass(/(?:^|\s)light(?:\s|$)/);
  await page.emulateMedia({ colorScheme: 'dark' });
  await expect(page.locator('html')).toHaveClass(/(?:^|\s)light(?:\s|$)/);
  await expect.poll(() => page.evaluate(key => JSON.parse(localStorage.getItem(key))?.theme, CUSTOMIZER_KEY)).toBe('light');
});

test('auto theme follows live system preference changes and preserves auto after reload', async ({ page }) => {
  const panel = await openAppearance(page);
  const automatic = panel.locator('.theme-customizer-option-grid--theme > button').nth(2);
  await automatic.click();
  await expect(automatic).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('html')).toHaveClass(/(?:^|\s)light(?:\s|$)/);
  await page.emulateMedia({ colorScheme: 'dark' });
  await expect(page.locator('html')).toHaveClass(/(?:^|\s)dark(?:\s|$)/);
  await page.emulateMedia({ colorScheme: 'light' });
  await expect(page.locator('html')).toHaveClass(/(?:^|\s)light(?:\s|$)/);
  await expect.poll(() => page.evaluate(key => JSON.parse(localStorage.getItem(key))?.theme, CUSTOMIZER_KEY)).toBe('auto');
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.reload({ waitUntil: 'domcontentloaded' });
  await expect(page.locator('html')).toHaveClass(/(?:^|\s)dark(?:\s|$)/);
});

test('bundled wallpaper presets load locally and retain the selected preset after reload', async ({ page }) => {
  const panel = await openAppearance(page);
  await panel.getByRole('button', { name: '湖畔晨光', exact: true }).click();
  await expect(panel.getByRole('button', { name: '湖畔晨光', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await expect.poll(async () => (await savedWallpaper(page))?.presetId).toBe('lake');
  const background = page.locator('.wallpaper-background img').last();
  await expect(background).toHaveAttribute('src', '/wallpapers/lake.svg');
  await expect.poll(() => background.evaluate(img => img.complete && img.naturalWidth > 0)).toBe(true);
  await page.reload({ waitUntil: 'domcontentloaded' });
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', '/wallpapers/lake.svg');
});

test('custom image upload is stored in the browser and survives navigation and reload', async ({ page }) => {
  const panel = await openAppearance(page);
  const image = await uploadWallpaper(page, panel);
  await expect(panel.getByAltText('当前壁纸预览', { exact: true })).toHaveAttribute('src', image);
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', image);
  await page.keyboard.press('Escape');
  await expect(panel).not.toBeVisible();
  await page.goto('/review', { waitUntil: 'domcontentloaded' });
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', image);
  await page.reload({ waitUntil: 'domcontentloaded' });
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', image);
  await expect.poll(async () => (await savedWallpaper(page))?.mode).toBe('custom');
});

test('failed custom wallpaper URLs report errors and retain the last valid wallpaper', async ({ page }) => {
  const panel = await openAppearance(page);
  const image = await uploadWallpaper(page, panel);
  await page.route('https://wallpaper.invalid/missing.png', route => route.abort('failed'));
  await panel.getByLabel('图片地址', { exact: true }).fill('https://wallpaper.invalid/missing.png');
  await panel.getByRole('button', { name: '应用地址', exact: true }).click();
  await expect(page.getByText('图片加载失败，请检查地址或选择其他图片。', { exact: true })).toBeVisible();
  expect((await savedWallpaper(page)).customImage).toBe(image);
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', image);

  await panel.getByLabel('图片地址', { exact: true }).fill('javascript:alert(1)');
  await panel.getByRole('button', { name: '应用地址', exact: true }).click();
  await expect(page.getByText('请输入有效的 HTTP / HTTPS 图片地址。', { exact: true })).toBeVisible();
  expect((await savedWallpaper(page)).customImage).toBe(image);
});
