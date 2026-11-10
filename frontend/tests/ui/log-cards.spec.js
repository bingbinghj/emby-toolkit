import { test, expect } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';

const longBody = '媒体 JSON 在失败后被修改，保留当前文件及备份。恢复目录 /config/backups/media-info-reload/' + 'long-path-'.repeat(12) + '。完整结尾可见';
const liveLine = `[12:30:45] [CRITICAL] ${longBody}`;
const historyLine = `2026-11-10 12:30:45,123 - emby_toolkit::media_reload - CRITICAL - ${longBody}`;
const pythonSource = 'app.modules.emby.media_info:restore_saved_json:354';
const warningLine = `2026-11-10 12:30:44,000 - ${pythonSource} - WARNING - 提醒完整记录`;
const task = { is_running: false, state: 'failed', last_action: '日志卡片验收', message: '读取完整失败原因', logs: [liveLine, '[12:30:44] [WARNING] 提醒完整记录'] };

async function openLogs(page, api) {
  api.setTask(task);
  await page.goto('/dashboard');
  await page.getByRole('button', { name: '通知中心', exact: true }).click();
  await expect(page.locator('.notification-log .log-record-body').first()).toHaveText(longBody);
}

async function assertFullText(page, locator) {
  await locator.scrollIntoViewIfNeeded();
  const state = await locator.evaluate(element => {
    const panel = element.closest('.log-record-panel');
    const box = element.getBoundingClientRect(), shell = panel.getBoundingClientRect();
    return { clippedX: element.scrollWidth > element.clientWidth + 1, clippedY: element.scrollHeight > element.clientHeight + 1,
      withinPanel: box.left >= shell.left + 8 && box.right <= shell.right - 8 && box.bottom <= shell.bottom - 8,
      wrap: getComputedStyle(element).whiteSpace };
  });
  expect(state).toEqual({ clippedX: false, clippedY: false, withinPanel: true, wrap: 'pre-wrap' });
}

for (const [name, preference] of [
  ['light', { theme: 'light', glassEnabled: false }],
  ['dark', { theme: 'dark', glassEnabled: false }],
  ['auto-dark', { theme: 'auto', glassEnabled: false }],
  ['dark-clear', { theme: 'dark', glassEnabled: true, glassAppearance: 'clear', glassQuality: 'css' }],
  ['dark-tinted', { theme: 'dark', glassEnabled: true, glassAppearance: 'tinted', glassQuality: 'css' }],
  ['dark-frosted', { theme: 'dark', glassEnabled: true, glassAppearance: 'frosted', glassQuality: 'css' }],
]) {
  for (const width of [390, 1440]) {
    test(`${name} ${width} live and history log cards show complete readable text`, async ({ page, api }) => {
      test.setTimeout(90_000);
      await page.emulateMedia({ colorScheme: 'dark' });
      await page.setViewportSize({ width, height: 844 });
      await page.addInitScript(preference => {
        localStorage.setItem('moviepilot-theme-customizer', JSON.stringify(preference));
        const canvas = document.createElement('canvas'); canvas.width = canvas.height = 32;
        const context = canvas.getContext('2d'); context.fillStyle = '#fff'; context.fillRect(0, 0, 32, 32);
        localStorage.setItem('etk-wallpaper-settings', JSON.stringify({ mode: 'custom', customImage: canvas.toDataURL(), rotationInterval: 0 }));
      }, preference);
      api.respond('GET', '/api/logs/list', ['app.log']);
      api.respond('GET', '/api/logs/view', { file: 'app.log', lines: [historyLine, warningLine], page: 1, pages: 1, total_lines: 2 });
      await openLogs(page, api);
      const live = page.locator('.notification-log .log-record-body').first();
      await assertFullText(page, live);
      const [liveContrast] = await measureReadingContrast(page, [{ role: `${name}/${width} live body`, locator: live }]);
      expect(liveContrast.contrast).toBeGreaterThanOrEqual(4.5);
      await page.getByRole('tab', { name: '历史日志', exact: true }).click();
      const history = page.locator('.history-log-list .log-record-body').first();
      await expect(history).toHaveText(longBody);
      await assertFullText(page, history);
      const modules = page.locator('.history-log-list .log-record-module');
      await expect(modules).toHaveText(['emby_toolkit::media_reload', pythonSource]);
      for (const source of await modules.all()) {
        await source.scrollIntoViewIfNeeded();
        await expect(source).toBeVisible();
        expect(await source.evaluate(element => element.scrollWidth <= element.clientWidth + 1 && element.scrollHeight <= element.clientHeight + 1)).toBe(true);
      }
      await history.scrollIntoViewIfNeeded();
      const [historyContrast] = await measureReadingContrast(page, [{ role: `${name}/${width} history body`, locator: history }]);
      expect(historyContrast.contrast).toBeGreaterThanOrEqual(4.5);
      expect(api.requests.find(request => request.path === '/api/logs/view').query.format).toBe('json');
      await expect(page.locator('.notification-center iframe')).toHaveCount(0);
      console.log(JSON.stringify({ name, width, liveContrast, historyContrast }));
    });
  }
}

