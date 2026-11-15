import { test, expect } from './fixtures.js';

const historyPath = '/api/media_organize/history';
const queuePath = '/api/media_organize/queue';
const organizePath = '/api/actions/reorganize';
const movie = { id: '00000000-0000-4000-8000-000000000101', item_id: '101', name: '队列验证电影', item_type: 'Movie', status: 'success' };
const episode = { id: '00000000-0000-4000-8000-000000000008', item_id: 'episode-8', name: '第八集', item_type: 'Episode', series_id: 'series-1', series_name: '队列验证电视剧', season_number: 1, episode_number: 8, status: 'success' };
const row = (id, item = movie, changes = {}) => ({ id, item_id: item.item_id, item_json: { ...item }, mode: 'reprocess', status: 'queued', message: '等待处理', progress: 0, progress_known: false, current_action: '', result: null, ...changes });
const queuePage = items => ({ items, total: items.length, page: 1, page_size: 100,
  queued_count: items.filter(item => item.status === 'queued').length,
  running_count: items.filter(item => item.status === 'running').length });
const historyPage = items => ({ items, total: items.length });
const task = (page, id) => page.getByTestId('organize-queue-task-' + id);
const queueRequests = api => api.requests.filter(request => request.method === 'GET' && request.path === queuePath);
const openQueue = async page => {
  await page.getByTestId('organize-queue-open').click();
  await expect(page.getByTestId('organize-queue-dialog')).toBeVisible();
};
const refreshQueue = page => page.getByRole('button', { name: '刷新整理队列', exact: true }).click();

for (const viewport of [{ width: 1280, height: 900 }, { width: 390, height: 844 }]) {
  test(`the queue opens above pagination on desktop and above the dock on mobile at ${viewport.width}px without widening the page`, async ({ page, api }) => {
    await page.setViewportSize(viewport);
    api.respond('GET', historyPath, historyPage([]));
    api.respond('GET', queuePath, queuePage([]));
    await page.goto('/media-organize');
    const button = page.getByTestId('organize-queue-open');
    await expect(button).toBeVisible();
    const bounds = await button.boundingBox();
    expect(bounds.x).toBeGreaterThan(viewport.width - 120);
    if (viewport.width >= 960) {
      const footer = await page.getByTestId('organize-footer').boundingBox();
      expect(bounds.y + bounds.height).toBeLessThanOrEqual(footer.y - 20);
      expect(viewport.width - bounds.x - bounds.width).toBeGreaterThanOrEqual(30);
    } else {
      expect(bounds.y).toBeGreaterThan(viewport.height - 200);
    }
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(viewport.width);
    await openQueue(page);
    await expect(page.getByTestId('organize-queue-dialog')).toContainText('当前没有整理任务');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.getByRole('button', { name: '关闭整理队列', exact: true }).click();
    await expect(page.getByTestId('organize-queue-dialog')).not.toBeVisible();
    expect(api.requests.some(request => request.method !== 'GET' && request.path.startsWith(queuePath))).toBe(false);
  });
}

test('rescraping one episode enters the queue immediately without claiming completion or expanding its scope', async ({ page, api }) => {
  let items = [];
  api.respond('GET', historyPath, historyPage([episode]));
  api.respond('GET', queuePath, () => queuePage(items));
  api.respond('POST', organizePath, () => {
    items = [row('episode-rescrape', episode, { mode: 'rescrape' })];
    return { mode: 'rescrape', queued_count: 1, duplicate_count: 0, queue_ids: ['episode-rescrape'], items, message: '已加入重新刮削队列' };
  }, 202);
  await page.goto('/media-organize');
  await page.getByTestId('organize-items').locator('tbody tr').filter({ has: page.locator('input[type="checkbox"]') }).first().getByRole('checkbox').check();
  await page.getByTestId('organize-selected').click();
  await page.getByTestId('organize-mode').click();
  await page.locator('.v-overlay--active').getByRole('option', { name: '重新刮削', exact: true }).click();
  await expect(page.locator('.v-overlay--active').filter({ has: page.getByTestId('organize-confirm') })).toContainText('保留已有媒体信息、音轨和章节');
  await page.getByTestId('organize-confirm').click();
  await expect(page.getByTestId('organize-queue-dialog')).toBeVisible();
  await expect(task(page, 'episode-rescrape')).toContainText('重新刮削');
  await expect(task(page, 'episode-rescrape')).toContainText('等待处理');
  await expect(task(page, 'episode-rescrape')).not.toContainText('已完成');
  await expect(page.getByTestId('organize-result')).toContainText('已加入重新刮削队列');
  const posts = api.requests.filter(request => request.method === 'POST' && request.path === organizePath);
  expect(posts).toHaveLength(1);
  expect(posts[0].body).toEqual({ item_ids: ['episode-8'], history_ids: [episode.id], mode: 'rescrape' });
  expect(api.requests.some(request => request.path.startsWith('/api/tasks/result/'))).toBe(false);
});

