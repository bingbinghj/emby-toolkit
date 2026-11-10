import { test, expect } from './fixtures.js';
import { readFileSync } from 'node:fs';
const config = JSON.parse(readFileSync(new URL('./config.json', import.meta.url)));
const headerSearch = page => page.getByRole('searchbox', { name: '搜索设置项', exact: true });
const result = (page, label) => page.getByRole('button', { name: `打开设置：${label}`, exact: true });
const writes = api => api.requests.filter(request => ['POST', 'PUT', 'DELETE', 'PATCH'].includes(request.method) && request.path !== '/api/config/rename_preview');
test.beforeEach(async ({ page }) => { await page.setViewportSize({ width: 1440, height: 900 }); });

test('header settings search crosses categories, highlights the target and retains unsaved drafts', async ({ page, api }) => {
  await page.goto('/setting?tab=system');
  await page.locator('[data-field="delay_between_items_sec"] input').fill('1.2');
  await headerSearch(page).fill('电影重命名格式');
  await expect(result(page, '电影重命名格式')).toBeVisible();
  await expect(page.locator('.setting-card').filter({ has: page.getByText('基础设置', { exact: true }) })).toBeHidden();
  await result(page, '电影重命名格式').click();
  await expect(page).toHaveURL(/tab=rule/);
  await expect(page.locator('[data-field="movie_rename_format"]')).toHaveClass(/settings-search-highlight/);
  await headerSearch(page).clear();
  await page.getByRole('tab', { name: '系统', exact: true }).click();
  await expect(page.locator('[data-field="delay_between_items_sec"] input')).toHaveValue('1.2');
  await expect(page.locator('.setting-card:visible')).toHaveCount(5);
  expect(writes(api)).toEqual([]);
});

test('hint searches reach the local metadata directory picker', async ({ page }) => {
  await page.goto('/setting?tab=system');
  await headerSearch(page).fill('metadata');
  await result(page, '媒体元数据目录').click();
  await expect(page).toHaveURL(/tab=directory/);
  await expect(page.locator('[data-field="emby_metadata_path"]')).toHaveClass(/settings-search-highlight/);
  await expect(page.getByRole('button', { name: '选择媒体元数据目录', exact: true })).toBeVisible();
});

test('unmatched searches hide unrelated settings and clearing restores all current cards', async ({ page, api }) => {
  await page.goto('/setting?tab=system');
  await headerSearch(page).fill('不存在设置xyz');
  await expect(page.getByText('没有匹配的设置项', { exact: true })).toBeVisible();
  await expect(page.locator('.setting-card:visible')).toHaveCount(0);
  await headerSearch(page).clear();
  await expect(page.locator('.setting-card:visible')).toHaveCount(5);
  expect(writes(api)).toEqual([]);
});

test('a conditionally hidden setting points to its real enabling field without changing it', async ({ page, api }) => {
  api.respond('GET', '/api/config', { ...config, ai_translate_actor_role: false, ai_translate_title_overview: false });
  await page.goto('/setting?tab=system');
  await headerSearch(page).fill('翻译模式');
  await result(page, '翻译模式').click();
  await expect(page.locator('[data-field="ai_translate_actor_role"]')).toHaveClass(/settings-search-highlight/);
  await expect(page.getByText('启用翻译演员与角色后可设置翻译模式。', { exact: true })).toBeVisible();
  await expect(page.getByRole('checkbox', { name: '翻译演员与角色', exact: true })).not.toBeChecked();
  expect(writes(api)).toEqual([]);
});

test('general settings can locate notification channel fields in the real edit dialog', async ({ page, api }) => {
  await page.goto('/setting?tab=system');
  await headerSearch(page).fill('Bot Token');
  await result(page, 'Bot Token').click();
  await expect(page).toHaveURL(/\/settings\/notifications$/);
  const field = page.locator('[data-field="telegram_bot_token"]');
  await expect(field).toHaveClass(/settings-search-highlight/);
  await expect(field.locator('input')).toHaveAttribute('type', 'password');
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.getByRole('button', { name: '取消', exact: true }).click();
  expect(writes(api)).toEqual([]);
});

test('a personal receiver draft survives following results between notification and general settings', async ({ page, api }) => {
  await page.goto('/settings/notifications');
  await page.getByRole('button', { name: '编辑 Telegram 通知渠道', exact: true }).click();
  await page.getByLabel('个人 Chat ID', { exact: true }).fill('pending-personal');
  await page.getByRole('button', { name: '确定', exact: true }).click();
  await headerSearch(page).fill('电影重命名格式');
  await result(page, '电影重命名格式').click();
  await expect(page).toHaveURL(/tab=rule/);
  await headerSearch(page).fill('个人 Chat ID');
  await result(page, '个人 Chat ID').click();
  await expect(page).toHaveURL(/\/settings\/notifications$/);
  await expect(page.getByLabel('个人 Chat ID', { exact: true })).toHaveValue('pending-personal');
  await page.getByRole('button', { name: '取消', exact: true }).click();
  expect(writes(api)).toEqual([]);
});

test('mobile header search selects an actual setting and closes its input dialog', async ({ page, api }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/setting?tab=system');
  await page.getByRole('button', { name: '打开搜索设置项', exact: true }).click();
  await headerSearch(page).fill('电影重命名格式');
  await headerSearch(page).press('Enter');
  await expect(page).toHaveURL(/tab=rule/);
  await expect(page.getByRole('button', { name: '关闭搜索', exact: true })).toHaveCount(0);
  await expect(page.locator('[data-field="movie_rename_format"]')).toHaveClass(/settings-search-highlight/);
  expect(writes(api)).toEqual([]);
});