test('history cards paginate, filter errors and search complete contexts without losing raw exports', async ({ page, api }) => {
  api.respond('GET', '/api/logs/list', ['app.log', 'app.log.1']);
  api.respond('GET', '/api/logs/view', request => ({ file: request.query.filename, lines: request.query.page === '2' ? ['2026-11-09 12:00:00,000 - module - INFO - 较早日志完整结尾'] : [historyLine, warningLine], page: Number(request.query.page) || 1, pages: 2, total_lines: 501 }));
  api.respond('GET', '/api/logs/search', [{ file: 'app.log.1', content: historyLine }]);
  api.respond('GET', '/api/logs/search_context', [{ file: 'app.log.1', lines: ['2026-11-10 12:30:43,000 - module - INFO - [任务开始] 恢复', historyLine, '2026-11-10 12:30:46,000 - module - ERROR - [任务结束] 失败完整结尾'] }]);
  await openLogs(page, api);
  await page.getByRole('tab', { name: '历史日志', exact: true }).click();
  await expect(page.locator('.history-log-list .log-record')).toHaveCount(2);
  await page.getByRole('button', { name: '较早日志', exact: true }).click();
  await expect(page.getByText('较早日志完整结尾', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: '较新日志', exact: true }).click();
  await page.locator('.history-log-view .v-select').filter({ has: page.getByLabel('日志级别', { exact: true }) }).locator('.v-field').click();
  await page.getByRole('option', { name: '错误', exact: true }).click();
  await expect(page.locator('.history-log-list .log-record')).toHaveCount(1);
  await page.locator('.history-log-view').getByLabel('在所有日志文件中搜索', { exact: true }).fill('JSON');
  await page.locator('.history-log-view').getByRole('button', { name: '搜索', exact: true }).click();
  await expect.poll(() => api.requests.some(request => request.path === '/api/logs/search_context')).toBe(true);
  await expect(page.locator('.history-log-list .log-record')).toHaveCount(2);
  await expect(page.locator('.history-log-list mark').first()).toHaveText('JSON');
  await expect(page.getByText('[任务结束] 失败完整结尾', { exact: true })).toBeVisible();
  await page.getByLabel('匹配日志行', { exact: true }).check();
  await expect(page.locator('.history-log-list .log-record')).toHaveCount(1);
  const download = page.waitForEvent('download');
  await page.locator('.history-log-view').getByRole('button', { name: '导出日志', exact: true }).click();
  const saved = await download;
  const stream = await saved.createReadStream();
  let text = ''; for await (const chunk of stream) text += chunk.toString();
  expect(text).toBe(historyLine);
});

