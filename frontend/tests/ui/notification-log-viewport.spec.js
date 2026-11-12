import { readFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';

const lines = Array.from({ length: 30 }, (_, i) => `[12:30:${String(i).padStart(2, '0')}] [INFO] 第 ${i + 1} 条日志完整正文`);
const bellPath = JSON.parse(readFileSync(new URL('../../node_modules/@iconify-json/mdi/icons.json', import.meta.url))).icons['bell-outline'].body.match(/d="([^"]+)"/)[1];

async function roundedAncestorsContainGlyph(locator) {
  return locator.evaluate(element => {
    const text = [...element.childNodes].find(node => node.nodeType === Node.TEXT_NODE && node.textContent.trim());
    const range = document.createRange();
    range.setStart(text, 0); range.setEnd(text, 1);
    const glyph = range.getBoundingClientRect();
    const points = [[glyph.left, glyph.top], [glyph.right, glyph.top], [glyph.left, glyph.bottom], [glyph.right, glyph.bottom]];
    const failures = [];
    for (let ancestor = element.parentElement; ancestor; ancestor = ancestor.parentElement) {
      const style = getComputedStyle(ancestor);
      if (!['hidden', 'clip', 'auto', 'scroll'].some(value => [style.overflowX, style.overflowY].includes(value))) continue;
      const rect = ancestor.getBoundingClientRect();
      const radii = ['borderTopLeftRadius', 'borderTopRightRadius', 'borderBottomRightRadius', 'borderBottomLeftRadius'].map(key => Math.min(parseFloat(style[key]) || 0, rect.width / 2, rect.height / 2));
      const corners = [[rect.left, rect.top, 1, 1], [rect.right, rect.top, -1, 1], [rect.right, rect.bottom, -1, -1], [rect.left, rect.bottom, 1, -1]];
      for (const [x, y] of points) {
        if (x < rect.left - 1 || x > rect.right + 1 || y < rect.top - 1 || y > rect.bottom + 1) failures.push(ancestor.className);
        corners.forEach(([cx, cy, dx, dy], i) => {
          const radius = radii[i], px = (x - cx) * dx, py = (y - cy) * dy;
          if (radius && px < radius && py < radius && (px - radius) ** 2 + (py - radius) ** 2 > radius ** 2 + 1) failures.push(ancestor.className);
        });
      }
    }
    return failures;
  });
}

