import { test, expect } from './fixtures.js';

const editor = page => page.locator('.v-dialog.v-overlay--active').filter({ has: page.getByRole('button', { name: '关闭媒体服务器配置', exact: true }) });
const card = page => page.getByRole('button', { name: '编辑 Emby 媒体服务器', exact: true });
const configurationWrites = api => api.requests.filter(request => request.method === 'POST' && request.path === '/api/config');

test('MP media server card supports keyboard editing and a scrolling full screen mobile dialog', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/settings/general?tab=system');
  await expect(card(page).getByLabel(/音乐数量/)).toHaveCount(0);
  await expect(card(page).getByLabel(/电影数量/)).toBeVisible();
  await expect(card(page).getByLabel(/电视剧数量/)).toBeVisible();
  await card(page).focus();
  await card(page).press('Enter');
  let dialog = editor(page);
  await expect(dialog).toBeVisible();
  const desktopContent = dialog.locator('.v-overlay__content');
  await expect.poll(async () => Math.round((await desktopContent.boundingBox())?.width || 0)).toBe(640);
  const desktop = await desktopContent.boundingBox();
  expect(Math.abs(desktop.width - 640)).toBeLessThanOrEqual(1);
  expect(desktop.x).toBeGreaterThanOrEqual(0);
  expect(desktop.x + desktop.width).toBeLessThanOrEqual(1440);
  await dialog.getByRole('button', { name: '取消媒体服务器配置', exact: true }).click();
  await expect(dialog).toBeHidden();

  await page.setViewportSize({ width: 390, height: 844 });
  await card(page).click();
  dialog = editor(page);
  await expect(dialog).toHaveClass(/v-dialog--fullscreen/);
  const mobileContent = dialog.locator('.v-overlay__content');
  await expect.poll(async () => {
    const box = await mobileContent.boundingBox();
    return box && Object.fromEntries(Object.entries(box).map(([key, value]) => [key, Math.round(value)]));
  }).toEqual({ x: 0, y: 0, width: 390, height: 844 });
  const mobile = await mobileContent.boundingBox();
  expect(Math.abs(mobile.width - 390)).toBeLessThanOrEqual(1);
  expect(Math.abs(mobile.height - 844)).toBeLessThanOrEqual(1);
  expect(Math.abs(mobile.x)).toBeLessThanOrEqual(1);
  expect(Math.abs(mobile.y)).toBeLessThanOrEqual(1);
  const content = dialog.locator('.v-card-text').first();
  const scroll = await content.evaluate(element => ({ height: element.clientHeight, content: element.scrollHeight }));
  expect(scroll.content).toBeGreaterThan(scroll.height);
  await content.evaluate(element => { element.scrollTop = element.scrollHeight; });
  await expect(dialog.locator('[data-field="emby_webhook_ingest_enabled"]')).toBeInViewport({ ratio: 1 });
  await dialog.getByRole('button', { name: '取消媒体服务器配置', exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(configurationWrites(api)).toEqual([]);
});

test('media server connection tests show real results without saving and clear them when the draft changes', async ({ page, api }) => {
  api.respond('POST', '/api/config/emby_check', {
    success: true, message: 'Emby 连接正常', data: { name: 'Fixture Emby', version: '4.10.2.0' },
  });
  await page.goto('/settings/general?tab=system');
  await card(page).click();
  const dialog = editor(page);
  const address = dialog.locator('[data-field="emby_server_url"] input');
  const originalAddress = await address.inputValue();
  await dialog.getByRole('button', { name: '测试媒体服务器连接', exact: true }).click();
  const result = page.getByTestId('media-server-connection-result');
  await expect(result).toContainText('Fixture Emby');
  await expect(result).toContainText('4.10.2.0');
  const check = api.requests.find(request => request.method === 'POST' && request.path === '/api/config/emby_check');
  expect(Object.keys(check.body).sort()).toEqual(['emby_api_key', 'emby_api_timeout', 'emby_server_url']);
  expect(check.body.emby_server_url).toBe(originalAddress);
  expect(configurationWrites(api)).toEqual([]);

  await address.fill('http://unreachable-emby.invalid:8096');
  await expect(result).toHaveCount(0);
  api.respond('POST', '/api/config/emby_check', { success: false, message: '测试网络连接失败' }, 400);
  await dialog.getByRole('button', { name: '测试媒体服务器连接', exact: true }).click();
  await expect(result).toContainText('测试网络连接失败');
  await dialog.getByRole('button', { name: '取消媒体服务器配置', exact: true }).click();
  await card(page).click();
  await expect(editor(page).locator('[data-field="emby_server_url"] input')).toHaveValue(originalAddress);
  await expect(result).toHaveCount(0);
  await editor(page).getByRole('button', { name: '取消媒体服务器配置', exact: true }).click();
  expect(configurationWrites(api)).toEqual([]);
});