test('paused live following freezes the visible records until returning to newest', async ({ page, api }) => {
  await openLogs(page, api);
  await page.getByLabel('自动跟随日志', { exact: true }).uncheck();
  api.setTask({ ...task, logs: [...task.logs, '[12:31:00] [INFO] 后续日志完整结尾'] });
  await expect(page.getByRole('button', { name: '有新日志 · 回到最新', exact: true })).toBeVisible();
  await expect(page.getByText('后续日志完整结尾', { exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: '有新日志 · 回到最新', exact: true }).click();
  await expect(page.getByText('后续日志完整结尾', { exact: true })).toBeVisible();
});

test('failed history reads expose retry errors and confirmed empty pages replace old cards', async ({ page, api }) => {
  api.respond('GET', '/api/logs/list', ['app.log']);
  api.respond('GET', '/api/logs/view', { file: 'app.log', lines: [historyLine], page: 1, pages: 1, total_lines: 1 });
  await openLogs(page, api);
  await page.getByRole('tab', { name: '历史日志', exact: true }).click();
  await expect(page.locator('.history-log-list .log-record')).toHaveCount(1);
  api.respond('GET', '/api/logs/view', { error: '日志暂不可读取' }, 503);
  await page.getByRole('button', { name: '刷新历史日志', exact: true }).click();
  await expect(page.getByText('日志暂不可读取', { exact: true })).toBeVisible();
  api.respond('GET', '/api/logs/view', { file: 'app.log', lines: [], page: 1, pages: 1, total_lines: 0 });
  await page.getByRole('button', { name: '刷新历史日志', exact: true }).click();
  await expect(page.getByText('暂无日志内容', { exact: true })).toBeVisible();
  await expect(page.locator('.history-log-list .log-record')).toHaveCount(0);
});

test('switching the first pending context search to matching rejects the late context and preserves loading', async ({ page, api }) => {
  api.respond('GET', '/api/logs/list', ['app.log']);
  api.respond('GET', '/api/logs/view', { file: 'app.log', lines: [historyLine], page: 1, pages: 1, total_lines: 1 });
  const context = api.defer('GET', '/api/logs/search_context');
  const matching = api.defer('GET', '/api/logs/search');
  await openLogs(page, api);
  await page.getByRole('tab', { name: '历史日志', exact: true }).click();
  await expect(page.locator('.history-log-list .log-record')).toHaveCount(1);
  await page.locator('.history-log-view').getByLabel('在所有日志文件中搜索', { exact: true }).fill('JSON');
  await page.locator('.history-log-view').getByRole('button', { name: '搜索', exact: true }).click();
  await context.started;
  const filter = page.getByLabel('匹配日志行', { exact: true });
  await expect(filter).toBeEnabled();
  await filter.check();
  await matching.started;
  const staleResponse = page.waitForResponse(response => new URL(response.url()).pathname === '/api/logs/search_context');
  context.release([{ file: 'old-context.log', lines: ['2026-11-10 12:30:45,123 - stale::context - ERROR - 旧上下文 JSON 不应出现'] }]);
  await (await staleResponse).finished();
  await expect(page.locator('.history-log-view').getByRole('button', { name: '搜索', exact: true })).toBeDisabled();
  await expect(page.getByText('旧上下文 JSON 不应出现', { exact: true })).toHaveCount(0);
  matching.release([{ file: 'new-match.log', content: '2026-11-10 12:31:00,123 - latest::match:restore:354 - WARNING - 新匹配 JSON 完整结尾' }]);
  await expect(page.getByText('新匹配 JSON 完整结尾', { exact: true })).toBeVisible();
  await expect(page.getByText('latest::match:restore:354', { exact: true })).toBeVisible();
  await expect(page.locator('.history-log-view').getByRole('button', { name: '搜索', exact: true })).toBeEnabled();
  await expect(filter).toBeChecked();
  await expect(page.getByText('旧上下文 JSON 不应出现', { exact: true })).toHaveCount(0);
});

test('changing modes while a replacement search is pending accepts only the newest context and ignores stale errors', async ({ page, api }) => {
  api.respond('GET', '/api/logs/list', ['app.log']);
  api.respond('GET', '/api/logs/view', { file: 'app.log', lines: [historyLine], page: 1, pages: 1, total_lines: 1 });
  api.respond('GET', '/api/logs/search_context', [{ file: 'original.log', lines: ['2026-11-10 12:30:45,123 - original::context - INFO - 原上下文 JSON'] }]);
  await openLogs(page, api);
  await page.getByRole('tab', { name: '历史日志', exact: true }).click();
  await expect(page.locator('.history-log-list .log-record')).toHaveCount(1);
  await page.locator('.history-log-view').getByLabel('在所有日志文件中搜索', { exact: true }).fill('JSON');
  await page.locator('.history-log-view').getByRole('button', { name: '搜索', exact: true }).click();
  await expect(page.getByText('原上下文 JSON', { exact: true })).toBeVisible();
  const matching = api.defer('GET', '/api/logs/search');
  await page.getByLabel('匹配日志行', { exact: true }).check();
  await matching.started;
  const latest = api.defer('GET', '/api/logs/search_context');
  await page.getByLabel('完整上下文', { exact: true }).check();
  await latest.started;
  const staleResponse = page.waitForResponse(response => new URL(response.url()).pathname === '/api/logs/search');
  matching.release({ error: '旧匹配错误不应显示' });
  await (await staleResponse).finished();
  await expect(page.locator('.history-log-view').getByRole('button', { name: '搜索', exact: true })).toBeDisabled();
  await expect(page.getByText('日志搜索结果格式无效', { exact: true })).toHaveCount(0);
  await expect(page.getByText('旧匹配错误不应显示', { exact: true })).toHaveCount(0);
  latest.release([{ file: 'latest.log', lines: ['2026-11-10 12:31:00,123 - latest::context:restore:354 - ERROR - 最新上下文 JSON 完整结尾'] }]);
  await expect(page.getByText('最新上下文 JSON 完整结尾', { exact: true })).toBeVisible();
  await expect(page.getByText('latest::context:restore:354', { exact: true })).toBeVisible();
  await expect(page.locator('.history-log-view').getByRole('button', { name: '搜索', exact: true })).toBeEnabled();
  await expect(page.getByLabel('完整上下文', { exact: true })).toBeChecked();
  await expect(page.getByText('日志搜索结果格式无效', { exact: true })).toHaveCount(0);
});