test('MP realtime logs group adjacent bursts and keep original groups when filtering or pausing', async ({ page, api }) => {
  test.setTimeout(90_000);
  const make = (time, level, body) => `2026-11-11 12:30:45,${time} - media::source - ${level} - ${body}`;
  const records = [make('001', 'INFO', '第一条'), make('040', 'INFO', '第二条'), make('060', 'WARNING', '提醒'), make('070', 'INFO', '第三条')];
  api.setTask({ logs: records });
  await page.goto('/dashboard');
  await page.getByRole('button', { name: '实时日志', exact: true }).click();
  await expect(page.locator('.notification-log .log-record')).toHaveCount(3);
  await expect(page.locator('.notification-log .log-record').first().locator('.log-record-body')).toHaveText(['第一条', '第二条']);
  await page.locator('.logging-level-toggle').getByRole('button', { name: 'INFO', exact: true }).click();
  await expect(page.locator('.notification-log .log-record')).toHaveCount(2);
  await expect(page.locator('.notification-footer > span')).toContainText('3 条日志');
  await page.getByRole('button', { name: '暂停日志流', exact: true }).click();
  api.setTask({ logs: [...records, make('200', 'INFO', '后续记录')] });
  await expect(page.getByRole('button', { name: '有新日志 · 回到最新', exact: true })).toBeVisible();
  await expect(page.getByText('后续记录', { exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: '继续日志流', exact: true }).click();
  await expect(page.getByText('后续记录', { exact: true })).toBeVisible();
});

test('native live dates, rectangular accents and title actions match the requested log layout', async ({ page, api }) => {
  test.setTimeout(90_000);
  api.setTask({ logs: [
    '2026-11-11 09:56:24,010 - media::source - INFO - 昨日记录',
    '2026-11-12 09:56:24,020 - media::source - INFO - 今日记录',
  ] });
  await page.goto('/dashboard');
  const launcher = page.getByRole('button', { name: '实时日志', exact: true });
  await expect(launcher.locator('svg path')).toHaveAttribute('d', bellPath);
  await launcher.click();
  await expect(page.locator('.notification-log .log-record-time')).toHaveText(['2026/11/11 09:56:24', '2026/11/12 09:56:24']);
  const heading = page.locator('.notification-heading');
  await expect(heading.getByRole('button', { name: '导出实时日志', exact: true })).toBeVisible();
  await expect(heading.getByRole('link', { name: '在新窗口中打开实时日志', exact: true })).toHaveAttribute('href', '/logs');
  await expect(heading.getByRole('link', { name: '在新窗口中打开实时日志', exact: true })).toHaveAttribute('target', '_blank');
  await expect(page.locator('.logging-toolbar').getByRole('button', { name: '暂停日志流', exact: true })).toBeVisible();
  await expect(page.locator('.logging-toolbar').getByRole('button', { name: '导出实时日志', exact: true })).toHaveCount(0);
  for (const accent of await page.locator('.notification-log .log-record-accent').all()) {
    const size = await accent.evaluate(element => {
      const box = element.getBoundingClientRect();
      return { width: box.width, height: box.height, radius: getComputedStyle(element).borderTopLeftRadius };
    });
    expect(size.height).toBeGreaterThan(size.width);
    expect(size.radius).toBe('2px');
  }
  const download = page.waitForEvent('download');
  await heading.getByRole('button', { name: '导出实时日志', exact: true }).click();
  expect((await download).suggestedFilename()).toMatch(/^etk-.*\.log$/);
});

test('the new log window authenticates and continues receiving native live logs', async ({ page, api }) => {
  test.setTimeout(60_000);
  let records = ['2026-11-12 09:56:24,010 - media::source - INFO - 窗口初始记录'];
  const popupErrors = [], popupRequests = [];
  await page.context().route('**/api/**', async route => {
    const path = new URL(route.request().url()).pathname;
    popupRequests.push(path);
    if (path === '/api/auth/check_status') return route.fulfill({ json: { status: 'logged_in', user: { id: 'log-user', name: 'Log User', username: 'Log User', is_admin: true } } });
    if (path === '/api/status') return route.fulfill({ json: { is_running: false, state: 'idle', logs: records } });
    if (path === '/api/portal/account-info') return route.fulfill({ json: { name: 'Log User', status: 'active', profile_image_tag: null } });
    if (path === '/api/system/menu_config') return route.fulfill({ json: [] });
    throw new Error(`日志窗口请求了未定义接口 ${path}`);
  });
  api.setTask({ logs: records });
  await page.goto('/dashboard');
  await page.getByRole('button', { name: '实时日志', exact: true }).click();
  const popupEvent = page.waitForEvent('popup');
  await page.getByRole('link', { name: '在新窗口中打开实时日志', exact: true }).click();
  const popup = await popupEvent;
  popup.on('pageerror', error => popupErrors.push(error.message));
  await expect(popup).toHaveURL(/\/logs$/);
  await expect(popup.locator('.notification-center.is-standalone')).toBeVisible();
  await expect(popup.getByText('窗口初始记录', { exact: true })).toBeVisible();
  await expect(popup.getByRole('button', { name: '关闭实时日志', exact: true })).toHaveCount(0);
  records = [...records, '2026-11-12 09:56:25,010 - media::source - INFO - 窗口后续记录'];
  await expect(popup.getByText('窗口后续记录', { exact: true })).toBeVisible();
  expect(popupRequests).toContain('/api/auth/check_status');
  expect(popupRequests.filter(path => path === '/api/status').length).toBeGreaterThanOrEqual(2);
  expect(popupErrors).toEqual([]);
  await popup.close();
});

test('grouped log timestamps keep their milliseconds and remain readable over a dark glass wallpaper', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.addInitScript(() => {
    const canvas = document.createElement('canvas'); canvas.width = canvas.height = 64;
    const context = canvas.getContext('2d');
    for (let x = 0; x < 64; x += 8) for (let y = 0; y < 64; y += 8) {
      context.fillStyle = (x + y) % 16 ? '#fff' : '#080d14'; context.fillRect(x, y, 8, 8);
    }
    localStorage.setItem('etk-wallpaper-settings', JSON.stringify({ mode: 'custom', customImage: canvas.toDataURL(), rotationInterval: 0 }));
    localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme: 'dark', glassEnabled: true, glassAppearance: 'clear', glassQuality: 'css', glassDynamicsMode: 'off', radius: 'extra' }));
  });
  api.setTask({ logs: ['2026-11-11 12:30:45,001 - media::source - INFO - 第一条', '2026-11-11 12:30:45,040 - media::source - INFO - 第二条'] });
  await page.goto('/dashboard');
  await page.getByRole('button', { name: '实时日志', exact: true }).click();
  const timestamps = page.locator('.notification-log .log-record-inline-time');
  await expect(timestamps).toHaveText(['12:30:45,001', '12:30:45,040']);
  for (const reading of await measureReadingContrast(page, [0, 1].map(index => ({ role: `grouped timestamp ${index}`, locator: timestamps.nth(index) })))) {
    expect(reading.contrast).toBeGreaterThanOrEqual(4.5);
  }
});

