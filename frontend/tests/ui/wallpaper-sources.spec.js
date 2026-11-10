import { test, expect } from './fixtures.js';

test.describe.configure({ timeout: 60_000 });
const key = 'etk-wallpaper-settings';
const first = '/image_proxy/Items/101/Images/Backdrop/0?tag=first&quality=90';
const second = '/image_proxy/Items/102/Images/Backdrop/0?tag=second&quality=90';
const selectField = (panel, label) => panel.getByRole('combobox').filter({ hasText: label });

async function appearance(page, api) {
  for (const source of ['tmdb', 'bing', 'emby']) api.respond('GET', `/api/wallpaper/${source}`, { source, images: [first, second] });
  await page.goto('/settings/general');
  await page.getByRole('button', { name: '主题与壁纸', exact: true }).click();
  return page.locator('.theme-customizer-panel-host');
}

test('MP wallpaper sources display original Emby backdrops and keep saved long intervals', async ({ page, api }) => {
  const panel = await appearance(page, api);
  await panel.getByRole('button', { name: '来源', exact: true }).click();
  await selectField(panel, '壁纸来源').click();
  await page.getByRole('option', { name: 'Emby 媒体背景', exact: true }).click();
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', first);
  await selectField(panel, '轮换间隔').click();
  await page.getByRole('option', { name: '1 天', exact: true }).click();
  await expect.poll(() => page.evaluate(key => JSON.parse(localStorage.getItem(key)).rotationInterval, key)).toBe(86400);
  await page.reload();
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', first);
  expect(await page.evaluate(key => JSON.parse(localStorage.getItem(key)).source, key)).toBe('emby');
});

test('an unavailable source keeps the previous background and presents a retry action', async ({ page, api }) => {
  const panel = await appearance(page, api);
  await panel.getByRole('button', { name: '来源', exact: true }).click();
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', first);
  api.respond('GET', '/api/wallpaper/bing', { error: 'Bing 暂不可用' }, 502);
  await selectField(panel, '壁纸来源').click();
  await page.getByRole('option', { name: 'Bing 每日壁纸', exact: true }).click();
  await expect(panel.getByText(/Bing 暂不可用/)).toBeVisible();
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', first);
  api.respond('GET', '/api/wallpaper/bing', { source: 'bing', images: [second] });
  await panel.getByRole('button', { name: '重新读取壁纸', exact: true }).click();
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', second);
});

test('remote wallpaper rotation honors the selected MP interval and reduced motion', async ({ page, api }) => {
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.addInitScript(() => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme: 'dark', glassEnabled: false })));
  const panel = await appearance(page, api);
  await panel.getByRole('button', { name: '来源', exact: true }).click();
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', first);
  await page.clock.install();
  await selectField(panel, '轮换间隔').click();
  await page.getByRole('option', { name: '15 秒', exact: true }).click();
  await page.clock.fastForward(15000);
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', second);
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.clock.fastForward(45000);
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', second);
});

test('legacy rotation preferences remain selected while the new MP choices are available', async ({ page, api }) => {
  await page.addInitScript(() => localStorage.setItem('etk-wallpaper-settings', JSON.stringify({ mode: 'preset', presetId: 'lake', rotationInterval: 120 })));
  const panel = await appearance(page, api);
  await expect(panel.getByText('120 秒（已保存）', { exact: true })).toBeVisible();
  expect(await page.evaluate(key => JSON.parse(localStorage.getItem(key)).rotationInterval, key)).toBe(120);
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', '/wallpapers/lake.svg');
});

test('a custom API returning one image is reread at the rotation interval', async ({ page, api }) => {
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.addInitScript(() => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme: 'dark', glassEnabled: false })));
  const apiFirst = '/image_proxy/Items/103/Images/Backdrop/0?tag=api-first&quality=90';
  const apiSecond = '/image_proxy/Items/104/Images/Backdrop/0?tag=api-second&quality=90';
  let reads = 0;
  await page.route('https://wallpaper-api.invalid/list', route => route.fulfill({
    headers: { 'Access-Control-Allow-Origin': '*' }, json: { images: [reads++ ? apiSecond : apiFirst] },
  }));
  const panel = await appearance(page, api);
  await panel.getByRole('button', { name: '来源', exact: true }).click();
  await selectField(panel, '壁纸来源').click();
  await page.getByRole('option', { name: '自定义壁纸 API', exact: true }).click();
  await panel.getByLabel('壁纸 API 地址', { exact: true }).fill('https://wallpaper-api.invalid/list');
  await panel.getByRole('button', { name: '应用 API', exact: true }).click();
  await expect.poll(() => reads).toBe(1);
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', apiFirst);
  await expect(panel.getByText(/上一次成功背景仍保留/)).toHaveCount(0);
  await page.clock.install();
  await selectField(panel, '轮换间隔').click();
  await page.getByRole('option', { name: '15 秒', exact: true }).click();
  await page.clock.fastForward(15000);
  await expect.poll(() => reads).toBe(2);
  await expect(page.locator('.wallpaper-background img').last()).toHaveAttribute('src', apiSecond);
  await expect(panel.getByText(/上一次成功背景仍保留/)).toHaveCount(0);
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.clock.fastForward(45000);
  expect(reads).toBe(2);
});

for (const [theme, glass] of [['light', false], ['dark', true]]) {
  test(`${theme} theme keeps MP font families and readable title colors when font downloads fail`, async ({ page, api }) => {
    await page.addInitScript(([theme, glass]) => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme, glassEnabled: glass })), [theme, glass]);
    await appearance(page, api);
    await expect(page.locator('link#moviepilot-fonts')).toHaveAttribute('href', /family=Inter:.*family=Noto\+Sans\+SC.*display=swap/);
    const styles = await page.getByText('基础设置', { exact: true }).evaluate(element => {
      const css = getComputedStyle(element);
      return { font: css.fontFamily, color: css.color };
    });
    expect(styles.font).toContain('Inter');
    expect(styles.font).toContain('Noto Sans SC');
    expect(styles.color).not.toMatch(/^rgba\(.*?,\s*0\)$/);
    await expect(page.getByText('基础设置', { exact: true })).toBeVisible();
  });
}
