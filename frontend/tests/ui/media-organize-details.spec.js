import { test, expect } from './fixtures.js';

const historyPath = '/api/media_organize/history';
const actionPath = '/api/actions/reorganize';
const failed = { id: 'history-200', item_id: 'episode-200', item_type: 'Episode', name: '第二百集', series_name: '整理详情验证', season_number: 1, episode_number: 200, status: 'failed', file_name: '葫芦兄弟.Calabash Brothers.S01E12.1986.1440p.SDR.H264.AAC.2.0.mp4', source_path: '/media/整理详情验证/Season 01/S01E200.strm', message: '媒体信息提取失败：上游返回空结果。\n请检查媒体来源。', created_at: '2026-11-17T01:00:00Z' };
const other = { id: 'history-other', item_id: 'movie-other', item_type: 'Movie', name: '另一条已选媒体', status: 'success', message: '媒体入库完成' };
const receipt = request => ({ mode: request.body.mode, queued_count: 1, duplicate_count: 0, queue_ids: ['queue-200'], items: [{ id: 'queue-200', item_id: 'episode-200', mode: request.body.mode, status: 'queued' }] });

for (const mobile of [false, true]) {
  test(`${mobile ? 'mobile' : 'desktop'} history hides reasons until its status opens details and queue`, async ({ page, api }, testInfo) => {
    await page.setViewportSize(mobile ? { width: 390, height: 844 } : { width: 1280, height: 720 });
    api.respond('GET', historyPath, { items: [failed, other], total: 2 });
    await page.goto('/media-organize');
    const list = page.getByTestId(mobile ? 'organize-items-mobile' : 'organize-items');
    await expect(list).toContainText('失败');
    await expect(list).not.toContainText(failed.message);
    const row = list.locator(mobile ? '.history-card' : '.organize-record').filter({ hasText: failed.series_name });
    await expect(row).not.toContainText('—');
    const file = list.getByText(failed.file_name, { exact: true });
    await expect(file).toBeVisible();
    expect(await file.evaluate(el => el.scrollWidth <= el.clientWidth + 1 && el.scrollHeight <= el.clientHeight + 1)).toBe(true);
    const status = list.getByRole('button', { name: '失败，查看整理详情验证详情', exact: true });
    await status.focus();
    await status.press('Enter');
    const dialog = page.getByTestId('organize-detail-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText('S01E200');
    await expect(dialog).toContainText(failed.source_path);
    await expect(dialog).toContainText(failed.message);
    await expect(dialog.getByRole('button', { name: '重新整理', exact: true })).toBeEnabled();
    await expect(dialog.getByRole('button', { name: '重新刮削', exact: true })).toBeEnabled();
    expect(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath('organize-details.png') });
    await dialog.getByRole('button', { name: '查看队列', exact: true }).click();
    await expect(dialog).not.toBeVisible();
    await expect(page.getByTestId('organize-queue-dialog')).toBeVisible();
    await page.getByRole('button', { name: '关闭整理队列', exact: true }).click();
    await list.getByRole('button', { name: '成功，查看另一条已选媒体详情', exact: true }).click();
    await expect(dialog).toContainText(other.message);
    expect(api.requests.filter(request => request.method !== 'GET')).toHaveLength(0);
  });
}

for (const [mode, title] of [['reprocess', '重新整理'], ['rescrape', '重新刮削']]) {
  test(`detail ${mode} uses only its episode while retaining unrelated selection`, async ({ page, api }) => {
    api.respond('GET', historyPath, { items: [failed, other], total: 2 });
    api.respond('POST', actionPath, receipt, 202);
    await page.goto('/media-organize');
    await page.getByRole('checkbox', { name: `选择 ${other.name}`, exact: true }).check();
    await page.getByRole('button', { name: '失败，查看整理详情验证详情', exact: true }).click();
    await page.getByTestId('organize-detail-dialog').getByRole('button', { name: title, exact: true }).click();
    await expect(page.getByTestId('organize-detail-dialog')).not.toBeVisible();
    await expect(page.locator('.organize-confirmation-card .v-card-title')).toHaveText(mode === 'reprocess' ? '手动整理' : title);
    await page.getByTestId('organize-confirm').click();
    await expect(page.getByTestId('organize-queue-dialog')).toBeVisible();
    expect(api.requests.filter(request => request.path === actionPath).map(request => request.body)).toEqual([{ item_ids: ['episode-200'], history_ids: ['history-200'], mode }]);
    await page.getByRole('button', { name: '关闭整理队列', exact: true }).click();
    await expect(page.getByRole('checkbox', { name: `选择 ${other.name}`, exact: true })).toBeChecked();
  });
}

test('detail follows refreshed status and retains warnings without allowing duplicate active work', async ({ page, api }) => {
  let record = { ...failed, status: 'processing', message: '正在处理当前分集' };
  api.respond('GET', historyPath, () => ({ items: [record], total: 1 }));
  await page.goto('/media-organize');
  await page.getByRole('button', { name: '正在处理，查看整理详情验证详情', exact: true }).click();
  const dialog = page.getByTestId('organize-detail-dialog');
  await expect(dialog.getByRole('button', { name: '重新整理', exact: true })).toBeDisabled();
  await expect(dialog.getByRole('button', { name: '重新刮削', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: '关闭整理详情', exact: true }).click();
  record = { ...failed, status: 'warning', message: '入库完成，图片待确认' };
  await page.getByRole('button', { name: '刷新入库记录', exact: true }).click();
  await expect(page.getByTestId('organize-items')).not.toContainText(record.message);
  await page.getByRole('button', { name: '成功，查看整理详情验证详情', exact: true }).click();
  await expect(dialog).toContainText(record.message);
  await expect(dialog.locator('.v-alert')).toHaveClass(/v-alert--variant-tonal/);
  await expect(dialog.getByRole('button', { name: '重新整理', exact: true })).toBeEnabled();
});
