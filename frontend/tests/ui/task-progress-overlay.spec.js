import { writeFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';

for (const options of [
  { width: 2194, height: 1234, dpr: 1.75, glass: false },
  { width: 2194, height: 1234, dpr: 1.75, glass: true },
  { width: 390, height: 844, dpr: 1, glass: false },
]) {
  test.describe(`${options.width}px ${options.glass ? 'glass' : 'plain'} media-task overlay`, () => {
    test.use({ viewport: { width: options.width, height: options.height }, deviceScaleFactor: options.dpr });
    test('save keeps its editor open and the global topbar task card above the dialog through verification', async ({ page, api }, testInfo) => {
      await page.addInitScript(options => {
        localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme: options.glass ? 'dark' : 'light',
          glassEnabled: options.glass, glassQuality: 'css', glassDynamicsMode: 'off' }));
        localStorage.setItem('etk-wallpaper-settings', JSON.stringify({ mode: 'none' }));
      }, options);
      const taskId = 'media-save-visible-through-verification';
      const savePath = '/api/media_info/edit/101';
      const running = { task_id: taskId, is_running: true, state: 'running', progress: 25,
        progress_known: true, current_action: '保存媒体信息', message: '正在核对原章节与缩略图', history: [], logs: [] };
      api.respond('POST', savePath, () => {
        api.setTask(running);
        return { task_id: taskId, message: '已提交保存' };
      }, 202);
      const pending = api.defer('GET', `/api/tasks/result/${taskId}`);
      await page.goto('/edit-media/101');
      await page.getByTestId('media-info-open').click();
      const editor = page.locator('.v-dialog.v-overlay--active').filter({ has: page.getByTestId('media-info-save') });
      const title = editor.getByRole('textbox', { name: /音轨.*1.*当前标题/ });
      await title.fill('本次保存的新标题');
      await editor.getByTestId('media-info-save').click();
      await pending.started;
      const card = page.getByRole('button', { name: '查看当前任务', exact: true });
      await expect(card).toBeVisible();
      await expect(editor).toBeVisible();
      const geometry = await card.evaluate(element => {
        const r = element.getBoundingClientRect();
        const pointerEvents = element.style.pointerEvents;
        element.style.pointerEvents = 'auto';
        const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        element.style.pointerEvents = pointerEvents;
        return { left: r.left, right: r.right, top: r.top, width: r.width, pageWidth: innerWidth,
          isTop: element === top || element.contains(top), topElement: top?.className,
          dialogs: [...document.querySelectorAll('.v-dialog.v-overlay--active')].map(node => getComputedStyle(node).zIndex) };
      });
      writeFileSync(testInfo.outputPath('save-task-card.json'), JSON.stringify(geometry, null, 2));
      await page.screenshot({ path: testInfo.outputPath('save-task-card.png') });
      expect(geometry.isTop, 'the dialog scrim must not cover the topbar task progress').toBe(true);
      expect(Math.abs((geometry.left + geometry.right) / 2 - geometry.pageWidth / 2)).toBeLessThan(1);
      api.setTask({ ...running, progress: 91, phase: '正在验收章节和原图' });
      await expect(card.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '91');
      await expect(editor).toBeVisible();
      const result = { message: '媒体信息回读验收完成', metadata_refresh: { state: 'verified' }, warnings: 0 };
      const finished = { ...running, is_running: false, state: 'completed', progress: 100,
        result_status: 200, result };
      api.setTask(finished); pending.release(finished);
      await expect(page.getByText(result.message, { exact: true })).toBeVisible();
      await expect(editor).toBeVisible();
      await expect(title).toHaveValue('本次保存的新标题');
      await expect(editor.getByTestId('media-info-save')).not.toHaveClass(/v-btn--loading/);
      await expect(card).toHaveCount(0);
      expect(api.requests.filter(request => request.method === 'POST' && request.path === savePath)).toHaveLength(1);
    });
  });
}