test('running progress distinguishes an unknown percentage from measured progress and a real failed result', async ({ page, api }) => {
  let current = row('progress-task', movie, { status: 'running', progress: 87, progress_known: false, current_action: '读取当前媒体' });
  api.respond('GET', historyPath, historyPage([]));
  api.respond('GET', queuePath, () => queuePage([current]));
  await page.goto('/media-organize');
  await openQueue(page);
  const item = task(page, current.id);
  await expect(item).toContainText('正在处理');
  await expect(item).toContainText('处理中 · 读取当前媒体');
  await expect(item).not.toContainText('87%');
  await expect(item).not.toContainText('0%');
  await expect(item.getByRole('progressbar')).not.toHaveAttribute('aria-valuenow');
  current = { ...current, progress_known: true, progress: 37, current_action: '更新 NFO' };
  await refreshQueue(page);
  await expect(item).toContainText('37% · 更新 NFO');
  await expect(item.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '37');
  current = { ...current, status: 'failed', message: '媒体回读未通过', result: { failures: ['当前文件身份改变'], warning_details: ['核查材料已保留'] } };
  await refreshQueue(page);
  await expect(item).toContainText('失败');
  await expect(item).toContainText('当前文件身份改变');
  await expect(item).toContainText('核查材料已保留');
  await expect(item).not.toContainText('已完成');
  await expect(item.locator('.v-progress-linear')).toHaveCount(0);
});

test('pause and stop states preserve measured progress without presenting continued processing', async ({ page, api }) => {
  let current = row('paused-task', movie, { status: 'running', job_state: 'paused', pause_requested: true, progress: 37, progress_known: true, message: '已到安全暂停点' });
  api.respond('GET', historyPath, historyPage([]));
  api.respond('GET', queuePath, () => queuePage([current]));
  await page.goto('/media-organize');
  await openQueue(page);
  const item = task(page, current.id);
  await expect(item.locator('.v-chip')).toHaveText('已暂停');
  await expect(item.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '37');
  await expect(item.getByRole('button', { name: /取消等待任务/ })).toBeDisabled();
  current = { ...current, job_state: 'pausing', progress_known: false, message: '等待安全暂停点' };
  await refreshQueue(page);
  await expect(item.locator('.v-chip')).toHaveText('暂停中');
  await expect(item.getByRole('progressbar')).toHaveCount(0);
  await expect(item).not.toContainText('处理中');
  current = { ...current, job_state: 'stopping', stop_requested: true, progress_known: true, message: '保存收尾结果' };
  await refreshQueue(page);
  await expect(item.locator('.v-chip')).toHaveText('停止中');
  await expect(item.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '37');
  current = { ...current, job_state: 'running', stop_requested: false, pause_requested: false, message: '继续执行' };
  await refreshQueue(page);
  await expect(item.locator('.v-chip')).toHaveText('正在处理');
  expect(api.requests.some(request => request.method !== 'GET' && request.path.startsWith(queuePath))).toBe(false);
});

test('only waiting tasks can be cancelled and double clicks submit one cancellation', async ({ page, api }) => {
  const waitingEpisode = { ...episode, item_id: 'episode-9', name: '第九集', episode_number: 9 };
  let items = [row('running-task', episode, { status: 'running', message: '正在执行' }), row('waiting-task', waitingEpisode)];
  api.respond('GET', historyPath, historyPage([]));
  api.respond('GET', queuePath, () => queuePage(items));
  const pending = api.defer('DELETE', queuePath + '/waiting-task');
  await page.goto('/media-organize');
  await openQueue(page);
  await expect(task(page, 'running-task').getByRole('button', { name: /取消等待任务/ })).toBeDisabled();
  await task(page, 'waiting-task').getByRole('button', { name: /取消等待任务/ }).dblclick();
  await pending.started;
  expect(api.requests.filter(request => request.method === 'DELETE' && request.path === queuePath + '/waiting-task')).toHaveLength(1);
  items = items.map(item => item.id === 'waiting-task' ? { ...item, status: 'cancelled', message: '等待任务已取消' } : item);
  pending.release({ item: items.find(item => item.id === 'waiting-task'), message: '等待任务已取消' });
  await expect(task(page, 'waiting-task')).toContainText('已取消');
  await expect(task(page, 'running-task')).toContainText('正在执行');
  expect(api.requests.some(request => request.method === 'DELETE' && request.path === queuePath + '/running-task')).toBe(false);
});

