import { test, expect, mediaDetails } from './fixtures.js';

const recoveryPath = '/api/media_info/recovery';
const episodeRecord = { id: 'episode-8', target_ids: ['61838'], related_item_ids: ['61789', '61788'],
  kind: 'media-reload', can_recover: true, reason: '第8集需要恢复', directory: '/config/backups/media-info-reload/episode-8' };
const movieRecord = { id: 'movie-201', target_ids: ['201'], related_item_ids: [],
  kind: 'media-reload', can_recover: true, reason: '电影201需要恢复', directory: '/config/backups/media-info-reload/movie-201' };
const state = { pending: true, records: [episodeRecord, movieRecord] };

for (const [type, id, expectedRecord] of [
  ['Series', '61788', episodeRecord], ['Season', '61789', episodeRecord], ['Episode', '61838', episodeRecord],
  ['Series', '61790', null], ['Season', '61791', null], ['Episode', '61839', null],
  ['Movie', '201', movieRecord], ['Movie', '202', null],
]) {
  test(`${type} ${id} shows recovery only for its verified media scope`, async ({ page, api }) => {
    const name = `范围测试 ${type} ${id}`;
    api.respond('GET', `/api/media_for_editing/${id}`, { ...mediaDetails, item_id: id, item_name: name, item_type: type });
    api.respond('GET', recoveryPath, state);
    await page.goto(`/edit-media/${id}`);
    await expect(page.locator('.editor-page')).toContainText(name);
    const alert = page.getByTestId('media-recovery-alert');
    if (expectedRecord) {
      await expect(alert).toContainText(expectedRecord.directory);
      await expect(alert).not.toContainText(expectedRecord === episodeRecord ? movieRecord.directory : episodeRecord.directory);
    } else {
      await expect(alert).toHaveCount(0);
    }
    expect(api.requests.filter(request => request.method === 'POST' && request.path === recoveryPath)).toHaveLength(0);
  });
}

test('pending media recovery has no global warning on the dashboard', async ({ page, api }) => {
  api.respond('GET', recoveryPath, state);
  await page.goto('/dashboard');
  await expect(page.locator('.dashboard-page')).toBeVisible();
  await expect(page.getByTestId('media-recovery-alert')).toHaveCount(0);
  await expect(page.getByText('媒体信息需要恢复', { exact: true })).toHaveCount(0);
  expect(api.requests.filter(request => request.path === recoveryPath)).toHaveLength(0);
});

test('changing the edited item clears an old recovery error while its new read is pending', async ({ page, api }) => {
  api.respond('GET', '/api/media_for_editing/61838', { ...mediaDetails, item_id: '61838', item_name: '第8集', item_type: 'Episode' });
  api.respond('GET', '/api/media_for_editing/61839', { ...mediaDetails, item_id: '61839', item_name: '第9集', item_type: 'Episode' });
  api.respond('GET', recoveryPath, { error: '第8集旧恢复状态错误' }, 500);
  await page.goto('/edit-media/61838');
  await expect(page.getByTestId('media-recovery-alert')).toContainText('第8集旧恢复状态错误');
  const next = api.defer('GET', recoveryPath);
  await page.evaluate(async () => {
    await document.getElementById('app').__vue_app__.config.globalProperties.$router.push('/edit-media/61839');
  });
  await next.started;
  await expect(page.getByText('第8集旧恢复状态错误', { exact: true })).toHaveCount(0);
  next.release(state);
  await expect(page.locator('.editor-page')).toContainText('第9集');
  await expect(page.getByTestId('media-recovery-alert')).toHaveCount(0);
  expect(api.requests.filter(request => request.method === 'POST' && request.path === recoveryPath)).toHaveLength(0);
});
