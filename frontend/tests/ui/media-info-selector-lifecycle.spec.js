import { test, expect, mediaDetails } from './fixtures.js';

test('a late series episode response cannot replace the selector for another opened series', async ({ page, api }) => {
  for (const [id, name] of [['A', '原剧集'], ['B', '新剧集']]) {
    api.respond('GET', `/api/media_for_editing/${id}`, { ...mediaDetails, item_id: id, item_name: name, item_type: 'Series' });
  }
  const oldPath = '/api/media_info/series/A/episodes';
  const old = api.defer('GET', oldPath);
  api.respond('GET', '/api/media_info/series/B/episodes', [{ emby_id: 'B01', season_number: 1, episode_number: 1, title: '新剧集分集' }]);
  await page.goto('/edit-media/A');
  await page.getByTestId('media-info-open').click();
  await old.started;
  await page.evaluate(async () => {
    await document.getElementById('app').__vue_app__.config.globalProperties.$router.push('/edit-media/B');
  });
  await expect(page.locator('.editor-page')).toContainText('新剧集');
  await page.getByTestId('media-info-open').click();
  const selector = page.locator('.v-dialog.v-overlay--active').filter({ has: page.getByRole('button', { name: '关闭分集选择', exact: true }) });
  await expect(selector.locator('.episode-chip')).toHaveAttribute('title', /新剧集分集/);
  const finished = page.waitForResponse(response => new URL(response.url()).pathname === oldPath);
  old.release([{ emby_id: 'A01', season_number: 1, episode_number: 1, title: '原剧集迟到分集' }]);
  await (await finished).finished();
  await expect(selector.locator('.episode-chip')).toHaveAttribute('title', /新剧集分集/);
  await expect(selector.locator('.episode-chip')).toHaveCount(1);
  await expect(selector.getByText('原剧集迟到分集')).toHaveCount(0);
  expect(api.requests.filter(request => request.method === 'POST' && request.path.startsWith('/api/media_info/'))).toHaveLength(0);
});
