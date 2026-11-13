import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve, extname, sep } from 'node:path';
import { test, expect } from './fixtures.js';

test.use({ realLogStream: true });

const staticRoot = fileURLToPath(new URL('../../dist/', import.meta.url));
const encode = block => block.split('\n').map(line => `data: ${line}\n`).join('') + '\n';
const debugLine = 'DEBUG: 2026-11-13 13:19:14,476 sync.rs - 原始磁盘DEBUG历史';
const multiline = 'INFO: 2026-11-13 13:19:15,476 sync.rs - 原始多行正文\n第二行顶格显示\n第三行完整保留';
const increment = 'INFO: 2026-11-13 13:19:16,476 sync.rs - 实时流增量记录';

async function liveServer() {
  const streams = new Set(), sockets = new Set(), cookies = [], history = [debugLine, multiline];
  let connections = 0;
  const server = createServer(async (request, response) => {
    const path = new URL(request.url, 'http://localhost').pathname;
    if (path === '/api/logs/stream') {
      connections++; cookies.push(request.headers.cookie || '');
      response.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache', Connection: 'keep-alive' });
      streams.add(response);
      response.on('close', () => streams.delete(response));
      response.write('retry: 1000\n\n');
      response.write(encode(history.join('\n')));
      return;
    }
    const file = path === '/logs' || path === '/dashboard' ? resolve(staticRoot, 'index.html') : resolve(staticRoot, '.' + path);
    if (!file.startsWith(staticRoot.endsWith(sep) ? staticRoot : staticRoot + sep)) { response.writeHead(403); response.end(); return; }
    try {
      const data = await readFile(file);
      const type = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.png': 'image/png', '.woff2': 'font/woff2' }[extname(file)] || 'application/octet-stream';
      response.writeHead(200, { 'Content-Type': type }); response.end(data);
    } catch { response.writeHead(404); response.end(); }
  });
  server.on('connection', socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)); });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  return {
    origin: `http://127.0.0.1:${server.address().port}`, streams, cookies,
    get connections() { return connections; },
    append(block) { history.push(block); for (const response of streams) response.write(encode(block)); },
    disconnect() { for (const response of [...streams]) response.end(); },
    async close() { for (const socket of sockets) socket.destroy(); await new Promise(resolve => server.close(resolve)); },
  };
}

test('native EventSource sends the administrator cookie, receives DEBUG and multiline deltas, and closes owned connections', async ({ page, api }) => {
  test.setTimeout(60_000);
  const live = await liveServer();
  try {
    await page.context().addCookies([{ name: 'etk_session', value: 'fixture-stream-session', url: live.origin }]);
    api.setTask({ logs: ['INFO: 2026-11-13 13:19:14,476 status.rs - 任务快照不应作为实时日志'] });
    await page.goto(live.origin + '/logs');
    const messages = page.locator('.notification-log .log-record-message');
    await expect(messages).toHaveText(['原始磁盘DEBUG历史', '原始多行正文\n第二行顶格显示\n第三行完整保留']);
    expect(await messages.nth(1).textContent()).toBe('原始多行正文\n第二行顶格显示\n第三行完整保留');
    expect(live.cookies).toEqual(['etk_session=fixture-stream-session']);
    await expect(page.locator('.notification-log .log-record[data-level="DEBUG"]')).toHaveCount(1);
    await expect(page.getByText('任务快照不应作为实时日志', { exact: true })).toHaveCount(0);

    live.append(increment);
    await expect(messages).toHaveText(['原始磁盘DEBUG历史', '原始多行正文\n第二行顶格显示\n第三行完整保留', '实时流增量记录']);
    expect(api.requests.filter(request => request.path === '/api/logs/stream')).toHaveLength(1);
    await page.getByRole('button', { name: '暂停日志流', exact: true }).click();
    await expect.poll(() => live.streams.size).toBe(0);
    await expect(messages).toHaveCount(3);
    await page.getByRole('button', { name: '继续日志流', exact: true }).click();
    await expect.poll(() => live.connections).toBe(2);
    await expect(messages).toHaveText(['原始磁盘DEBUG历史', '原始多行正文\n第二行顶格显示\n第三行完整保留', '实时流增量记录']);

    live.disconnect();
    await expect(page.locator('.notification-log').getByRole('alert')).toContainText('实时日志连接失败');
    await expect.poll(() => live.connections).toBe(3);
    await expect(page.locator('.notification-log').getByRole('alert')).toHaveCount(0);
    await expect(messages).toHaveText(['原始磁盘DEBUG历史', '原始多行正文\n第二行顶格显示\n第三行完整保留', '实时流增量记录']);
    expect(live.cookies.every(cookie => cookie === 'etk_session=fixture-stream-session')).toBe(true);

    await page.goto(live.origin + '/dashboard');
    await expect(page.locator('.notification-center')).toHaveCount(0);
    await expect.poll(() => live.streams.size).toBe(0);
  } finally { await live.close(); }
});
