import { test, expect, mediaInfo } from './fixtures.js';

for (const warnings of [false, true]) {
  test(`a second default-only save retains verified traditional Chinese${warnings ? ' while another target has a warning' : ''}`, async ({ page, api }) => {
    const path = '/api/media_info/edit/101';
    const stored = structuredClone(mediaInfo);
    Object.assign(stored.mediainfo[0].MediaSourceInfo.MediaStreams[2], { Language: 'chi', Title: '旧字幕', IsDefault: false });
    api.respond('GET', path, stored);
    api.respond('POST', path, request => {
      const canonical = structuredClone(request.body.mediainfo);
      canonical[0].MediaSourceInfo.MediaStreams.find(stream => stream.Type === 'Subtitle').Language = 'cht';
      return { message: '当前媒体已通过回读', media_info_saved: true, saved_item_ids: ['101'],
        saved_mediainfo: canonical, metadata_refresh: { state: 'verified', verified_ids: ['101'], failed_ids: [] },
        warnings: warnings ? 1 : 0, failures: [], batch_reports: warnings
          ? [{ item_id: '102', skipped: [{ type: 'Audio', ordinal: 1, reason: '其它目标的音轨范围不唯一' }] }] : [] };
    });
    await page.goto('/edit-media/101');
    await page.getByTestId('media-info-open').click();
    const editor = page.locator('.v-dialog.v-overlay--active').filter({ has: page.getByTestId('media-info-save') });
    const title = editor.getByRole('textbox', { name: /字幕.*1.*当前标题/ });
    await title.fill('繁体字幕');
    await editor.getByTestId('media-info-save').click();
    const language = editor.locator('.v-autocomplete').filter({ has: page.getByRole('textbox', { name: /字幕.*1.*修正语言/ }) });
    await expect(language.getByText('繁体', { exact: true })).toBeVisible();
    await expect(editor).toBeVisible();
    await expect(title).toHaveValue('繁体字幕');
    if (warnings) await expect(editor.getByText(/102.*其它目标的音轨范围不唯一/)).toBeVisible();
    await editor.getByRole('checkbox', { name: /字幕.*1.*默认轨道/ }).check();
    await editor.getByTestId('media-info-save').click();
    await expect.poll(() => api.requests.filter(request => request.method === 'POST' && request.path === path).length).toBe(2);
    const requests = api.requests.filter(request => request.method === 'POST' && request.path === path);
    const subtitle = requests[1].body.mediainfo[0].MediaSourceInfo.MediaStreams.find(stream => stream.Type === 'Subtitle');
    expect(subtitle).toMatchObject({ Language: 'cht', Title: '繁体字幕', IsDefault: true });
    await expect(editor).toBeVisible();
  });
}
