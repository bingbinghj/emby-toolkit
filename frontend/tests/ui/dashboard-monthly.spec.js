import { mkdirSync, writeFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';

const artifactRoot = '/root/emby-toolkit-nov11-verification/dashboard-monthly';
mkdirSync(artifactRoot, { recursive: true });
const period = (date = new Date()) => `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}`;
const counts = { movie_count: 7, tv_count: 5, episode_count: 122, music_count: 0 };
const statistics = () => ({
  ...Object.fromEntries(Object.entries(counts).map(([key, month_count]) => [key, { count: 800, month_count }])),
  month: { period: period(), state: 'recording', source: 'etk-history', recording_since: '2026-11-10T18:30:57Z', partial: true },
});
const payload = statistics => ({ statistics, recent: [], libraries: [], errors: {} });

test('MP monthly additions show real history numbers and a history failure leaves fresh Emby totals available', async ({ page, api }) => {
  api.respond('GET', '/api/dashboard/media', payload(statistics()));
  await page.goto('/dashboard');
  for (const [key, count] of Object.entries(counts)) await expect(page.getByTestId(`dashboard-${key}-month`)).toHaveText(`本月新增 ${count}`);
  await expect(page.getByTestId('dashboard-movie_count-month')).toHaveAttribute('title', /按 ETK 成功入库记录统计.*记录始于/);
  const next = { ...statistics(), movie_count: { count: 801, month_count: null }, month: { period: period(), state: 'unavailable', source: 'etk-history', error: '入库记录数据库暂不可用' } };
  api.respond('GET', '/api/dashboard/media', payload(next));
  await page.reload();
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('801');
  for (const key of Object.keys(counts)) await expect(page.getByTestId(`dashboard-${key}-month`)).toHaveText('本月新增 —');
  await expect(page.locator('[data-dashboard-module="statistics"]')).toContainText('本月入库统计读取失败：入库记录数据库暂不可用');
  await expect(page.getByRole('button', { name: '重试媒体统计', exact: true })).toBeVisible();
});

for (const cache of ['legacy', 'previous-month']) test(`${cache} snapshot cannot invent a monthly zero or relabel an old month`, async ({ page, api }) => {
  const value = statistics();
  if (cache === 'legacy') {
    delete value.month;
    for (const key of Object.keys(counts)) delete value[key].month_count;
  } else value.month.period = `${new Date().getFullYear() - 1}-${String(new Date().getMonth() + 1).padStart(2, '0')}`;
  await page.addInitScript(value => localStorage.setItem('ETK_DASHBOARD_SNAPSHOT_V1:ui-test-user:statistics', JSON.stringify({ savedAt: Date.now(), value })), value);
  api.respond('GET', '/api/dashboard/media', { error: '暂不可用' }, 503);
  await page.goto('/dashboard');
  await expect(page.getByTestId('dashboard-movie_count')).toHaveText('800');
  for (const key of Object.keys(counts)) await expect(page.getByTestId(`dashboard-${key}-month`)).toHaveText('本月新增 —');
});

const themes = [
  { name: 'plain-light', theme: 'light', glassEnabled: false },
  { name: 'plain-dark', theme: 'dark', glassEnabled: false },
  ...['light', 'dark'].flatMap(theme => ['clear', 'tinted', 'frosted'].flatMap(glassAppearance => ['adaptive', 'clear'].map(glassUIStyle => ({
    name: `${theme}-${glassAppearance}-${glassUIStyle}`, theme, glassEnabled: true, glassAppearance, glassUIStyle,
  })))),
];

for (const width of [390, 1440]) for (const theme of themes) test(`${width}-${theme.name} monthly captions and enabled module labels are readable and clear of rounded clipping`, async ({ page, api }) => {
  const id = `${width}-${theme.name}`;
  await page.setViewportSize({ width, height: 1000 });
  await page.emulateMedia({ colorScheme: theme.theme, reducedMotion: 'reduce' });
  await page.addInitScript(theme => {
    const canvas = document.createElement('canvas'); canvas.width = canvas.height = 64;
    const context = canvas.getContext('2d');
    for (let x = 0; x < 64; x += 8) for (let y = 0; y < 64; y += 8) {
      context.fillStyle = (x + y) % 16 ? '#ffffff' : '#080d14'; context.fillRect(x, y, 8, 8);
    }
    localStorage.setItem('etk-wallpaper-settings', JSON.stringify({ mode: 'custom', customImage: canvas.toDataURL(), rotationInterval: 0 }));
    localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ ...theme, glassQuality: 'css', glassDynamicsMode: 'off', radius: 'extra', layout: 'vertical' }));
  }, theme);
  const values = statistics();
  values.episode_count.month_count = 1234567890;
  api.respond('GET', '/api/dashboard/media', payload(values));
  await page.goto('/dashboard');
  const captions = page.locator('.dashboard-stat-addition');
  await expect(captions).toHaveCount(4);
  const geometry = await captions.evaluateAll(elements => elements.map(element => {
    const box = element.getBoundingClientRect(), card = element.closest('.dashboard-card').getBoundingClientRect();
    const css = getComputedStyle(element);
    return { top: box.top - card.top, bottom: card.bottom - box.bottom, left: box.left - card.left, right: card.right - box.right,
      height: box.height, lineHeight: parseFloat(css.lineHeight), clientHeight: element.clientHeight, scrollHeight: element.scrollHeight, fontSize: css.fontSize };
  }));
  for (const item of geometry) {
    for (const edge of ['top', 'bottom', 'left', 'right']) expect(item[edge], `${id}: ${edge} padding protects all caption text from card clipping`).toBeGreaterThanOrEqual(8);
    expect(item.clientHeight).toBeGreaterThanOrEqual(item.scrollHeight);
    expect(item.height / item.lineHeight).toBeCloseTo(Math.round(item.height / item.lineHeight), 1);
    expect(item.fontSize).toBe('11.52px');
  }
  const captionReadings = await measureReadingContrast(page, Object.keys(counts).map(key => ({ role: `${key} monthly addition`, locator: page.getByTestId(`dashboard-${key}-month`) })));
  await page.screenshot({ path: `${artifactRoot}/${id}-captions.png`, animations: 'disabled' });
  await page.getByRole('button', { name: '调整布局', exact: true }).hover();
  await page.getByRole('button', { name: '仪表盘设置', exact: true }).click();
  const enabled = page.getByRole('dialog').locator('.dashboard-setting-item.enabled .dashboard-setting-label');
  await expect(enabled).toHaveCount(4);
  const labelReadings = await measureReadingContrast(page, ['存储空间', '媒体统计', '最新入库 - Emby', '我的媒体库'].map(title => ({ role: `enabled ${title}`, locator: enabled.filter({ hasText: title }) })));
  writeFileSync(`${artifactRoot}/${id}.json`, JSON.stringify({ geometry, readings: [...captionReadings, ...labelReadings] }, null, 2));
  await page.screenshot({ path: `${artifactRoot}/${id}-settings.png`, animations: 'disabled' });
  for (const reading of [...captionReadings, ...labelReadings]) expect(reading.contrast, `${id}: ${reading.role}`).toBeGreaterThanOrEqual(4.5);
});
