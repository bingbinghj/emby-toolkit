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

for (const width of [390, 1440]) {
  test(`${width} history uses page buttons and direct jumps, retaining the old page after a failed read`, async ({ page, api }) => {
    await page.setViewportSize({ width, height: 900 });
    await openHistory(page, api);
    api.respond('GET', '/api/logs/view', request => ({ file: request.query.filename, lines: [logLine(`第 ${request.query.page} 页内容`)], page: Number(request.query.page), pages: 12, total_lines: 5501 }));
    await page.getByRole('button', { name: '刷新历史日志', exact: true }).click();
    const pagination = page.locator('.history-pagination');
    await expect(pagination).toContainText('第 1 / 12 页，共 5501 行');
    await expect(page.getByRole('button', { name: '导出日志', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: /较新日志|较早日志/ })).toHaveCount(0);
    await expect(page.getByRole('button', { name: '下载实时日志', exact: true })).toBeVisible();
    await pagination.getByRole('button', { name: '下一页', exact: true }).click();
    await expect(page.locator('.history-log-list')).toContainText('第 2 页内容');
    await pagination.getByRole('spinbutton', { name: '跳转页码', exact: true }).fill('11');
    await pagination.getByRole('spinbutton', { name: '跳转页码', exact: true }).press('Enter');
    await expect(page.locator('.history-log-list')).toContainText('第 11 页内容');
    expect(viewReads(api).at(-1).query.page).toBe('11');
    api.respond('GET', '/api/logs/view', { error: '请求日志页失败' }, 503);
    await pagination.getByRole('button', { name: '下一页', exact: true }).click();
    await expect(page.getByText('请求日志页失败', { exact: true })).toBeVisible();
    await expect(pagination).toContainText('第 11 / 12 页');
    await expect(pagination.getByRole('spinbutton', { name: '跳转页码', exact: true })).toHaveValue('11');
    await expect(page.locator('.history-log-list')).toContainText('第 11 页内容');
    const count = viewReads(api).length;
    await pagination.getByRole('spinbutton', { name: '跳转页码', exact: true }).fill('13');
    await pagination.getByRole('button', { name: '跳转', exact: true }).click();
    expect(viewReads(api)).toHaveLength(count);
    expect(await pagination.evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
  });
}
