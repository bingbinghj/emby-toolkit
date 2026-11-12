import { test, expect } from './fixtures.js';

const card = (page, title) => page.locator('.setting-card').filter({ has: page.getByText(title, { exact: true }) });
const writes = api => api.requests.filter(request => request.method === 'POST' && request.path === '/api/config');
const mediaServerDialog = page => page.locator('.v-dialog.v-overlay--active').filter({ has: page.getByRole('button', { name: '关闭媒体服务器配置', exact: true }) });

test('media server omits external playback URL and uses an icon plus filled primary save action', async ({ page, api }) => {
  await page.goto('/setting?tab=system');
  await expect(page.locator('[data-field="emby_public_url"]')).toHaveCount(0);
  await page.locator('[data-field="delay_between_items_sec"] input').fill('1.7');
  await page.getByRole('button', { name: '编辑 Emby 媒体服务器', exact: true }).click();
  let server = mediaServerDialog(page);
  const originalAddress = await server.locator('[data-field="emby_server_url"] input').inputValue();
  await server.locator('[data-field="emby_server_url"] input').fill('http://cancelled-emby.invalid:8096');
  await server.getByRole('button', { name: '取消媒体服务器配置', exact: true }).click();
  await expect(page.locator('[data-field="delay_between_items_sec"] input')).toHaveValue('1.7');
  await page.getByRole('button', { name: '编辑 Emby 媒体服务器', exact: true }).click();
  server = mediaServerDialog(page);
  await expect(server.locator('[data-field="emby_server_url"] input')).toHaveValue(originalAddress);
  const save = server.getByRole('button', { name: '保存媒体服务器', exact: true });
  await expect(save).toHaveClass(/v-btn--variant-flat/);
  await expect(save).toHaveText('保存');
  await expect(save.locator('.v-icon')).toBeVisible();
  await server.locator('[data-field="emby_server_url"] input').fill('http://updated-emby.invalid:8096');
  api.respond('POST', '/api/config', { error: '媒体服务器保存失败' }, 500);
  await save.click();
  await expect.poll(() => writes(api).length).toBe(1);
  await expect(server).toBeVisible();
  await expect(server.locator('[data-field="emby_server_url"] input')).toHaveValue('http://updated-emby.invalid:8096');
  await expect(page.getByText('媒体服务器保存失败', { exact: true }).first()).toBeVisible();
  api.respond('POST', '/api/config', { saved_fields: { emby_server_url: 'http://updated-emby.invalid:8096' } });
  await save.click();
  await expect.poll(() => writes(api).length).toBe(2);
  await expect(server).toBeHidden();
  await page.getByRole('button', { name: '编辑 Emby 媒体服务器', exact: true }).click();
  await expect(mediaServerDialog(page).locator('[data-field="emby_server_url"] input')).toHaveValue('http://updated-emby.invalid:8096');
  await mediaServerDialog(page).getByRole('button', { name: '取消媒体服务器配置', exact: true }).click();
  for (const request of writes(api)) {
    expect(request.body.emby_server_url).toBe('http://updated-emby.invalid:8096');
    expect(request.body).not.toHaveProperty('emby_public_url');
    expect(request.body).not.toHaveProperty('delay_between_items_sec');
    expect(request.body).not.toHaveProperty('emby_metadata_path');
  }
  await expect(page.locator('[data-field="delay_between_items_sec"] input')).toHaveValue('1.7');
});

test('independent image and SHA1 settings remain editable and save outside disabled filesystem monitor', async ({ page, api }) => {
  await page.goto('/setting?tab=rule');
  await expect(page.locator('[data-field="monitor_enabled"] input')).not.toBeChecked();
  const monitor = card(page, '实时监控');
  await expect(monitor.locator('[data-field="tmdb_image_language_preference"]')).toHaveCount(0);
  await expect(monitor.locator('[data-field="monitor_sha1_pc_search"]')).toHaveCount(0);
  await page.getByRole('radio', { name: '原语言优先', exact: true }).check();
  await page.getByRole('button', { name: '保存整理与刮削', exact: true }).click();
  await expect.poll(() => writes(api).length).toBe(1);
  expect(writes(api)[0].body.tmdb_image_language_preference).toBe('original');
  expect(writes(api)[0].body).not.toHaveProperty('monitor_enabled');
  await page.getByRole('tab', { name: '存储 & 目录', exact: true }).click();
  const sha = card(page, '目录').locator('[data-field="monitor_sha1_pc_search"] input');
  await expect(sha).toBeEnabled();
  await sha.uncheck();
  await page.getByRole('button', { name: '保存目录', exact: true }).click();
  await expect.poll(() => writes(api).length).toBe(2);
  expect(writes(api)[1].body.monitor_sha1_pc_search).toBe(false);
  expect(writes(api)[1].body).not.toHaveProperty('monitor_enabled');
});

test('ingest credits switch appears between intro and thumbnails and saves its actual draft', async ({ page, api }) => {
  await page.goto('/setting?tab=rule');
  const ingest = card(page, '入库自动处理');
  const fields = await ingest.locator('[data-field]').evaluateAll(nodes => nodes.map(node => node.dataset.field));
  expect(fields.indexOf('auto_extract_credits_on_ingest')).toBeGreaterThan(fields.indexOf('auto_extract_intro_fingerprint_on_ingest'));
  expect(fields.indexOf('auto_extract_credits_on_ingest')).toBeLessThan(fields.indexOf('auto_extract_video_thumbnails_on_ingest'));
  await ingest.getByRole('checkbox', { name: '入库完成后提取片尾', exact: true }).check();
  await ingest.getByRole('button', { name: '保存入库自动处理', exact: true }).click();
  await expect.poll(() => writes(api).length).toBe(1);
  expect(writes(api)[0].body.auto_extract_credits_on_ingest).toBe(true);
  await expect(ingest).toContainText('仅处理电视剧分集');
});
