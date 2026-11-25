import { test, expect } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';
import { readFileSync } from 'node:fs';

const defaults = JSON.parse(readFileSync(new URL('../../../backend/assets/config_defaults.json', import.meta.url)));

for (const theme of ['light', 'dark']) for (const width of [390, 1440]) {
  test(`MP 音乐设置多选顺序、上限、保存与可读性 ${theme} ${width}px`, async ({ page, api }, testInfo) => {
    test.setTimeout(60000);
    const config = structuredClone(defaults);
    api.respond('GET', '/api/config', () => config);
    api.respond('POST', '/api/config', request => {
      Object.assign(config, request.body);
      return { message: '配置已保存', saved_fields: request.body };
    });
    await page.setViewportSize({ width, height: 900 });
    await page.addInitScript(theme => localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme, glassEnabled: true, glassAppearance: 'frosted', glassQuality: 'css' })), theme);
    await page.goto('/settings/general?tab=music');
    const card = page.locator('#music-settings');
    const rename = card.locator('[data-field="music_rename_on_save"]');
    await expect(rename.getByRole('checkbox')).not.toBeChecked();
    await rename.getByRole('checkbox').check();
    await expect(rename).toContainText('之前已修改标签的歌曲也会补做同步');
    const searchType = card.locator('[data-field="music_search_type"]');
    await expect(searchType).toContainText('专辑');
    await searchType.getByRole('combobox').click();
    await page.getByRole('option', { name: '单曲', exact: true }).click();
    const regions = card.locator('[data-field="music_release_region_priority"]');
    const scripts = card.locator('[data-field="music_release_script_priority"]');
    await regions.locator('input').press('ArrowDown');
    await expect(page.getByRole('option')).toHaveCount(8);
    await page.getByRole('option', { name: '中国台湾', exact: true }).click();
    await page.getByRole('option', { name: '韩国', exact: true }).click();
    await page.getByRole('option', { name: '日本', exact: true }).click();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('option')).toHaveCount(0);
    await expect(regions.locator('.v-chip')).toHaveCount(3);
    await expect(regions).toContainText('韩国');
    await expect(regions.locator('.v-chip')).toHaveText(['中国大陆', '中国香港', '韩国']);
    await scripts.locator('input').press('ArrowDown');
    await expect(page.getByRole('option')).toHaveCount(5);
    await page.getByRole('option', { name: '拉丁文字', exact: true }).click();
    await page.getByRole('option', { name: '韩文', exact: true }).click();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('option')).toHaveCount(0);
    const match = card.locator('[data-field="music_track_match"]');
    await match.getByRole('combobox').click();
    await expect(page.getByRole('option')).toHaveCount(3);
    await page.getByRole('option', { name: '严格校验失败时回落忽略时长', exact: true }).click();
    await expect(card).toContainText('已有标签设为“仅缺失”时仍保留原值');
    for (const field of [searchType, regions, scripts, match]) {
      await field.evaluate(node => node.scrollIntoView({ block: 'center' }));
      const text = field.locator('.v-select__selection-text, .v-chip__content').first();
      const readings = await measureReadingContrast(page, [{ role: await field.getAttribute('data-field'), locator: text }]);
      console.log(JSON.stringify(readings));
      for (const reading of readings) expect(reading.contrast).toBeGreaterThanOrEqual(4.5);
      const bounds = await field.evaluate(node => ({ width: node.clientWidth, scrollWidth: node.scrollWidth }));
      expect(bounds.scrollWidth).toBeLessThanOrEqual(bounds.width + 1);
      const textSize = await text.evaluate(node => ({ width: node.clientWidth, scrollWidth: node.scrollWidth, height: node.clientHeight, scrollHeight: node.scrollHeight }));
      expect(textSize.scrollWidth).toBeLessThanOrEqual(textSize.width + 1);
      expect(textSize.scrollHeight).toBeLessThanOrEqual(textSize.height + 1);
    }
    await page.screenshot({ path: testInfo.outputPath('music-settings.png') });
    await card.getByRole('button', { name: '保存', exact: true }).click();
    await expect.poll(() => api.requests.filter(row => row.method === 'POST' && row.path === '/api/config').length).toBe(1);
    const saved = api.requests.find(row => row.method === 'POST' && row.path === '/api/config').body;
    expect(saved.music_release_region_priority).toBe('CN,HK,KR');
    expect(saved.music_release_script_priority).toBe('Hans,Hant,Kore');
    expect(saved.music_track_match).toBe('fallback_ignore_duration');
    expect(saved.music_search_type).toBe('recording');
    expect(saved.music_rename_on_save).toBe(true);
    expect(saved).not.toHaveProperty('emby_server_url');
    await page.reload();
    await expect(regions.locator('.v-chip')).toHaveText(['中国大陆', '中国香港', '韩国']);
    await expect(match).toContainText('严格校验失败时回落忽略时长');
    await expect(searchType).toContainText('单曲');
    await expect(rename.getByRole('checkbox')).toBeChecked();
    await rename.scrollIntoViewIfNeeded();
    for (const selector of ['.v-label, .app-responsive-input__label', '.v-messages__message, .app-responsive-input__hint']) {
      const text = rename.locator(selector).filter({ visible: true }).first();
      const bounds = await text.evaluate(node => ({ width: node.clientWidth, scrollWidth: node.scrollWidth, height: node.clientHeight, scrollHeight: node.scrollHeight }));
      expect(bounds.scrollWidth).toBeLessThanOrEqual(bounds.width + 1);
      expect(bounds.scrollHeight).toBeLessThanOrEqual(bounds.height + 1);
      for (const reading of await measureReadingContrast(page, [{ role: '同步重命名设置', locator: text }])) expect(reading.contrast).toBeGreaterThanOrEqual(4.5);
    }
    await page.screenshot({ path: testInfo.outputPath('music-rename-setting.png') });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBeTruthy();
  });
}
