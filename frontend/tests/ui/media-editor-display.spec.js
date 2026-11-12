import { test, expect, mediaInfo } from './fixtures.js';

const recoveryPath = '/api/media_info/recovery';
const record = (id, targets) => ({ id, target_ids: targets, kind: 'media-reload', can_recover: true,
  directory: `/config/backups/media-info-reload/${id}`, reason: '前次媒体信息尚未验收' });
const editor = page => page.locator('.v-overlay--active').filter({ has: page.getByTestId('media-info-save') });

test('unrelated media recovery is hidden and switching to its version shows only its record', async ({ page, api }) => {
  const stored = structuredClone(mediaInfo);
  stored.versions.push({ emby_id: '102', file_name: 'Browser.Movie.Version2.mkv', path: '/media/Version2.mkv' });
  api.respond('GET', '/api/media_info/edit/101', stored);
  api.respond('GET', '/api/media_info/edit/102', stored);
  api.respond('GET', recoveryPath, { pending: true, records: [record('version-102', ['102']), record('other', ['61688'])] });
  await page.goto('/edit-media/101');
  await expect(page.getByTestId('media-info-open')).toBeVisible();
  await expect(page.getByTestId('media-recovery-alert')).toHaveCount(0);
  await page.getByTestId('media-info-open').click();
  await expect(editor(page).getByTestId('media-recovery-alert')).toHaveCount(0);
  await expect(editor(page).getByTestId('media-info-save')).toBeDisabled();
  await editor(page).getByRole('textbox', { name: '媒体版本', exact: true }).press('ArrowDown');
  await page.getByRole('option', { name: /Browser.Movie.Version2/ }).click();
  await expect(editor(page).getByTestId('media-recovery-alert')).toContainText('/version-102');
  await expect(editor(page).getByTestId('media-recovery-alert')).not.toContainText('/other');
  expect(api.requests.filter(request => request.method === 'POST')).toEqual([]);
});

for (const width of [390, 768, 1440]) {
  test(`PCM track label stays on one line without overflowing the page at ${width}px`, async ({ page, api }) => {
    const stored = structuredClone(mediaInfo);
    stored.mediainfo[0].MediaSourceInfo.MediaStreams = [{ Type: 'Audio', Index: 1, Codec: 'pcm_s16le', Channels: 2, Language: 'jpn', Title: '日语' }];
    api.respond('GET', '/api/media_info/edit/101', stored);
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/edit-media/101');
    await page.getByTestId('media-info-open').click();
    const label = editor(page).locator('.stream-technical-label');
    await expect(label).toHaveText('#1 · PCM_S16LE · 2ch');
    const position = await label.evaluate(element => {
      const range = document.createRange();
      range.selectNodeContents(element);
      const region = element.closest('.media-info-table-scroll');
      return { lines: range.getClientRects().length, wrap: getComputedStyle(element).whiteSpace,
        pageWidth: document.documentElement.scrollWidth, viewport: innerWidth,
        tableScrolls: [region, region.querySelector('.v-table__wrapper')].some(scroll => scroll && scroll.scrollWidth > scroll.clientWidth) };
    });
    expect(position.lines).toBe(1);
    expect(position.wrap).toBe('nowrap');
    expect(position.pageWidth).toBeLessThanOrEqual(position.viewport);
    if (width === 390) expect(position.tableScrolls).toBe(true);
  });
}
