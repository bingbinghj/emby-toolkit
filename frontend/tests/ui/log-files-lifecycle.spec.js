import { test, expect } from './fixtures.js';

const logLine = body => `2026-11-12 12:00:00,000 - fixture::logs - INFO - ${body}`;
const pageData = body => ({ file: 'app.log', lines: [logLine(body)], page: 1, pages: 1, total_lines: 1 });
const viewReads = api => api.requests.filter(request => request.method === 'GET' && request.path === '/api/logs/view');
async function openHistory(page, api, lines = [logLine('已读取的原文件记录')]) {
  api.respond('GET', '/api/logs/list', ['app.log']);
  api.respond('GET', '/api/logs/view', { file: 'app.log', lines, page: 1, pages: 1, total_lines: lines.length });
  await page.goto('/dashboard');
  await page.getByRole('button', { name: '实时日志', exact: true }).click();
  await page.getByRole('tab', { name: '历史日志', exact: true }).click();
  await expect(page.locator('.history-log-list .log-record-message').first()).toHaveText('已读取的原文件记录');
}

test('a failed file-list refresh remains visible without reading old content, then retries normally', async ({ page, api }) => {
  await openHistory(page, api);
  const initialReads = viewReads(api).length;
  api.respond('GET', '/api/logs/list', { error: '日志文件列表暂不可用' }, 503);
  api.respond('GET', '/api/logs/view', pageData('列表失败时不应读取此内容'));
  await page.getByRole('button', { name: '刷新历史日志', exact: true }).click();
  await expect(page.getByText('日志文件列表暂不可用', { exact: true })).toBeVisible();
  await expect(page.locator('.history-log-list .log-record-message')).toHaveText('已读取的原文件记录');
  expect(viewReads(api)).toHaveLength(initialReads);
  await expect(page.getByText('列表失败时不应读取此内容', { exact: true })).toHaveCount(0);

  api.respond('GET', '/api/logs/list', ['app.log', 'app.log.1']);
  api.respond('GET', '/api/logs/view', { file: 'app.log',
    lines: Array.from({ length: 500 }, (_, index) => logLine(index ? `重试后记录 ${index + 1}` : '重试后的日志记录')),
    page: 1, pages: 2, total_lines: 501 });
  await page.getByRole('button', { name: '刷新历史日志', exact: true }).click();
  await expect(page.locator('.history-log-list .log-record-message').first()).toHaveText('重试后的日志记录');
  await expect(page.locator('.history-log-list .log-record')).toHaveCount(500);
  await expect(page.locator('.history-pagination')).toBeVisible();
  await expect(page.getByText('日志文件列表暂不可用', { exact: true })).toHaveCount(0);
  expect(viewReads(api)).toHaveLength(initialReads + 1);
  expect(viewReads(api).at(-1).query).toEqual({ filename: 'app.log', page: '1', format: 'json' });
  api.respond('GET', '/api/logs/list', []);
  await page.getByRole('button', { name: '刷新历史日志', exact: true }).click();
  await expect(page.locator('.history-log-list .log-record')).toHaveCount(0);
  await expect(page.locator('.history-pagination')).toHaveCount(0);
  await expect(page.getByText('暂无日志内容', { exact: true })).toBeVisible();
  expect(viewReads(api)).toHaveLength(initialReads + 1);
});

test('closing history while the file list refreshes prevents its later content request', async ({ page, api }) => {
  await openHistory(page, api);
  const initialReads = viewReads(api).length;
  const pending = api.defer('GET', '/api/logs/list');
  await page.getByRole('button', { name: '刷新历史日志', exact: true }).click();
  await pending.started;
  await page.getByRole('button', { name: '关闭实时日志', exact: true }).click();
  await expect(page.locator('.history-log-view')).toHaveCount(0);
  const unexpectedRead = page.waitForRequest(request => new URL(request.url()).pathname === '/api/logs/view', { timeout: 500 })
    .then(() => true, error => {
      if (error.name !== 'TimeoutError') throw error;
      return false;
    });
  const finished = page.waitForResponse(response => new URL(response.url()).pathname === '/api/logs/list');
  pending.release(['app.log', 'app.log.1']);
  await (await finished).finished();
  expect(await unexpectedRead).toBe(false);
  expect(viewReads(api)).toHaveLength(initialReads);
  await expect(page.locator('.dashboard-page')).toBeVisible();
});

test('export keeps the last successful file and raw line endings while another file is pending or failed', async ({ page, api }) => {
  const lines = [`${logLine('已读取的原文件记录')}\n`, `${logLine('原文件较早记录')}\n`];
  await openHistory(page, api, lines);
  api.respond('GET', '/api/logs/list', ['app.log', 'app.log.1']);
  const refreshed = page.waitForResponse(response => new URL(response.url()).pathname === '/api/logs/view');
  await page.getByRole('button', { name: '刷新历史日志', exact: true }).click();
  await (await refreshed).finished();
  await expect(page.getByRole('button', { name: '刷新历史日志', exact: true })).not.toHaveClass(/v-btn--loading/);
  await expect(page.locator('.history-log-list .log-record')).toHaveCount(2);
  let received, release;
  const started = new Promise(resolve => { received = resolve; });
  const completion = new Promise(resolve => { release = resolve; });
  api.respond('GET', '/api/logs/view', async request => { received(request); return completion; }, 503);
  await page.locator('.history-file-select .v-field').click();
  await page.getByRole('option', { name: 'app.log.1', exact: true }).click();
  expect((await started).query.filename).toBe('app.log.1');
  const exportVisible = async () => {
    const pending = page.waitForEvent('download');
    await page.locator('.history-log-view').getByRole('button', { name: '导出日志', exact: true }).click();
    const downloaded = await pending;
    expect(downloaded.suggestedFilename()).toBe('app.log');
    const stream = await downloaded.createReadStream();
    let text = ''; for await (const chunk of stream) text += chunk.toString();
    expect(text).toBe(lines.join(''));
  };
  await exportVisible();
  const finished = page.waitForResponse(response => new URL(response.url()).pathname === '/api/logs/view');
  release({ error: 'B 日志读取失败' });
  await (await finished).finished();
  await expect(page.getByText('B 日志读取失败', { exact: true })).toBeVisible();
  await expect(page.locator('.history-log-list .log-record')).toHaveCount(2);
  await exportVisible();
});
