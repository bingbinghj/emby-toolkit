import { test, expect } from './fixtures.js';

for (const width of [320, 390, 768, 1280]) {
  test(`history controls and their labels fit at ${width}px`, async ({ page, api }) => {
    await page.setViewportSize({ width, height: 844 });
    api.respond('GET', '/api/logs/list', ['app.log']);
    api.respond('GET', '/api/logs/view', { lines: ['INFO: 第一条日志'], page: 1, pages: 9, total_lines: 4001 });
    await page.goto('/dashboard');
    await page.getByRole('button', { name: '实时日志', exact: true }).click();
    await page.getByRole('tab', { name: '历史日志', exact: true }).click();
    const section = page.getByRole('region', { name: '历史日志', exact: true });
    await expect(section.getByText('app.log', { exact: true })).toBeVisible();
    await section.getByRole('radio').first().focus();
    const clips = await section.evaluate(root => {
      const clipped = [];
      const fields = root.querySelectorAll('.v-field, .v-selection-control__input, .app-responsive-input__label, .v-radio .v-label, .history-pagination .text-caption, .v-pagination button');
      for (const field of fields) {
        if (!field.getClientRects().length) continue;
        const rect = field.getBoundingClientRect();
        // Include space for a visible focus ring and the field outline.
        const gap = field.matches('.v-field, .v-selection-control__input') ? 2 : 0;
        for (const [x, y] of [[rect.left-gap, rect.top-gap], [rect.right+gap, rect.top-gap], [rect.left-gap, rect.bottom+gap], [rect.right+gap, rect.bottom+gap]]) {
          if (x < 0 || x > innerWidth || y < 0 || y > innerHeight) { clipped.push(field.className); break; }
          const hit = document.elementFromPoint(x, y);
          if (!hit || !(root.contains(hit) || hit.contains(root))) { clipped.push(field.className); break; }
        }
      }
      return clipped;
    });
    expect(clips).toEqual([]);
  });
}
