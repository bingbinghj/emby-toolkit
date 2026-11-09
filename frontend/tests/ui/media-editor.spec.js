import { test, expect, mediaDetails } from './fixtures.js';

test('VAutocomplete submits a primitive language code and retains save warnings in the editor', async ({ page, api }) => {
  api.respond('POST', '/api/media_info/edit/101', {
    message: '媒体信息 JSON 已保存，Emby 刷新尚未确认', warnings: 1,
    warning_details: ['101：无法访问上游服务'], metadata_refresh: { state: 'unverified', failed_ids: ['101'], failures: [] },
  });
  await page.goto('/edit-media/101');
  await page.getByTestId('media-info-open').click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByTestId('media-info-save') });
  const field = dialog.locator('.v-autocomplete').filter({ has: page.getByRole('textbox', { name: /音轨.*1.*修正语言/ }) });
  const language = field.getByRole('textbox', { name: /音轨.*1.*修正语言/ });
  await expect(field.getByText('英语', { exact: true })).toBeVisible();
  await language.click();
  await language.fill('日语');
  await page.getByRole('option', { name: '日语', exact: true }).click();
  await expect(field.getByText('日语', { exact: true })).toBeVisible();
  await dialog.getByTestId('media-info-save').click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/media_info/edit/101')).toBe(true);
  const payload = api.requests.find(request => request.method === 'POST' && request.path === '/api/media_info/edit/101').body;
  const track = payload.mediainfo[0].MediaSourceInfo.MediaStreams.find(stream => stream.Type === 'Audio');
  expect(track.Language).toBe('jpn');
  expect(typeof track.Language).toBe('string');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText('101：无法访问上游服务', { exact: true })).toBeVisible();
  await expect(dialog.getByText('Emby 刷新尚未通过验收（101）', { exact: true })).toBeVisible();
});

test('late actor translation preserves edits made while its request is running', async ({ page, api }) => {
  const translation = api.defer('POST', '/api/actions/translate_cast_sa');
  await page.goto('/edit-media/101');
  const actor = page.getByLabel('演员', { exact: true }).first();
  await expect(actor).toHaveValue('Actor One');
  await page.getByRole('button', { name: '一键翻译', exact: true }).click();
  await translation.started;
  await actor.fill('保留新演员草稿');
  translation.release(mediaDetails.current_emby_cast.map(person => ({ ...person, name: '迟到译文' })));
  await expect(page.getByText('翻译期间演员列表已修改，已保留当前草稿，请确认后重新翻译。', { exact: true })).toBeVisible();
  await expect(actor).toHaveValue('保留新演员草稿');
  await expect(page.getByRole('button', { name: '一键翻译', exact: true })).toBeEnabled();
});

test('confirmed media save closes the editor after receiving its result', async ({ page }) => {
  await page.goto('/edit-media/101');
  await page.getByTestId('media-info-open').click();
  await page.getByTestId('media-info-save').click();
  await expect(page.getByRole('button', { name: '关闭媒体信息编辑器', exact: true })).toHaveCount(0);
  await expect(page.getByText('媒体信息已更新', { exact: true })).toBeVisible();
});
