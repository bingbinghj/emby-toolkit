import { readFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';

const initialConfig = JSON.parse(readFileSync(new URL('./config.json', import.meta.url)));
const storageFields = ['p115_auth_method', 'p115_app_id', 'p115_api_priority', 'p115_request_interval'];
const storageDialog = page => page.locator('.v-dialog.v-overlay--active').filter({
  has: page.getByRole('button', { name: '关闭存储配置', exact: true }),
});
const storageInput = (dialog, field) => dialog.locator(`[data-field="${field}"] input`).first();
const configWrites = api => api.requests.filter(request => request.method === 'POST' && request.path === '/api/config');
async function openStorage(page) {
  await page.getByRole('button', { name: '编辑 115 存储', exact: true }).click();
  const dialog = storageDialog(page);
  await expect(dialog).toBeVisible();
  return dialog;
}

test('115 summary opens its MP width dialog by keyboard and uses fullscreen on mobile', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/setting?tab=directory');
  const card = page.getByRole('button', { name: '编辑 115 存储', exact: true });
  await expect(card.getByText('115 网盘', { exact: true })).toBeVisible();
  await card.focus();
  await card.press('Enter');
  const desktop = storageDialog(page);
  await expect(desktop).toBeVisible();
  const size = await desktop.locator('.v-overlay__content').evaluate(element => ({
    width: element.getBoundingClientRect().width,
    rem: parseFloat(getComputedStyle(document.documentElement).fontSize),
    maxWidth: parseFloat(getComputedStyle(element).maxWidth),
  }));
  expect(size.maxWidth).toBeCloseTo(size.rem * 40, 0);
  expect(size.width).toBeCloseTo(size.rem * 40, 0);
  await desktop.getByRole('button', { name: '取消存储配置', exact: true }).click();
  await expect(desktop).toBeHidden();

  await page.setViewportSize({ width: 390, height: 844 });
  const mobile = await openStorage(page);
  await expect(mobile).toHaveClass(/v-dialog--fullscreen/);
  const bounds = await mobile.locator('.v-overlay__content').boundingBox();
  expect(bounds.width).toBeGreaterThanOrEqual(389);
  expect(bounds.height).toBeGreaterThanOrEqual(843);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(391);
  await mobile.getByRole('button', { name: '关闭存储配置', exact: true }).click();
  expect(configWrites(api)).toEqual([]);
});

test('cancelling 115 configuration discards its draft and preserves another card draft', async ({ page, api }) => {
  const original = { ...initialConfig, p115_app_id: 'original-app-id' };
  api.respond('GET', '/api/config', original);
  await page.goto('/setting?tab=directory');
  await page.locator('[data-field="local_strm_root"] input').fill('/config/pending-other-card');
  const dialog = await openStorage(page);
  await storageInput(dialog, 'p115_request_interval').fill('1.6');
  await dialog.getByRole('radio', { name: '自定义 AppID 扫码', exact: true }).check();
  await storageInput(dialog, 'p115_app_id').fill('unsaved-app-id');
  await dialog.getByRole('radio', { name: '优先 OpenAPI（备用）', exact: true }).check();
  await dialog.getByRole('button', { name: '取消存储配置', exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(configWrites(api)).toEqual([]);

  const reopened = await openStorage(page);
  await expect(storageInput(reopened, 'p115_request_interval')).toHaveValue(String(original.p115_request_interval));
  await expect(reopened.getByRole('radio', { name: '网页登录授权', exact: true })).toBeChecked();
  await expect(reopened.getByRole('radio', { name: '优先 Cookie（默认）', exact: true })).toBeChecked();
  await reopened.getByRole('radio', { name: '自定义 AppID 扫码', exact: true }).check();
  await expect(storageInput(reopened, 'p115_app_id')).toHaveValue('original-app-id');
  await reopened.getByRole('button', { name: '关闭存储配置', exact: true }).click();
  await expect(page.locator('[data-field="local_strm_root"] input')).toHaveValue('/config/pending-other-card');
  expect(configWrites(api)).toEqual([]);
});

test('115 failed save retains its draft and retry merges only canonical storage fields', async ({ page, api }) => {
  api.respond('GET', '/api/config', { ...initialConfig, p115_auth_method: 'qrcode', p115_app_id: 'saved-app-id' });
  api.respond('POST', '/api/config', { error: '115 配置写入失败' }, 500);
  await page.goto('/setting?tab=directory');
  await page.locator('[data-field="local_strm_root"] input').fill('/config/keep-other-card');
  const dialog = await openStorage(page);
  await storageInput(dialog, 'p115_request_interval').fill('1.2');
  await storageInput(dialog, 'p115_app_id').fill('edited-app-id');
  await dialog.getByRole('radio', { name: '优先 OpenAPI（备用）', exact: true }).check();
  await dialog.getByRole('button', { name: '保存存储', exact: true }).click();
  await expect(dialog.getByText('115 配置写入失败', { exact: true })).toBeVisible();
  await expect(storageInput(dialog, 'p115_request_interval')).toHaveValue('1.2');
  await expect(storageInput(dialog, 'p115_app_id')).toHaveValue('edited-app-id');
  expect(configWrites(api)).toHaveLength(1);

  api.respond('POST', '/api/config', request => ({ saved_fields: {
    ...request.body, p115_request_interval: 1.3, p115_app_id: 'canonical-app-id',
    local_strm_root: '/config/unrequested-server-value',
  } }));
  await dialog.getByRole('button', { name: '保存存储', exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(configWrites(api)).toHaveLength(2);
  for (const request of configWrites(api)) {
    expect(Object.keys(request.body).sort()).toEqual([...storageFields].sort());
    expect(request.body.p115_request_interval).toBe(1.2);
    expect(request.body.p115_app_id).toBe('edited-app-id');
    expect(request.body.p115_api_priority).toBe('openapi');
  }
  const reopened = await openStorage(page);
  await expect(storageInput(reopened, 'p115_request_interval')).toHaveValue('1.3');
  await expect(storageInput(reopened, 'p115_app_id')).toHaveValue('canonical-app-id');
  await reopened.getByRole('button', { name: '取消存储配置', exact: true }).click();
  await expect(page.locator('[data-field="local_strm_root"] input')).toHaveValue('/config/keep-other-card');
});

test('115 setting search opens and focuses the dialog and an unsaved AppID cannot authorize the old AppID', async ({ page, api }) => {
  api.respond('GET', '/api/config', { ...initialConfig, p115_auth_method: 'qrcode', p115_app_id: 'saved-app-id' });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/setting?tab=system');
  await page.getByRole('searchbox', { name: '搜索设置项', exact: true }).fill('API 请求间隔');
  await page.getByRole('button', { name: '打开设置：API 请求间隔 (秒)', exact: true }).click();
  await expect(page).toHaveURL(/tab=directory/);
  const dialog = storageDialog(page);
  await expect(dialog).toBeVisible();
  const interval = storageInput(dialog, 'p115_request_interval');
  await expect(interval).toBeFocused();
  await expect(dialog.locator('[data-field="p115_request_interval"]')).toHaveClass(/settings-search-highlight/);
  await storageInput(dialog, 'p115_app_id').fill('unsaved-app-id');
  await dialog.getByRole('button', { name: '扫码授权', exact: true }).click();
  await expect(page.getByText('请先保存自定义 AppID 和授权方式，再扫码授权。', { exact: true }).first()).toBeVisible();
  expect(api.requests.filter(request => request.path === '/api/p115/qrcode')).toEqual([]);
  expect(configWrites(api)).toEqual([]);
  await dialog.getByRole('button', { name: '取消存储配置', exact: true }).click();
});
