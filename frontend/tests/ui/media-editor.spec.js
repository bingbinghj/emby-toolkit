import { test, expect, mediaDetails, mediaInfo } from './fixtures.js';

async function expectSaveAtRight(button) {
  await expect(button).toBeVisible();
  const position = await button.evaluate(element => {
    const footer = element.closest('.v-card-actions');
    return { right: element.getBoundingClientRect().right,
      expected: footer.getBoundingClientRect().right - parseFloat(getComputedStyle(footer).paddingRight) };
  });
  expect(Math.abs(position.right - position.expected)).toBeLessThanOrEqual(2);
}

test('subtitle editor shows actual languages and original titles without modifying an untouched save', async ({ page, api }) => {
  const stored = structuredClone(mediaInfo);
  stored.mediainfo[0].MediaSourceInfo.MediaStreams = [
    { Type: 'Subtitle', Index: 5, Codec: 'ass', Language: 'chi', Title: '粤语', DisplayLanguage: 'Chinese', DisplayTitle: 'Chinese (ASS)' },
    { Type: 'Subtitle', Index: 6, Codec: 'ass', Language: 'chi', Title: '日语简体' },
    { Type: 'Subtitle', Index: 7, Codec: 'ass', Language: 'yue', Title: '简体' },
  ];
  stored.iso_language_options = [{ label: '粤语', value: 'yue', aliases: ['yue'] }];
  api.respond('GET', '/api/media_info/edit/101', stored);
  await page.goto('/edit-media/101');
  await page.getByTestId('media-info-open').click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByTestId('media-info-save') });
  for (const [ordinal, label, title] of [[1, '中文', '粤语'], [2, '中文', '日语简体'], [3, '粤语', '简体']]) {
    const name = new RegExp(`字幕.*${ordinal}.*修正语言`);
    const field = dialog.locator('.v-autocomplete').filter({ has: page.getByRole('textbox', { name }) });
    await expect(field.getByText(label, { exact: true })).toBeVisible();
    await expect(dialog.getByRole('textbox', { name: new RegExp(`字幕.*${ordinal}.*当前标题`) })).toHaveValue(title);
  }
  await dialog.getByTestId('media-info-save').click();
  await expect.poll(() => api.requests.filter(request => request.method === 'POST' && request.path === '/api/media_info/edit/101').length).toBe(1);
  const payload = api.requests.find(request => request.method === 'POST' && request.path === '/api/media_info/edit/101').body;
  expect(payload.mediainfo).toEqual(stored.mediainfo);
});

test('choosing simplified from actual generic Chinese changes the code while retaining the original title', async ({ page, api }) => {
  const stored = structuredClone(mediaInfo);
  stored.mediainfo[0].MediaSourceInfo.MediaStreams[2] = { Type: 'Subtitle', Index: 5, Codec: 'ass', Language: 'chi', Title: '粤语' };
  api.respond('GET', '/api/media_info/edit/101', stored);
  await page.goto('/edit-media/101');
  await page.getByTestId('media-info-open').click();
  const dialog = page.locator('.v-overlay--active').filter({ has: page.getByTestId('media-info-save') });
  const language = dialog.getByRole('textbox', { name: /字幕.*1.*修正语言/ });
  await language.click();
  await language.fill('简体');
  await page.getByRole('option', { name: '简体', exact: true }).click();
  await expect(dialog.getByRole('textbox', { name: /字幕.*1.*当前标题/ })).toHaveValue('粤语');
  await dialog.getByTestId('media-info-save').click();
  await expect.poll(() => api.requests.some(request => request.method === 'POST' && request.path === '/api/media_info/edit/101')).toBe(true);
  const payload = api.requests.find(request => request.method === 'POST' && request.path === '/api/media_info/edit/101').body;
  const subtitle = payload.mediainfo[0].MediaSourceInfo.MediaStreams.find(stream => stream.Type === 'Subtitle');
  expect(subtitle.Language).toBe('chs');
  expect(subtitle.Title).toBe('粤语');
});

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

test('confirmed media save retains the editor after receiving its result', async ({ page }) => {
  await page.goto('/edit-media/101');
  await expectSaveAtRight(page.getByRole('button', { name: '保存', exact: true }));
  await page.getByTestId('media-info-open').click();
  await expectSaveAtRight(page.getByTestId('media-info-save'));
  await page.getByTestId('media-info-save').click();
  await expect(page.getByRole('button', { name: '关闭媒体信息编辑器', exact: true })).toBeVisible();
  await expect(page.getByText('媒体信息已更新', { exact: true })).toBeVisible();
});
