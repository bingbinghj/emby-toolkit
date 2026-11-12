import { mkdirSync, writeFileSync } from 'node:fs';
import { test, expect } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';

const evidence = '/root/emby-toolkit-nov11-verification/remember-login';
mkdirSync(evidence, { recursive: true });
test.use({ authState: 'login_required' });

for (const remember of [false, true]) {
  test(`login submits explicit remember=${remember} and never stores credentials`, async ({ page, api }) => {
    await page.goto('/login');
    const checkbox = page.getByRole('checkbox', { name: '保持登录', exact: true });
    await expect(checkbox).toBeChecked();
    await checkbox.setChecked(remember);
    await page.getByLabel('用户名', { exact: true }).fill('UI Test');
    await page.getByLabel('密码', { exact: true }).fill('never-persist-this-password');
    await page.getByRole('button', { name: '登录', exact: true }).click();
    await expect(page).toHaveURL(/\/dashboard$/);
    expect(api.requests.find(request => request.path === '/api/auth/login').body).toEqual({ username: 'UI Test', password: 'never-persist-this-password', remember });
    expect(await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }))).not.toContain('never-persist-this-password');
    await page.reload();
    await expect(page.locator('.dashboard-page')).toBeVisible();
    api.setAuth('login_required');
    await page.goto('/login');
    await expect(page.getByLabel('密码', { exact: true })).toHaveValue('');
  });
}

test('remember checkbox supports keyboard and is disabled during pending authentication', async ({ page, api }) => {
  const pending = api.defer('POST', '/api/auth/login');
  await page.goto('/login');
  const checkbox = page.getByRole('checkbox', { name: '保持登录', exact: true });
  await checkbox.focus();
  await page.keyboard.press('Space');
  await expect(checkbox).not.toBeChecked();
  await page.getByLabel('用户名', { exact: true }).fill('UI Test');
  await page.getByLabel('密码', { exact: true }).fill('fixture');
  await page.getByRole('button', { name: '登录', exact: true }).click();
  await pending.started;
  await expect(checkbox).toBeDisabled();
  pending.release({ user: { id: 'ui-test-user', name: 'UI Test', is_admin: true }, remember: false });
});

for (const [theme, glassEnabled, appearance] of [
  ['light', false, 'clear'], ['dark', false, 'clear'],
  ['light', true, 'clear'], ['dark', true, 'clear'],
  ['dark', true, 'tinted'], ['dark', true, 'frosted'],
]) {
  for (const width of [390, 1440]) {
    test(`remember label is readable and unclipped ${theme}/${glassEnabled}/${appearance}/${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 1000 });
      await page.emulateMedia({ colorScheme: theme });
      await page.addInitScript(({ theme, glassEnabled, appearance }) => {
        localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme, glassEnabled, glassAppearance: appearance, glassQuality: 'css', glassDynamicsMode: 'off' }));
        const canvas = document.createElement('canvas'); canvas.width = canvas.height = 32;
        const ctx = canvas.getContext('2d'); ctx.fillStyle = 'white'; ctx.fillRect(0, 0, 32, 32);
        localStorage.setItem('etk-wallpaper-settings', JSON.stringify({ mode: 'custom', customImage: canvas.toDataURL(), rotationInterval: 0 }));
      }, { theme, glassEnabled, appearance });
      await page.goto('/login');
      const label = page.locator('.native-login-checkbox__label');
      await expect(label).toBeVisible();
      const layout = await label.evaluate(element => {
        const box = element.getBoundingClientRect(), card = element.closest('.login-card').getBoundingClientRect();
        return { fontSize: getComputedStyle(element).fontSize, x: box.x, right: box.right, cardX: card.x, cardRight: card.right, scrollWidth: document.documentElement.scrollWidth, viewport: innerWidth };
      });
      expect(layout.fontSize).toBe('15px');
      expect(layout.x - layout.cardX).toBeGreaterThan(20);
      expect(layout.cardRight - layout.right).toBeGreaterThan(20);
      expect(layout.scrollWidth).toBeLessThanOrEqual(layout.viewport + 1);
      const readings = await measureReadingContrast(page, [{ role: '保持登录', locator: label }]);
      const id = `${theme}-${glassEnabled}-${appearance}-${width}`;
      writeFileSync(`${evidence}/${id}.json`, JSON.stringify({ layout, readings }, null, 2));
      await page.screenshot({ path: `${evidence}/${id}.png` });
      expect(readings[0].contrast).toBeGreaterThanOrEqual(4.5);
    });
  }
}
