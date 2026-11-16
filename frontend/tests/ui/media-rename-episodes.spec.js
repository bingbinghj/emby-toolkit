import { test, expect, mediaDetails, mediaInfo } from './fixtures.js';

const episode = (id, number = 1) => ({ emby_id: id, season_number: 1, episode_number: number, title: `分集 ${number}` });
const renameContext = name => ({ source_name: `${name}.mkv`, file_ext: '.mkv', fields: {}, versions: [] });
const selector = page => page.locator('.v-dialog.v-overlay--active').filter({ has: page.getByRole('button', { name: '关闭分集选择', exact: true }) });
const renameDialog = page => page.locator('.v-dialog.v-overlay--active').filter({ has: page.getByRole('button', { name: '关闭重命名', exact: true }) });

for (const type of ['Series', 'Season']) {
  test(`${type} rename opens episode selection and loads only the selected file`, async ({ page, api }) => {
    api.respond('GET', '/api/media_for_editing/parent', { ...mediaDetails, item_id: 'parent', item_type: type });
    api.respond('GET', '/api/media_info/series/parent/episodes', [episode('E199', 199), episode('E200', 200)]);
    api.respond('GET', '/api/media_rename/E200', renameContext('凡人修仙传.S01E200'));
    await page.goto('/edit-media/parent');
    await page.getByRole('button', { name: '重命名媒体', exact: true }).click();
    await expect(selector(page)).toContainText('选择要重命名的分集文件');
    await expect(selector(page).getByRole('button', { name: '全选', exact: true })).toHaveCount(0);
    await expect(selector(page).locator('.episode-chip-checkbox')).toHaveCount(0);
    await selector(page).locator('.episode-chip').filter({ hasText: '200' }).click();
    await expect(renameDialog(page).getByLabel('新文件名', { exact: true })).toHaveValue('凡人修仙传.S01E200');
    await expect(selector(page)).toHaveCount(0);
    expect(api.requests.filter(request => request.path.startsWith('/api/media_rename/')).map(request => request.path)).toEqual(['/api/media_rename/E200']);
    expect(api.requests.filter(request => request.method === 'POST' && request.path.startsWith('/api/media_'))).toHaveLength(0);
  });
}

test('the media-info selector still opens the selected episode editor', async ({ page, api }) => {
  api.respond('GET', '/api/media_for_editing/parent', { ...mediaDetails, item_id: 'parent', item_type: 'Series' });
  api.respond('GET', '/api/media_info/series/parent/episodes', [episode('E200', 200)]);
  api.respond('GET', '/api/media_info/edit/E200', mediaInfo);
  await page.goto('/edit-media/parent');
  await page.getByTestId('media-info-open').click();
  await expect(selector(page).getByRole('button', { name: '全选', exact: true })).toBeVisible();
  await selector(page).locator('.episode-chip').click();
  await expect(page.getByRole('button', { name: '关闭媒体信息编辑器', exact: true })).toBeVisible();
  await expect(renameDialog(page)).toHaveCount(0);
  expect(api.requests.some(request => request.path.startsWith('/api/media_rename/'))).toBe(false);
});

test('ffprobe naming supplementation still previews without renaming the movie file', async ({ page, api }) => {
  api.respond('GET', '/api/media_rename/101', renameContext('原文件名'));
  api.respond('POST', '/api/media_rename/101/ffprobe', { fields: { video_codec: 'HEVC' }, suggested_name: '预览文件名.mkv' });
  api.respond('POST', '/api/media_rename/101/preview', { target_name: '预览文件名.mkv', target_relative_path: '电影/预览文件名.mkv', preview_token: 'test-preview' });
  await page.goto('/edit-media/101');
  await page.getByRole('button', { name: '重命名媒体', exact: true }).click();
  await expect(renameDialog(page).getByLabel('新文件名', { exact: true })).toHaveValue('原文件名');
  await renameDialog(page).getByRole('button', { name: 'ffprobe 命名补充并预览', exact: true }).click();
  await expect(renameDialog(page)).toContainText('预览结果');
  await expect(renameDialog(page).getByLabel('新文件名', { exact: true })).toHaveValue('预览文件名');
  expect(api.requests.filter(request => request.method === 'POST' && request.path.startsWith('/api/media_rename/')).map(request => request.path)).toEqual(['/api/media_rename/101/ffprobe', '/api/media_rename/101/preview']);
});

test('movie rename uses its current page after another media-info editor was opened', async ({ page, api }) => {
  api.respond('GET', '/api/media_for_editing/other', { ...mediaDetails, item_id: 'other', item_type: 'Episode' });
  api.respond('GET', '/api/media_info/edit/other', mediaInfo);
  api.respond('GET', '/api/media_rename/101', renameContext('当前电影'));
  await page.goto('/edit-media/other');
  await page.getByTestId('media-info-open').click();
  await expect(page.getByRole('button', { name: '关闭媒体信息编辑器', exact: true })).toBeVisible();
  await page.evaluate(async () => {
    await document.getElementById('app').__vue_app__.config.globalProperties.$router.push('/edit-media/101');
  });
  await expect(page.locator('.editor-page')).toContainText('浏览器验证电影');
  await page.getByRole('button', { name: '重命名媒体', exact: true }).click();
  await expect(renameDialog(page).getByLabel('新文件名', { exact: true })).toHaveValue('当前电影');
  expect(api.requests.filter(request => request.path.startsWith('/api/media_rename/')).map(request => request.path)).toEqual(['/api/media_rename/101']);
});

test('a late rename context cannot replace the episode chosen on a new media page', async ({ page, api }) => {
  for (const id of ['A', 'B']) {
    api.respond('GET', `/api/media_for_editing/${id}`, { ...mediaDetails, item_id: id, item_name: `剧集 ${id}`, item_type: 'Series' });
    api.respond('GET', `/api/media_info/series/${id}/episodes`, [episode(`${id}01`)]);
  }
  const old = api.defer('GET', '/api/media_rename/A01');
  api.respond('GET', '/api/media_rename/B01', renameContext('新剧集.S01E01'));
  await page.goto('/edit-media/A');
  await page.getByRole('button', { name: '重命名媒体', exact: true }).click();
  await selector(page).locator('.episode-chip').click();
  await old.started;
  await page.evaluate(async () => {
    await document.getElementById('app').__vue_app__.config.globalProperties.$router.push('/edit-media/B');
  });
  await expect(page.locator('.editor-page')).toContainText('剧集 B');
  await page.getByRole('button', { name: '重命名媒体', exact: true }).click();
  await selector(page).locator('.episode-chip').click();
  await expect(renameDialog(page).getByLabel('新文件名', { exact: true })).toHaveValue('新剧集.S01E01');
  const finished = page.waitForResponse(response => new URL(response.url()).pathname === '/api/media_rename/A01');
  old.release(renameContext('旧剧集.S01E01'));
  await (await finished).finished();
  await expect(renameDialog(page).getByLabel('新文件名', { exact: true })).toHaveValue('新剧集.S01E01');
});