for (const preference of [{ theme: 'light', primaryColor: '#FFFFFF' }, { theme: 'dark', primaryColor: '#000000' }]) {
  test(`${preference.theme} extreme primary ${preference.primaryColor} log text and action icons remain readable`, async ({ page, api }) => {
    test.setTimeout(90_000);
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.addInitScript(value => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ ...value, glassEnabled: false })), preference);
    api.setTask({ logs: ['INFO', 'WARNING', 'ERROR'].map((level, index) => `2026-11-12 09:56:2${index},010 - source - ${level} - ${level}完整正文`) });
    await page.goto('/dashboard');
    await page.getByRole('button', { name: '实时日志', exact: true }).click();
    const center = page.locator('.notification-center');
    const primary = preference.theme === 'light' ? '255,255,255' : '0,0,0';
    await expect.poll(() => center.evaluate(element => getComputedStyle(element).getPropertyValue('--v-theme-primary').replaceAll(' ', ''))).toBe(primary);
    const textProbes = ['INFO', 'WARNING', 'ERROR'].flatMap(level => {
      const record = center.locator(`.notification-log .log-record[data-level="${level}"]`);
      return [
        { role: `${level} timestamp`, locator: record.locator('.log-record-time') },
        { role: `${level} label`, locator: record.locator('.log-record-level') },
        { role: `${level} filter`, locator: center.locator('.logging-level-toggle').getByRole('button', { name: level, exact: true }).locator('.v-btn__content') },
      ];
    });
    textProbes.push(
      { role: 'active filter', locator: center.locator('.logging-level-toggle__button--active .v-btn__content') },
      { role: 'active log tab', locator: center.getByRole('tab', { name: '实时日志', exact: true }).locator('.v-btn__content') },
    );
    const iconProbes = [
      { role: 'download icon', locator: center.getByRole('button', { name: '导出实时日志', exact: true }).locator('svg') },
      { role: 'new window icon', locator: center.getByRole('link', { name: '在新窗口中打开实时日志', exact: true }).locator('svg') },
      { role: 'pause icon', locator: center.getByRole('button', { name: '暂停日志流', exact: true }).locator('svg') },
    ];
    const text = await measureReadingContrast(page, textProbes);
    const icons = await measureReadingContrast(page, iconProbes);
    console.log(JSON.stringify({ preference, text, icons }));
    for (const reading of text) expect(reading.contrast, reading.role).toBeGreaterThanOrEqual(4.5);
    for (const reading of icons) expect(reading.contrast, reading.role).toBeGreaterThanOrEqual(3);
  });
}

for (const preference of [
  { theme: 'light', glassEnabled: false, radius: 'extra' },
  { theme: 'dark', glassEnabled: false, radius: 'extra' },
  { theme: 'dark', glassEnabled: true, glassAppearance: 'clear', glassQuality: 'css', radius: 'extra' },
]) {
  for (const width of [390, 1440]) {
    test(`${preference.theme} glass=${preference.glassEnabled} ${width} notification heading count and list fit the rounded viewport`, async ({ page, api }) => {
      test.setTimeout(90_000);
      await page.setViewportSize({ width, height: 844 });
      await page.addInitScript(value => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify(value)), preference);
      api.setTask({ is_running: false, state: 'completed', last_action: '日志验收', message: '读取完成', logs: lines });
      api.respond('GET', '/api/logs/list', ['app.log']);
      api.respond('GET', '/api/logs/view', { file: 'app.log', lines, page: 1, pages: 1, total_lines: 30 });
      await page.goto('/dashboard');
      await page.getByRole('button', { name: '实时日志', exact: true }).click();
      await expect(page.locator('.notification-log .log-record')).toHaveCount(30);
      const heading = page.locator('.notification-heading h2');
      const count = page.locator('.notification-footer > span');
      await expect(count).toContainText('30 条日志');
      expect(await roundedAncestorsContainGlyph(heading)).toEqual([]);
      expect(await roundedAncestorsContainGlyph(count)).toEqual([]);
      const layout = await page.locator('.notification-log').evaluate(region => {
        const viewport = region.getBoundingClientRect();
        const visible = [...region.querySelectorAll('.log-record')].filter(record => {
          const box = record.getBoundingClientRect();
          return box.top >= viewport.top && box.bottom <= viewport.bottom;
        }).length;
        return { height: viewport.height, visible, scrolls: region.scrollHeight > region.clientHeight };
      });
      expect(layout.height).toBeGreaterThan(width > 600 ? 300 : 220);
      expect(layout.visible).toBeGreaterThanOrEqual(4);
      expect(layout.scrolls).toBe(true);
      await page.getByRole('tab', { name: '历史日志', exact: true }).click();
      await expect(page.locator('.history-log-list .log-record')).toHaveCount(30);
      const historyHeight = await page.locator('.history-log-list').evaluate(element => element.clientHeight);
      expect(historyHeight).toBeGreaterThan(width > 600 ? 300 : 220);
    });
  }
}
