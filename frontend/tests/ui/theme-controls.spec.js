import { test, expect } from './fixtures.js';

test.describe.configure({ timeout: 60_000 });
const THEME_KEY = 'moviepilot-theme-customizer';

async function openAppearance(page) {
  const trigger = page.getByRole('button', { name: '主题与壁纸', exact: true });
  await trigger.click();
  const panel = page.locator('.theme-customizer-panel-host');
  await expect(panel).toBeVisible();
  return { panel, trigger };
}

async function openGlassSettings(page, panel) {
  await panel.getByRole('button', { name: '玻璃设置', exact: true }).click();
  const dialog = page.locator('.v-dialog.v-overlay--active');
  await expect(dialog.getByText('玻璃设置', { exact: true })).toBeVisible();
  return dialog;
}

test.beforeEach(async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce', colorScheme: 'light' });
  await page.goto('/user-management', { waitUntil: 'domcontentloaded' });
  await expect(page.getByRole('heading', { name: '用户管理', exact: true })).toBeVisible();
});

test('theme side panel takes keyboard focus and returns it to the trigger on close', async ({ page }) => {
  const trigger = page.getByRole('button', { name: '主题与壁纸', exact: true });
  await trigger.focus();
  await trigger.press('Enter');
  const panel = page.locator('.theme-customizer-panel-host');
  await expect(panel).toBeVisible();
  await expect.poll(() => panel.evaluate(element => element.contains(document.activeElement))).toBe(true);
  await page.keyboard.press('Escape');
  await expect(panel).not.toBeVisible();
  await expect(trigger).toBeFocused();

  await trigger.press('Enter');
  await expect(panel).toBeVisible();
  const backgroundControl = page.getByRole('button', { name: 'UI Test 的用户菜单', exact: true });
  await backgroundControl.focus();
  await page.keyboard.press('Escape');
  await expect(panel).not.toBeVisible();
  await expect(backgroundControl).toBeFocused();
});

test('glass settings are reachable and Escape only dismisses the active dialog', async ({ page }) => {
  const { panel } = await openAppearance(page);
  const dialog = await openGlassSettings(page, panel);
  await page.keyboard.press('Escape');
  await expect(dialog).not.toBeVisible();
  await expect(panel).toBeVisible();
  await expect(page.getByRole('heading', { name: '用户管理', exact: true })).toBeVisible();
});

test('glass preview cancels without writes and saved quality survives reload', async ({ page }) => {
  const { panel } = await openAppearance(page);
  const originalStorage = await page.evaluate(key => localStorage.getItem(key), THEME_KEY);
  const originalAppearance = await page.locator('html').getAttribute('data-glass-appearance');
  let dialog = await openGlassSettings(page, panel);
  await dialog.locator('.glass-settings-dialog__appearance').getByRole('button', { name: '磨砂', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-glass-appearance', 'frosted');
  expect(await page.evaluate(key => localStorage.getItem(key), THEME_KEY)).toBe(originalStorage);
  await dialog.getByRole('button', { name: '关闭', exact: true }).click();
  await expect(dialog).not.toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-glass-appearance', originalAppearance);
  expect(await page.evaluate(key => localStorage.getItem(key), THEME_KEY)).toBe(originalStorage);

  dialog = await openGlassSettings(page, panel);
  await dialog.locator('.glass-settings-dialog__appearance').getByRole('button', { name: '磨砂', exact: true }).click();
  await dialog.locator('.glass-settings-dialog__quality').getByRole('button', { name: '高质量', exact: true }).click();
  await dialog.getByRole('button', { name: '保存', exact: true }).click();
  await expect(dialog).not.toBeVisible();
  await expect.poll(() => page.evaluate(key => JSON.parse(localStorage.getItem(key))?.glassQuality, THEME_KEY)).toBe('high');
  await page.reload({ waitUntil: 'domcontentloaded' });
  await expect(page.getByRole('heading', { name: '用户管理', exact: true })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-glass-quality', 'high');
  await expect(page.locator('html')).toHaveAttribute('data-glass-appearance', 'frosted');
});

test('custom middle-gray accents keep readable text on visible plain buttons', async ({ page }) => {
  await page.goto('/settings/general', { waitUntil: 'domcontentloaded' });
  await expect(page.getByRole('heading', { name: '通用设置', exact: true })).toBeVisible();
  const { panel } = await openAppearance(page);
  await panel.locator('.theme-customizer-glass-toggle input').uncheck({ force: true });
  await panel.locator('input[type="color"]').evaluate(input => {
    input.value = '#999999';
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'plain');
  await expect.poll(() => page.evaluate(key => JSON.parse(localStorage.getItem(key))?.primaryColor, THEME_KEY)).toBe('#999999');
  await page.keyboard.press('Escape');
  await page.getByRole('tab', { name: /115/ }).click();
  await page.getByRole('button', { name: '账户与连接', exact: true }).click();
  const button = page.getByRole('button', { name: '扫码获取', exact: true });
  await expect(button).toBeVisible();
  const colors = await button.evaluate(element => {
    const styles = getComputedStyle(element);
    return { text: styles.color, background: styles.backgroundColor };
  });
  const luminance = color => color.match(/\d+(?:\.\d+)?/g).slice(0, 3).map(Number)
    .reduce((sum, byte, index) => {
      const value = byte / 255;
      return sum + (value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4) * [0.2126, 0.7152, 0.0722][index];
    }, 0);
  const text = luminance(colors.text), background = luminance(colors.background);
  const contrast = (Math.max(text, background) + 0.05) / (Math.min(text, background) + 0.05);
  expect(contrast).toBeGreaterThanOrEqual(4.5);
  await page.reload({ waitUntil: 'domcontentloaded' });
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'plain');
  await expect.poll(() => page.evaluate(key => JSON.parse(localStorage.getItem(key))?.primaryColor, THEME_KEY)).toBe('#999999');
});
