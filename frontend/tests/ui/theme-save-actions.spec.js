import { test, expect } from './fixtures.js';

const cases = [
  { theme: 'light', glassEnabled: false, primaryColor: '#FFFFFF' },
  { theme: 'dark', glassEnabled: false, primaryColor: '#000000' },
  { theme: 'light', glassEnabled: true, glassAppearance: 'clear', primaryColor: '#8D51F9' },
  { theme: 'dark', glassEnabled: true, glassAppearance: 'clear', primaryColor: '#999999' },
  { theme: 'light', glassEnabled: true, glassAppearance: 'tinted', primaryColor: '#FFFFFF' },
  { theme: 'dark', glassEnabled: true, glassAppearance: 'tinted', primaryColor: '#1976D2' },
  { theme: 'light', glassEnabled: true, glassAppearance: 'frosted', primaryColor: '#FFB400' },
  { theme: 'dark', glassEnabled: true, glassAppearance: 'frosted', primaryColor: '#999999' },
];

async function prepare(page, options) {
  await page.emulateMedia({ colorScheme: options.theme });
  await page.addInitScript(options => {
    localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ ...options,
      glassQuality: 'css', glassDynamicsMode: 'off', layout: 'vertical' }));
  }, options);
}

async function expectFilledPrimary(button) {
  await expect(button).toBeVisible();
  await expect(button).toBeEnabled();
  const values = await button.evaluate(element => {
    const style = getComputedStyle(element);
    const rgb = token => `rgb(${style.getPropertyValue(token).trim().split(/\s*,\s*/).join(', ')})`;
    return { background: style.backgroundColor, primary: rgb('--v-theme-primary'), onPrimary: rgb('--v-theme-on-primary'),
      foreground: getComputedStyle(element.querySelector('.v-btn__content')).color,
      icon: getComputedStyle(element.querySelector('.v-btn__prepend .v-icon')).color };
  });
  expect(values.background).toBe(values.primary);
  expect(values.foreground).toBe(values.onPrimary);
  expect(values.icon).toBe(values.onPrimary);
  const position = await button.evaluate(element => {
    const footer = element.closest('.v-card-actions');
    return { left: element.getBoundingClientRect().left, expected: footer.getBoundingClientRect().left + parseFloat(getComputedStyle(footer).paddingLeft) };
  });
  expect(Math.abs(position.left - position.expected)).toBeLessThanOrEqual(2);
  const luminance = color => color.match(/[\d.]+/g).slice(0, 3).map(channel => Number(channel) / 255)
    .reduce((sum, value, index) => sum + (value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4)
      * [.2126, .7152, .0722][index], 0);
  const background = luminance(values.background), foreground = luminance(values.foreground);
  expect((Math.max(background, foreground) + .05) / (Math.min(background, foreground) + .05)).toBeGreaterThanOrEqual(4.5);
}

for (const options of cases) {
  test(`${options.theme} ${options.glassEnabled ? options.glassAppearance : 'plain'} ${options.primaryColor} keeps card and dialog saves filled`, async ({ page, api }) => {
    await prepare(page, options);
    await page.goto('/settings/general?tab=system');
    await expectFilledPrimary(page.getByRole('button', { name: '保存基础设置', exact: true }));
    await page.getByRole('button', { name: '编辑 Emby 媒体服务器', exact: true }).click();
    await expectFilledPrimary(page.getByRole('button', { name: '保存媒体服务器', exact: true }));
    await page.getByRole('button', { name: '取消媒体服务器配置', exact: true }).click();
    expect(api.requests.filter(request => request.method !== 'GET' && request.path !== '/api/config/rename_preview')).toEqual([]);
  });
}

test('changing the custom primary color updates the existing save button without reloading', async ({ page }) => {
  await prepare(page, cases[3]);
  await page.goto('/settings/general?tab=system');
  const save = page.getByRole('button', { name: '保存基础设置', exact: true });
  await expectFilledPrimary(save);
  await page.getByRole('button', { name: '主题与壁纸', exact: true }).click();
  await page.locator('.theme-customizer-native-color').evaluate(input => {
    input.value = '#FFFFFF';
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await expect.poll(() => save.evaluate(element => getComputedStyle(element).backgroundColor)).toBe('rgb(255, 255, 255)');
  await page.locator('.theme-customizer-header').getByRole('button', { name: '关闭', exact: true }).click();
  await expectFilledPrimary(save);
});

test('media rating and hover details follow the current primary and surface palettes', async ({ page, api }) => {
  await prepare(page, cases[4]);
  api.respond('GET', '/api/media_center/libraries/movies/items', { library: { id: 'movies', name: '电影', collection_type: 'movies' },
    total: 1, start_index: 0, items: [{ Id: '101', Name: '主题评分验证', Type: 'Movie', CommunityRating: 8.8,
      Overview: '主题控制媒体详情前景与背景', ProductionYear: 2026, ImageTags: { Primary: 'fixture' } }] });
  await page.goto('/library/movies');
  const rating = page.getByLabel('Emby 社区评分 8.8', { exact: true });
  await expect(rating).toBeVisible();
  expect(await rating.evaluate(element => getComputedStyle(element).backgroundColor)).toBe('rgb(255, 255, 255)');
  expect(await rating.locator('.v-chip__content').evaluate(element => getComputedStyle(element).color)).toBe('rgb(0, 0, 0)');
  await page.getByRole('button', { name: '编辑 主题评分验证', exact: true }).hover();
  const detail = page.locator('.media-card-detail');
  await expect(detail).toBeVisible();
  const colors = await detail.evaluate(element => {
    const style = getComputedStyle(element);
    return { foreground: style.color, title: getComputedStyle(element.querySelector('.media-detail-title')).color,
      background: style.backgroundImage, page: getComputedStyle(document.body).backgroundColor };
  });
  expect(colors.foreground).toBe('rgb(0, 0, 0)');
  expect(colors.title).toBe(colors.foreground);
  expect(colors.background).toContain('rgba(255, 255, 255, 0.96)');
  expect(colors.page).not.toBe('rgb(255, 255, 255)');
});