test('a cancellation conflict keeps the task visible until its current server state is read', async ({ page, api }) => {
  let current = row('cancel-race');
  api.respond('GET', historyPath, historyPage([]));
  api.respond('GET', queuePath, () => queuePage([current]));
  api.respond('DELETE', queuePath + '/cancel-race', { error: '任务已开始，不能取消等待任务' }, 409);
  await page.goto('/media-organize');
  await openQueue(page);
  await task(page, 'cancel-race').getByRole('button', { name: /取消等待任务/ }).click();
  await expect(page.getByTestId('organize-queue-action-result')).toContainText('任务已开始，不能取消等待任务');
  await expect(task(page, 'cancel-race')).toContainText('等待处理');
  await expect(task(page, 'cancel-race')).not.toContainText('已取消');
  current = { ...current, status: 'running', message: '正在执行' };
  await refreshQueue(page);
  await expect(task(page, 'cancel-race')).toContainText('正在执行');
  await expect(task(page, 'cancel-race').getByRole('button', { name: /取消等待任务/ })).toBeDisabled();
});

test('retrying a failed scrape submits once and keeps its original operation mode', async ({ page, api }) => {
  let current = row('retry-scrape', movie, { status: 'failed', mode: 'rescrape', message: '上次刮削失败' });
  let receive, release;
  const started = new Promise(resolve => { receive = resolve; });
  const completion = new Promise(resolve => { release = resolve; });
  const retryPath = queuePath + '/retry-scrape/retry';
  api.respond('GET', historyPath, historyPage([]));
  api.respond('GET', queuePath, () => queuePage([current]));
  api.respond('POST', retryPath, request => { receive(request); return completion; }, 202);
  await page.goto('/media-organize');
  await openQueue(page);
  await task(page, 'retry-scrape').getByRole('button', { name: /重试任务/ }).dblclick();
  await started;
  expect(api.requests.filter(request => request.method === 'POST' && request.path === retryPath)).toHaveLength(1);
  current = { ...current, status: 'queued', message: '等待重新刮削' };
  release({ mode: current.mode, queued_count: 1, duplicate_count: 0, queue_ids: [current.id], items: [current], message: '已重新加入整理队列' });
  await expect(task(page, 'retry-scrape')).toContainText('等待重新刮削');
  await expect(task(page, 'retry-scrape')).toContainText('重新刮削');
  await expect(task(page, 'retry-scrape').getByRole('button', { name: /取消等待任务/ })).toBeEnabled();
  expect(api.requests.some(request => request.path.startsWith('/api/tasks/result/'))).toBe(false);
});

test('retry blocks the same media and operation while allowing a different queued operation', async ({ page, api }) => {
  api.respond('GET', historyPath, historyPage([]));
  api.respond('GET', queuePath, queuePage([
    row('old-failure', movie, { status: 'failed', mode: 'reprocess' }),
    row('old-scrape-failure', movie, { status: 'failed', mode: 'rescrape' }),
    row('current-job', movie, { status: 'running', mode: 'reprocess' }),
  ]));
  await page.goto('/media-organize');
  await openQueue(page);
  await expect(task(page, 'old-failure').getByRole('button', { name: /重试任务/ })).toBeDisabled();
  await expect(task(page, 'old-scrape-failure').getByRole('button', { name: /重试任务/ })).toBeEnabled();
  expect(api.requests.some(request => request.method === 'POST' && request.path.startsWith(queuePath))).toBe(false);
});

test('an unreadable queue retains its last tasks and can be retried without showing a false empty state', async ({ page, api }) => {
  const current = row('retained-task');
  api.respond('GET', historyPath, historyPage([]));
  api.respond('GET', queuePath, queuePage([current]));
  await page.goto('/media-organize');
  await openQueue(page);
  await expect(task(page, current.id)).toBeVisible();
  api.respond('GET', queuePath, { error: '整理队列暂时无法读取' }, 503);
  await refreshQueue(page);
  await expect(page.getByTestId('organize-queue-error')).toContainText('整理队列暂时无法读取');
  await expect(task(page, current.id)).toBeVisible();
  await expect(page.getByTestId('organize-queue-dialog')).not.toContainText('当前没有整理任务');
  api.respond('GET', queuePath, queuePage([{ ...current, message: '服务器状态已重新读取' }]));
  await page.getByTestId('organize-queue-error').getByRole('button', { name: '重新读取', exact: true }).click();
  await expect(page.getByTestId('organize-queue-error')).toHaveCount(0);
  await expect(task(page, current.id)).toContainText('服务器状态已重新读取');
});

