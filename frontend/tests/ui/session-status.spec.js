import { test, expect } from './fixtures.js';

test('verified session expiry removes the old task card and stops status polling', async ({ page, api }) => {
  api.setTask({ task_id: 'expired-session-task', is_running: true, state: 'running',
    current_action: '正在保存媒体信息', progress: 42, progress_known: true, message: '任务仍在服务器运行', logs: [] });
  await page.goto('/dashboard');
  const card = page.getByRole('button', { name: '查看当前任务', exact: true });
  await expect(card).toBeVisible();
  const authReads = () => api.requests.filter(request => request.method === 'GET' && request.path === '/api/auth/check_status').length;
  const statusReads = () => api.requests.filter(request => request.method === 'GET' && request.path === '/api/status').length;
  const initialAuthReads = authReads();
  api.respond('GET', '/api/auth/check_status', { status: 'login_required' });
  api.respond('GET', '/api/status', { error: '登录会话已失效' }, 403);
  await expect.poll(authReads).toBeGreaterThan(initialAuthReads);
  await expect(card).toHaveCount(0);
  const finalStatusReads = statusReads();
  await page.waitForTimeout(2300);
  expect(statusReads()).toBe(finalStatusReads);
});
