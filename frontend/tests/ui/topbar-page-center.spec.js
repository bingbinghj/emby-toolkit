import { writeFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';

const cases = [
  { width: 2194, height: 1234, dsf: 1.75, layout: 'vertical', glass: false },
  { width: 2194, height: 1234, dsf: 1.75, layout: 'collapsed', glass: false },
  { width: 2194, height: 1234, dsf: 1.75, layout: 'vertical', glass: true },
  { width: 2194, height: 1234, dsf: 1.75, layout: 'collapsed', glass: true },
  { width: 1920, height: 1080, dsf: 2, layout: 'vertical', glass: false },
  { width: 2560, height: 1440, dsf: 1.5, layout: 'vertical', glass: false },
  { width: 3072, height: 1728, dsf: 1.25, layout: 'vertical', glass: false },
  { width: 3840, height: 2160, dsf: 1, layout: 'vertical', glass: false },
  { width: 2194, height: 1234, dsf: 1.75, layout: 'horizontal', glass: false },
];
const task = { is_running: true, state: 'running', task_id: 'page-center-task', current_action: '页面中心测试任务',
  last_action: '页面中心测试任务', progress: 42, progress_known: true, message: '测试页面中心', logs: [], history: [] };
async function measure(page) {
  return page.evaluate(() => {
    const rect = element => { const r = element.getBoundingClientRect(); return { left: r.left, right: r.right, width: r.width, center: r.left + r.width / 2 }; };
    const card = document.querySelector('.running-task'), row = card.closest('.theme-navbar-row');
    const controls = [...row.querySelectorAll('.theme-navbar-leading button,.theme-navbar-leading input,.theme-navbar-leading a,.theme-navbar-actions button')]
      .filter(element => element.getClientRects().length && getComputedStyle(element).visibility !== 'hidden').map(rect);
    return { page: rect(document.querySelector('.v-application')), navbar: rect(card.closest('.layout-navbar')), card: rect(card), controls,
      dpr: devicePixelRatio, innerWidth, clientWidth: document.documentElement.clientWidth, columns: getComputedStyle(row).gridTemplateColumns };
  });
}
function centered(geometry) {
  expect(Math.abs(geometry.card.center - geometry.page.center)).toBeLessThan(1);
  for (const control of geometry.controls) {
    expect(control.right <= geometry.card.left - 4 || control.left >= geometry.card.right + 4).toBe(true);
  }
}
for (const options of cases) {
  const id = `${options.width}-dpr${options.dsf}-${options.layout}-${options.glass ? 'glass' : 'plain'}`;
  test.describe(id, () => {
    test.use({ viewport: { width: options.width, height: options.height }, deviceScaleFactor: options.dsf });
    test('task stays at the page center and outside the edge controls', async ({ page, api }, testInfo) => {
      await page.addInitScript(options => {
        localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme: options.glass ? 'dark' : 'light', layout: options.layout,
          glassEnabled: options.glass, glassQuality: 'css', glassAppearance: 'clear', glassDynamicsMode: 'off' }));
        localStorage.setItem('etk-wallpaper-settings', JSON.stringify({ mode: 'none' }));
      }, options);
      api.setTask(task);
      await page.goto('/dashboard');
      await expect(page.getByRole('button', { name: '查看当前任务', exact: true })).toBeVisible();
      await expect.poll(async () => { const g = await measure(page); return Math.abs(g.card.center - g.page.center); }).toBeLessThan(1);
      const initial = await measure(page);
      expect(initial.dpr).toBe(options.dsf); centered(initial);
      writeFileSync(testInfo.outputPath('page-center.json'), JSON.stringify(initial, null, 2));
      if (options.width === 2194 && options.layout === 'vertical' && !options.glass) {
        await page.getByRole('button', { name: '主题与壁纸', exact: true }).click();
        const panel = page.locator('.theme-customizer-panel-host');
        for (const [label, collapsed] of [['折叠', true], ['垂直', false]]) {
          await panel.getByRole('button', { name: label, exact: true }).click();
          if (collapsed) await expect(page.locator('.etk-default-layout')).toHaveClass(/layout-vertical-nav-collapsed/);
          else await expect(page.locator('.etk-default-layout')).not.toHaveClass(/layout-vertical-nav-collapsed/);
          await expect.poll(async () => { const g = await measure(page); return Math.abs(g.card.center - g.page.center); }).toBeLessThan(1);
          const changed = await measure(page); centered(changed);
          expect(Math.abs(changed.card.center - initial.card.center)).toBeLessThan(1);
          writeFileSync(testInfo.outputPath(`drawer-${label}.json`), JSON.stringify(changed, null, 2));
        }
        await panel.locator('[data-native-dialog-close]').click();
        for (const width of [1920, 1280, 1024, 2194]) {
          await page.setViewportSize({ width, height: width === 2194 ? 1234 : 1080 });
          const shell = page.locator('.etk-default-layout'), drawer = page.locator('.layout-vertical-nav');
          await expect(shell).toHaveAttribute('data-shell-mode', width >= 1280 ? 'desktop' : 'app');
          if (width >= 1280) await expect(drawer).toBeInViewport({ ratio: 1 });
          else await expect(drawer).not.toBeInViewport();
          await expect.poll(async () => { const g = await measure(page); return Math.abs(g.card.center - g.page.center); }).toBeLessThan(1);
          const resized = await measure(page); centered(resized);
          expect(resized.dpr).toBe(options.dsf);
          writeFileSync(testInfo.outputPath(`resize-${width}.json`), JSON.stringify(resized, null, 2));
        }
      }
    });
  });
}
