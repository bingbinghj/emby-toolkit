import { test, expect } from './fixtures.js';

const task = { task_id: 'topbar-card-task', is_running: true, state: 'running',
  current_action: '保存媒体信息并回读章节', progress: 42, progress_known: true,
  message: '核对原图与章节', logs: [] };

for (const glassEnabled of [false, true]) {
  for (const width of [390, 1440]) {
    test(`${width}px ${glassEnabled ? 'glass' : 'plain'} topbar keeps a visible task card centered between its controls`, async ({ page, api }, testInfo) => {
      await page.setViewportSize({ width, height: 900 });
      await page.addInitScript(glassEnabled => {
        localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({
          theme: glassEnabled ? 'dark' : 'light', glassEnabled, glassAppearance: 'clear',
          primaryColor: '#8D51F9', glassQuality: 'css', glassDynamicsMode: 'off', layout: 'vertical',
        }));
      }, glassEnabled);
      api.setTask(task);
      await page.goto('/dashboard');
      const current = page.getByRole('button', { name: '查看当前任务', exact: true });
      await expect(current).toBeVisible();
      await expect(current).toContainText(task.current_action);
      await expect(current.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '42');
      const geometry = await current.evaluate(element => {
        const rect = node => {
          const { left, right, top, bottom, width, height } = node.getBoundingClientRect();
          return { left, right, top, bottom, width, height };
        };
        const row = element.closest('.theme-navbar-row');
        const style = getComputedStyle(element);
        return { card: rect(element), row: rect(row), center: rect(row.querySelector('.theme-navbar-center')),
          leading: rect(row.querySelector('.theme-navbar-leading')), actions: rect(row.querySelector('.theme-navbar-actions')),
          background: style.backgroundColor, borderStyle: style.borderTopStyle, borderWidth: parseFloat(style.borderTopWidth) };
      });
      const middle = (rect, axis) => axis === 'x' ? (rect.left + rect.right) / 2 : (rect.top + rect.bottom) / 2;
      expect(Math.abs(middle(geometry.card, 'x') - middle(geometry.center, 'x'))).toBeLessThanOrEqual(2);
      expect(Math.abs(middle(geometry.card, 'y') - middle(geometry.row, 'y'))).toBeLessThanOrEqual(2);
      if (width >= 1280) expect(Math.abs(middle(geometry.card, 'x') - middle(geometry.row, 'x'))).toBeLessThanOrEqual(2);
      expect(geometry.card.left).toBeGreaterThanOrEqual(geometry.leading.right + 4);
      expect(geometry.card.right).toBeLessThanOrEqual(geometry.actions.left - 4);
      expect(geometry.card.height).toBeGreaterThanOrEqual(44);
      expect(geometry.background).not.toBe('rgba(0, 0, 0, 0)');
      expect(geometry.borderStyle).toBe('solid');
      expect(geometry.borderWidth).toBeGreaterThanOrEqual(1);
      await page.locator('.layout-navbar').screenshot({ path: testInfo.outputPath('topbar-card.png') });
      await current.click();
      await expect(page.getByText(task.message, { exact: true })).toBeVisible();
      await current.click();
      api.setTask({ ...task, progress: 0, progress_known: false });
      await expect(current.getByRole('progressbar')).not.toHaveAttribute('aria-valuenow');
      await expect(current.locator('.v-progress-linear__indeterminate.long')).toBeVisible();
      await expect(current.locator('.v-progress-linear__indeterminate.short')).toBeVisible();
      api.setTask({ ...task, is_running: false, state: 'completed' });
      await expect(current).toHaveCount(0);
    });
  }
}
