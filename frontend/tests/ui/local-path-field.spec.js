import { test, expect } from './fixtures.js';

const endpoint = '/api/p115/system/directories';
const field = page => page.getByLabel('媒体元数据目录', { exact: true });
const picker = page => page.locator('.local-path-menu');
const directories = path => ({ code: 200, current_path: path, data: path === '/metadata'
  ? [{ name: '..', path: '/', is_parent: true }, { name: 'chapters', path: '/metadata/chapters', is_parent: false }]
  : path === '/metadata/chapters' ? [{ name: '..', path: '/metadata', is_parent: true }] : [] });
const configWrites = api => api.requests.filter(request => request.method === 'POST' && request.path === '/api/config');

test('metadata directory browsing changes only the draft until its directory settings are saved', async ({ page, api }) => {
  api.respond('GET', endpoint, request => directories(request.query.path));
  await page.goto('/setting?tab=directory');
  await field(page).fill('/metadata');
  await page.getByRole('button', { name: '选择媒体元数据目录', exact: true }).click();
  await picker(page).getByRole('button', { name: '打开目录 chapters', exact: true }).click();
  await expect(picker(page).getByText('/metadata/chapters', { exact: true })).toBeVisible();
  await picker(page).getByRole('button', { name: '返回上一级目录', exact: true }).click();
  await picker(page).getByRole('button', { name: '打开目录 chapters', exact: true }).click();
  await picker(page).getByRole('button', { name: '选择此目录', exact: true }).click();
  await expect(field(page)).toHaveValue('/metadata/chapters');
  expect(configWrites(api)).toEqual([]);
  expect(api.requests.filter(request => request.path === endpoint).map(request => request.method)).toEqual(['GET', 'GET', 'GET', 'GET']);

  await page.getByRole('button', { name: '保存目录', exact: true }).click();
  await expect.poll(() => configWrites(api).length).toBe(1);
  const saved = configWrites(api)[0].body;
  expect(saved.emby_metadata_path).toBe('/metadata/chapters');
  expect(saved).not.toHaveProperty('emby_api_key');
  expect(saved).not.toHaveProperty('ai_api_key');
  expect(saved).not.toHaveProperty('telegram_bot_token');
});

test('an unavailable directory retains the draft and offers a retry without a configuration write', async ({ page, api }) => {
  api.respond('GET', endpoint, { code: 503, message: '媒体元数据目录暂不可读取' }, 503);
  await page.goto('/setting?tab=directory');
  await field(page).fill('/metadata');
  await page.getByRole('button', { name: '选择媒体元数据目录', exact: true }).click();
  await expect(picker(page).getByText('媒体元数据目录暂不可读取', { exact: true })).toBeVisible();
  await expect(picker(page).getByRole('button', { name: '选择此目录', exact: true })).toBeDisabled();
  await expect(field(page)).toHaveValue('/metadata');
  api.respond('GET', endpoint, request => directories(request.query.path));
  await picker(page).getByRole('button', { name: '重试读取目录', exact: true }).click();
  await expect(picker(page).getByRole('button', { name: '打开目录 chapters', exact: true })).toBeVisible();
  await picker(page).getByRole('button', { name: '取消', exact: true }).click();
  await expect(field(page)).toHaveValue('/metadata');
  expect(configWrites(api)).toEqual([]);
});

test('a directory response arriving after cancellation cannot replace the reopened picker or its input', async ({ page, api }) => {
  const old = api.defer('GET', endpoint);
  await page.goto('/setting?tab=directory');
  await field(page).fill('/old');
  await page.getByRole('button', { name: '选择媒体元数据目录', exact: true }).click();
  await old.started;
  await picker(page).getByRole('button', { name: '取消', exact: true }).click();
  await expect(picker(page)).toBeHidden();
  await field(page).fill('/new');
  api.respond('GET', endpoint, request => directories(request.query.path));
  await page.getByRole('button', { name: '选择媒体元数据目录', exact: true }).click();
  await expect(picker(page).getByText('/new', { exact: true })).toBeVisible();
  const finished = page.waitForResponse(response => new URL(response.url()).pathname === endpoint && new URL(response.url()).searchParams.get('path') === '/old');
  old.release(directories('/old'));
  await finished;
  await expect(picker(page).getByText('/old', { exact: true })).toHaveCount(0);
  await picker(page).getByRole('button', { name: '选择此目录', exact: true }).click();
  await expect(field(page)).toHaveValue('/new');
  expect(configWrites(api)).toEqual([]);
});
