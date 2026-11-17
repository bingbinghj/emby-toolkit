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

for (const width of [390, 1440]) {
  test(`${width} directory and monitoring paths share the inline picker and keep unsaved multi-path drafts`, async ({ page, api }) => {
    await page.setViewportSize({ width, height: 900 });
    api.respond('GET', endpoint, request => directories(request.query.path));
    await page.goto('/setting?tab=directory');
    for (const label of ['本地 STRM 根目录', 'CD2 媒体库本地根目录']) {
      const input = page.getByLabel(label, { exact: true });
      await input.fill('/metadata');
      const button = page.getByRole('button', { name: `选择${label}`, exact: true });
      expect(await button.evaluate(el => Boolean(el.closest('.v-field__append-inner')))).toBe(true);
      await button.click();
      await expect.poll(async () => {
        const box = await picker(page).boundingBox();
        return box && Math.abs(box.x + box.width / 2 - width / 2) < 2
          && Math.abs(box.y + box.height / 2 - 450) < 2;
      }).toBe(true);
      await picker(page).getByRole('button', { name: '打开目录 chapters', exact: true }).click();
      await picker(page).getByRole('button', { name: '选择此目录', exact: true }).click();
      await expect(input).toHaveValue('/metadata/chapters');
    }
    expect(await page.getByRole('button', { name: '选择 115 云端目录', exact: true }).evaluate(el => Boolean(el.closest('.v-field__append-inner')))).toBe(true);
    await page.goto('/setting?tab=rule');
    const group = page.locator('[data-field="monitor_paths"]');
    const paths = group.getByRole('textbox', { name: '监控路径', exact: true });
    await paths.fill('/metadata/chapters');
    await paths.press('Enter');
    const button = group.getByRole('button', { name: '选择监控路径', exact: true });
    expect(await button.evaluate(el => Boolean(el.closest('.v-field__append-inner')))).toBe(true);
    await button.click();
    await expect.poll(async () => {
      const box = await picker(page).boundingBox();
      return box && Math.abs(box.x + box.width / 2 - width / 2) < 2
        && Math.abs(box.y + box.height / 2 - 450) < 2;
    }).toBe(true);
    await picker(page).getByRole('button', { name: '打开目录 chapters', exact: true }).click();
    await picker(page).getByRole('button', { name: '选择此目录', exact: true }).click();
    await expect(group.locator('.v-chip').filter({ hasText: '/metadata/chapters' })).toHaveCount(1);
    const exclude = page.getByRole('button', { name: '选择排除路径', exact: true });
    expect(await exclude.evaluate(el => Boolean(el.closest('.v-field__append-inner')))).toBe(true);
    expect(configWrites(api)).toEqual([]);
  });
}
