import { readFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';
const tasks = JSON.parse(readFileSync(new URL('../../../backend/assets/task_catalog.json', import.meta.url))).manual;
for (const full of [false, true]) {
  test(`credits full=${full} submits an explicit mode`, async ({ page, api }) => {
    api.respond('GET', '/api/tasks/available', tasks);
    api.respond('POST', '/api/tasks/run', { task_id: 'credits-mode', message: '片尾任务已提交' }, 202);
    api.respond('GET', '/api/tasks/result/credits-mode', { task_id: 'credits-mode', is_running: false, state: 'completed', result_status: 200, message: '片尾提取完成', result: { updated: 0, failed: 0 } });
    await page.goto('/settings/scheduler');
    await page.locator('[data-task-key="extract-episode-credits"]').getByRole('button', { name: '执行', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('增量提取跳过已记录的失败分集');
    expect(api.requests.filter(r => r.path === '/api/tasks/run')).toHaveLength(0);
    await dialog.getByRole('button', { name: full ? '全量提取' : '增量提取', exact: true }).click();
    await expect.poll(() => api.requests.filter(r => r.path === '/api/tasks/run').map(r => r.body)).toEqual([{ task_name: 'extract-episode-credits', force_full_update: full }]);
  });
}
test('failure records use the existing table cleanup dialog', async ({ page, api }) => {
  api.respond('GET', '/api/database/tables', ['media_metadata', 'credits_failures']);
  api.respond('POST', '/api/actions/clear_tables', { cleared_tables: ['credits_failures'], message: '已清空片尾提取失败记录' });
  await page.goto('/setting?tab=system');
  await page.getByRole('button', { name: '清空指定表', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('checkbox', { name: '片尾提取失败记录', exact: true }).check();
  await expect(dialog.getByRole('checkbox', { name: '媒体元数据', exact: true })).not.toBeChecked();
  await dialog.getByRole('button', { name: '确认清空', exact: true }).click();
  await expect.poll(() => api.requests.filter(r => r.path === '/api/actions/clear_tables').map(r => r.body)).toEqual([{ tables: ['credits_failures'] }]);
});
const preferences = [
  { theme: 'light', glassEnabled: false, primaryColor: '#8D51F9' },
  { theme: 'dark', glassEnabled: false, primaryColor: '#8D51F9' },
  { theme: 'light', glassEnabled: true, primaryColor: '#FFFFFF', glassAppearance: 'clear', glassQuality: 'css' },
  { theme: 'dark', glassEnabled: true, primaryColor: '#FFB400', glassAppearance: 'frosted', glassQuality: 'css' },
];
async function checkPagination(page, root, name, results) {
  const active = root.locator('.v-pagination__item--is-active .v-btn');
  await expect(active).toBeVisible();
  await active.scrollIntoViewIfNeeded();
  await page.mouse.move(0, 0);
  await expect.poll(() => active.evaluate(el => getComputedStyle(el).backgroundColor.replaceAll(' ', '') === `rgb(${getComputedStyle(el).getPropertyValue('--v-theme-primary').trim()})`.replaceAll(' ', ''))).toBe(true);
  const readings = await measureReadingContrast(page, [
    { role: `${name} selected`, locator: active.locator('.v-btn__content') },
    { role: `${name} unselected`, locator: root.locator('.v-pagination__item:not(.v-pagination__item--is-active) .v-btn:not(:disabled) .v-btn__content').first() },
  ]);
  for (const reading of readings) expect(reading.contrast, JSON.stringify(reading)).toBeGreaterThanOrEqual(4.5);
  results.push(...readings);
}
for (const preference of preferences) {
  test(`${preference.theme} glass=${preference.glassEnabled} ${preference.primaryColor} readable pagination across pages`, async ({ page, api }, testInfo) => {
    test.setTimeout(240000);
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.addInitScript(p => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify(p)), preference);
    api.respond('GET', '/api/logs/list', ['app.log']);
    api.respond('GET', '/api/logs/view', { file: 'app.log', lines: ['INFO: 2026-11-17 15:00:00,000 logs - 分页可读性'], page: 1, pages: 4, total_lines: 1501 });
    const results = [];
    await page.goto('/dashboard');
    await page.getByRole('button', { name: '实时日志', exact: true }).click();
    await page.getByRole('tab', { name: '历史日志', exact: true }).click();
    await checkPagination(page, page.locator('.history-pagination'), 'history', results);
    const tabs = await measureReadingContrast(page, [{ role: 'history tab', locator: page.getByRole('tab', { name: '历史日志', exact: true }).locator('.v-btn__content') }]);
    for (const reading of tabs) expect(reading.contrast, JSON.stringify(reading)).toBeGreaterThanOrEqual(4.5);
    results.push(...tabs);
    await page.getByRole('button', { name: '关闭实时日志', exact: true }).click();
    api.respond('GET', '/api/media_organize/history', { items: [{ id: 'r1', item_id: '101', name: '分页测试', item_type: 'Episode', status: 'success' }], total: 151, page: 1, page_size: 50 });
    await page.goto('/media-organize');
    await checkPagination(page, page.getByTestId('organize-pagination'), 'organize', results);
    api.respond('GET', '/api/media_organize/queue', { items: [{ id: 'q1', item_id: '101', mode: 'reprocess', status: 'queued', item_json: { name: '测试媒体' } }], total: 201, page: 1, page_size: 100, queued_count: 201, running_count: 0 });
    await page.getByTestId('organize-queue-open').click();
    await expect(page.getByTestId('organize-queue-dialog')).toBeVisible();
    await page.getByRole('button', { name: '刷新整理队列', exact: true }).click();
    await checkPagination(page, page.getByTestId('organize-queue-dialog'), 'queue', results);
    const queueText = await measureReadingContrast(page, [
      { role: 'queue progress', locator: page.locator('.queue-overall-value') },
      { role: 'queue header', locator: page.locator('.queue-task-header > *').nth(1) },
    ]);
    for (const reading of queueText) expect(reading.contrast, JSON.stringify(reading)).toBeGreaterThanOrEqual(4.5);
    results.push(...queueText);
    await page.getByRole('button', { name: '关闭整理队列', exact: true }).click();
    api.respond('GET', '/api/review_items', { items: [{ item_id: '101', item_name: '复核分页', item_type: 'Movie', reason: '人工复核' }], total_items: 150 });
    await page.setViewportSize({ width: 390, height: 900 });
    await page.goto('/review');
    await checkPagination(page, page.locator('.review-page'), 'review', results);
    await testInfo.attach('contrast-results', { body: JSON.stringify(results, null, 2), contentType: 'application/json' });
  });
}
