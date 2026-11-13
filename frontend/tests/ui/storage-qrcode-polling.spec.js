import { readFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';

const config = JSON.parse(readFileSync(new URL('./config.json', import.meta.url)));
const qrReads = api => api.requests.filter(request => request.path === '/api/p115/qrcode/status');

test('a slow 115 QR status read does not overlap and its success closes exactly one authorization session', async ({ page, api }) => {
  api.respond('GET', '/api/config', { ...config, p115_auth_method: 'qrcode', p115_app_id: 'saved-app-id' });
  api.respond('POST', '/api/p115/qrcode', { success: true, data: { qrcode: 'data:image/svg+xml,<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200"/>' } });
  const pending = api.defer('GET', '/api/p115/qrcode/status');
  await page.goto('/setting?tab=directory');
  await page.clock.install();
  await page.getByRole('button', { name: '编辑 115 存储', exact: true }).click();
  await page.getByRole('button', { name: '扫码授权', exact: true }).click();
  const qrDialog = page.locator('.v-dialog.v-overlay--active').filter({ has: page.getByRole('button', { name: '关闭115 扫码登录', exact: true }) });
  await expect(qrDialog).toBeVisible();
  await expect(qrDialog.getByText('请使用 115 APP 扫描二维码登录', { exact: true })).toBeVisible();
  await page.clock.runFor(2000);
  await pending.started;
  await page.clock.runFor(6000);
  expect(qrReads(api)).toHaveLength(1);
  const finished = page.waitForResponse(response => new URL(response.url()).pathname === '/api/p115/qrcode/status');
  pending.release({ status: 'success' });
  await (await finished).finished();
  await expect(qrDialog.getByText('登录成功！Token 已自动保存', { exact: true })).toBeVisible();
  await page.clock.runFor(1600);
  await expect(qrDialog).toBeHidden();
  await page.clock.runFor(6000);
  expect(qrReads(api)).toHaveLength(1);
  expect(api.requests.filter(request => request.path === '/api/p115/qrcode' && request.method === 'POST')).toHaveLength(1);
  expect(api.requests.filter(request => request.path === '/api/config' && request.method === 'POST')).toHaveLength(0);
});