test('slow queue polls stay serial and late responses cannot restart polling after leaving the page', async ({ page, api }) => {
  await page.clock.install();
  const current = row('slow-task');
  api.respond('GET', historyPath, historyPage([]));
  api.respond('GET', queuePath, queuePage([current]));
  await page.goto('/media-organize');
  await openQueue(page);
  await expect(task(page, current.id)).toBeVisible();
  await expect(page.getByRole('button', { name: '刷新整理队列', exact: true })).not.toHaveClass(/v-btn--loading/);
  const before = queueRequests(api).length;
  const pending = api.defer('GET', queuePath);
  await page.clock.fastForward(3000);
  await pending.started;
  await expect(task(page, current.id)).toBeVisible();
  await page.clock.fastForward(6000);
  expect(queueRequests(api)).toHaveLength(before + 1);
  const failed = page.waitForEvent('requestfailed', request => new URL(request.url()).pathname === queuePath);
  await page.evaluate(async () => {
    await document.getElementById('app').__vue_app__.config.globalProperties.$router.push('/dashboard');
  });
  await expect(page.locator('.dashboard-page')).toBeVisible();
  pending.release(queuePage([{ ...current, message: '离开页面后的响应' }]));
  await failed;
  await page.clock.fastForward(12_000);
  expect(queueRequests(api)).toHaveLength(before + 1);
  await expect(page.getByTestId('organize-queue-open')).toHaveCount(0);
  await expect(page.getByTestId('organize-queue-dialog')).not.toBeVisible();
});

test('groups use media identity and season and preserve the active media when polling order changes', async ({ page, api }) => {
  const sameTitle = { ...movie, name: '同名影片' };
  const anotherMovie = { ...sameTitle, item_id: 'another-movie' };
  const seasonOne = { ...episode, series_name: '同名电视剧' };
  const seasonTwo = { ...seasonOne, item_id: 'season-two-episode', season_number: 2 };
  let items = [row('movie-one', sameTitle), row('movie-two', anotherMovie), row('season-one', seasonOne), row('season-two', seasonTwo)];
  api.respond('GET', historyPath, historyPage([]));
  api.respond('GET', queuePath, () => queuePage(items));
  await page.goto('/media-organize');
  await openQueue(page);
  const groups = page.getByRole('navigation', { name: '队列媒体列表', exact: true });
  await expect(groups.getByRole('button')).toHaveCount(4);
  await groups.getByRole('button', { name: /同名影片/ }).nth(1).click();
  await expect(task(page, 'movie-two')).toBeVisible();
  await expect(task(page, 'movie-one')).toHaveCount(0);
  items = [...items].reverse();
  await refreshQueue(page);
  await expect(task(page, 'movie-two')).toBeVisible();
  await expect(groups.locator('[aria-current="true"]')).toHaveCount(1);
  await groups.getByRole('button', { name: /同名电视剧 S02/ }).click();
  await expect(task(page, 'season-two')).toBeVisible();
  await expect(task(page, 'season-one')).toHaveCount(0);
  await expect(groups.getByRole('button', { name: /同名电视剧 S01/ })).toBeVisible();
});

test('actual completion refreshes admission history and preserves returned warning details', async ({ page, api }) => {
  let completed = false;
  api.respond('GET', historyPath, () => historyPage([{ ...movie, status: completed ? 'warning' : 'failed', message: completed ? '媒体已刮削，图片待确认' : '首次刮削失败' }]));
  api.respond('GET', queuePath, () => queuePage([row('completion-task', movie, completed
    ? { status: 'completed', mode: 'rescrape', message: '重新刮削已完成', result: { warning_details: ['图片待确认'] } }
    : { status: 'running', mode: 'rescrape', message: '正在重新刮削' })]));
  await page.goto('/media-organize');
  await openQueue(page);
  await expect(task(page, 'completion-task')).toContainText('正在重新刮削');
  const before = api.requests.filter(request => request.path === historyPath).length;
  completed = true;
  await refreshQueue(page);
  await expect(task(page, 'completion-task')).toContainText('重新刮削已完成');
  await expect(task(page, 'completion-task')).toContainText('图片待确认');
  await page.getByRole('button', { name: '关闭整理队列', exact: true }).click();
  await expect(page.getByTestId('organize-items')).toContainText('媒体已刮削，图片待确认');
  expect(api.requests.filter(request => request.path === historyPath).length).toBeGreaterThan(before);
});
