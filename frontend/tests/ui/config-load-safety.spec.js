import { readFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';

const coverConfig = JSON.parse(readFileSync(new URL('./cover-config.json', import.meta.url)));
const coverPosts = api => api.requests.filter(request => request.method === 'POST' && request.path === '/api/config/cover_generator');

test('cover read failure blocks saving until a retry loads the authoritative configuration', async ({ page, api }) => {
  api.respond('GET', '/api/config/cover_generator', { error: '封面配置暂时不可读' }, 500);
  await page.goto('/settings/cover-generator');
  await expect.poll(() => api.requests.some(request => request.path === '/api/config/cover_generator')).toBe(true);
  await expect(page.getByRole('alert').filter({ hasText: '封面配置暂时不可读' })).toBeVisible();
  await expect(page.locator('.v-card').filter({ hasText: '基础设置' }).getByRole('button', { name: '保存', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: '生成全部封面', exact: true })).toBeDisabled();
  expect(coverPosts(api)).toEqual([]);

  api.respond('GET', '/api/config/cover_generator', structuredClone(coverConfig));
  await page.getByRole('button', { name: '重试加载配置', exact: true }).click();
  await page.getByLabel('启用封面生成', { exact: true }).check();
  await page.getByLabel('多图 1', { exact: true }).check();
  await page.locator('.v-card').filter({ hasText: '基础设置' }).getByRole('button', { name: '保存', exact: true }).click();
  await expect.poll(() => coverPosts(api).length).toBe(1);
  const payload = coverPosts(api)[0].body;
  expect(payload.enabled).toBe(true);
  expect(payload.cover_style).toBe(coverConfig.cover_style);
  expect(payload.zh_font_path_local).toBe(coverConfig.zh_font_path_local);
  await expect(page.getByLabel('多图 1', { exact: true })).toBeChecked();
  await expect(page.getByText('配置已成功保存！', { exact: true })).toBeVisible();
});

test('cover settings remain editable after an authoritative empty configuration is loaded', async ({ page, api }) => {
  api.respond('GET', '/api/config/cover_generator', {});
  await page.goto('/settings/cover-generator');
  await page.getByLabel('启用封面生成', { exact: true }).check();
  await page.locator('.v-card').filter({ hasText: '基础设置' }).getByRole('button', { name: '保存', exact: true }).click();
  await expect.poll(() => coverPosts(api).length).toBe(1);
  expect(coverPosts(api)[0].body.enabled).toBe(true);
  await expect(page.getByRole('button', { name: '重试加载配置', exact: true })).toHaveCount(0);
  await expect(page.getByText('配置已成功保存！', { exact: true })).toBeVisible();
});
