import { test, expect, mediaInfo } from './fixtures.js';

const editor = page => page.locator('.v-dialog.v-overlay--active').filter({ has: page.getByTestId('media-info-save') });

test('initial open starts its media request before the language request completes', async ({ page, api }) => {
  const language = api.defer('GET', '/api/mappings/config/language_mapping');
  const media = api.defer('GET', '/api/media_info/edit/101');
  await page.goto('/edit-media/101');
  await page.getByTestId('media-info-open').click();
  await language.started;
  await expect.poll(() => api.requests.some(request => request.path === '/api/media_info/edit/101')).toBe(true);
  media.release(mediaInfo);
  language.release([{ label: '英语', value: 'eng', aliases: ['eng', 'en'] }]);
  await expect(editor(page)).toBeVisible();
});

test('reopening reuses track controls while discarding cancelled edits and closing language menus', async ({ page, api }) => {
  const stored = structuredClone(mediaInfo);
  stored.mediainfo[0].MediaSourceInfo.MediaStreams = Array.from({ length: 13 }, (_, index) => ({
    Type: index < 6 ? 'Audio' : 'Subtitle', Index: index + 1,
    Language: index < 6 ? 'eng' : 'chs', Title: `原轨道 ${index + 1}`, IsDefault: index === 0,
  }));
  api.respond('GET', '/api/media_info/edit/101', stored);
  await page.goto('/edit-media/101');
  await expect(page.getByTestId('media-info-save')).toHaveCount(0);
  await page.getByTestId('media-info-open').click();
  const title = editor(page).getByRole('textbox', { name: /音轨.*1.*当前标题/ });
  await title.evaluate(element => { window.__originalMediaTitleField = element; });
  await title.fill('取消的草稿');
  await editor(page).getByRole('textbox', { name: /音轨.*1.*修正语言/ }).click();
  await expect(page.getByRole('option', { name: '英语', exact: true })).toBeVisible();
  await editor(page).getByRole('button', { name: '关闭媒体信息编辑器' }).click();
  await expect(page.getByRole('option', { name: '英语', exact: true })).not.toBeVisible();
  await expect(page.getByTestId('media-info-save')).toHaveCount(1);
  await expect(page.getByTestId('media-info-save')).not.toBeVisible();
  await page.getByTestId('media-info-open').click();
  await expect(title).toHaveValue('原轨道 1');
  expect(await title.evaluate(element => element === window.__originalMediaTitleField)).toBe(true);
  const language = editor(page).getByRole('textbox', { name: /音轨.*1.*修正语言/ });
  await language.dblclick();
  await expect(title).toHaveValue('英语');
  await expect(language).not.toBeFocused();
  expect(api.requests.filter(request => request.method === 'POST')).toHaveLength(0);
});

test('cancelling a language selection cannot leave its search label on a reopened blank language track', async ({ page, api }) => {
  const stored = structuredClone(mediaInfo);
  delete stored.mediainfo[0].MediaSourceInfo.MediaStreams[1].Language;
  api.respond('GET', '/api/media_info/edit/101', stored);
  await page.goto('/edit-media/101');
  await page.getByTestId('media-info-open').click();
  const language = editor(page).getByRole('textbox', { name: /音轨.*1.*修正语言/ });
  await language.click();
  await page.getByRole('option', { name: '英语', exact: true }).click();
  await expect(language).toHaveValue('英语');
  await editor(page).getByRole('button', { name: '关闭媒体信息编辑器' }).click();
  await page.getByTestId('media-info-open').click();
  await expect(language).toHaveValue('');
  await expect(editor(page).locator('.v-autocomplete').filter({ has: language }).getByText('英语', { exact: true })).toHaveCount(0);
  await editor(page).getByTestId('media-info-save').click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/media_info/edit/101')).toBe(true);
  const payload = api.requests.find(request => request.method === 'POST' && request.path === '/api/media_info/edit/101').body;
  expect(payload.mediainfo).toEqual(stored.mediainfo);
});
