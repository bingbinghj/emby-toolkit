import { test, expect } from './fixtures.js';

test('MP settings use one navigation entry and working header categories', async ({ page }) => {
  await page.goto('/setting');
  const navigation = page.locator('.v-navigation-drawer');
  await expect(navigation.getByText('设置', { exact: true })).toHaveCount(1);
  await expect(navigation.getByText('通知设置', { exact: true })).toHaveCount(0);
  await expect(page.getByRole('tab', { name: '系统', exact: true })).toHaveAttribute('aria-selected', 'true');
  await page.getByRole('button', { name: '编辑 Emby 媒体服务器', exact: true }).click();
  const serverDialog = page.locator('.v-dialog.v-overlay--active').filter({ has: page.getByRole('button', { name: '关闭媒体服务器配置', exact: true }) });
  await expect(serverDialog.getByLabel('地址', { exact: true })).toBeVisible();
  await serverDialog.getByRole('button', { name: '取消媒体服务器配置', exact: true }).click();
  await expect(serverDialog).toBeHidden();
  await page.getByRole('tab', { name: '存储 & 目录', exact: true }).click();
  await expect(page).toHaveURL(/tab=directory/);
  await expect(page.getByLabel('媒体元数据目录', { exact: true })).toBeVisible();
  await page.getByRole('tab', { name: '规则', exact: true }).click();
  await expect(page.getByRole('button', { name: '保存实时监控', exact: true })).toBeVisible();
  await page.getByRole('tab', { name: '数据源', exact: true }).click();
  await expect(page.getByLabel('TMDB API Key', { exact: true })).toBeVisible();
  await page.getByRole('tab', { name: '通知', exact: true }).click();
  await expect(page).toHaveURL(/settings\/notifications/);
  await expect(page.locator('.notification-channel')).toBeVisible();
  await page.reload();
  await expect(page.getByRole('tab', { name: '通知', exact: true })).toHaveAttribute('aria-selected', 'true');
});

test('mobile header settings tabs stay within the viewport', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/setting?tab=search');
  const tabs = page.locator('.settings-header-tabs');
  await expect(tabs).toBeVisible();
  await page.getByRole('tab', { name: '通知', exact: true }).click();
  await expect(page.locator('.notification-channel')).toBeVisible();
  const navbar = await page.locator('.layout-navbar').boundingBox();
  const content = await page.locator('.notification-channel').boundingBox();
  expect(content.y).toBeGreaterThan(navbar.y + navbar.height);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(392);
});

test('MP notification deep links open the notification settings category', async ({ page }) => {
  await page.goto('/setting?tab=notification');
  await expect(page.getByRole('tab', { name: '通知', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('.notification-channel')).toBeVisible();
});
