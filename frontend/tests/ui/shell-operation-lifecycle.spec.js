import { test, expect } from './fixtures.js';

const menuPath = '/api/system/menu_config';
async function openMenu(page) {
  await page.getByRole('button', { name: 'UI Test 的用户菜单', exact: true }).click();
  await page.getByText('自定义导航', { exact: true }).click();
  const dialog = page.locator('.v-dialog.v-overlay--active').filter({ hasText: '自定义导航' });
  await expect(dialog.getByRole('button', { name: '保存', exact: true })).toBeEnabled();
  return dialog;
}
const dashboardName = dialog => dialog.locator('[data-menu-key="Dashboard"]').getByLabel('菜单名称', { exact: true });
async function finishMenuSave(page, dialog, release, result) {
  const response = page.waitForResponse(value => new URL(value.url()).pathname === menuPath && value.request().method() === 'POST');
  release(result); await response;
  await expect(dialog.getByRole('button', { name: '保存', exact: true })).not.toHaveClass(/v-btn--loading/);
}

test('navigation keeps edits entered after its submitted snapshot', async ({ page, api }) => {
  const pending = api.defer('POST', menuPath);
  await page.goto('/dashboard');
  const dialog = await openMenu(page);
  await dashboardName(dialog).fill('提交时的名称');
  await dialog.getByRole('button', { name: '保存', exact: true }).click();
  const submitted = await pending.started;
  await dashboardName(dialog).fill('尚未保存的新名称');
  await finishMenuSave(page, dialog, pending.release, { message: '导航保存完成' });
  await expect(dialog).toBeVisible();
  await expect(dashboardName(dialog)).toHaveValue('尚未保存的新名称');
  await expect(page.getByText('提交时的导航配置已保存，当前修改尚未保存。', { exact: true })).toBeVisible();
  expect(submitted.body.flatMap(item => item.children || [item]).find(item => item.key === 'Dashboard').label).toBe('提交时的名称');
  expect(api.requests.filter(request => request.method === 'POST' && request.path === menuPath)).toHaveLength(1);
});

for (const outcome of ['success', 'failure']) {
  test(`old navigation ${outcome} does not affect a reopened unchanged editor`, async ({ page, api }) => {
    let received, release;
    const started = new Promise(resolve => { received = resolve; });
    const completion = new Promise(resolve => { release = resolve; });
    api.respond('POST', menuPath, async request => { received(request); return completion; }, outcome === 'success' ? 200 : 500);
    await page.goto('/dashboard');
    const dialog = await openMenu(page);
    await dashboardName(dialog).fill('同一提交名称');
    await dialog.getByRole('button', { name: '保存', exact: true }).click();
    await started;
    await dialog.getByRole('button', { name: '关闭导航设置', exact: true }).click();
    await expect(dialog).toBeHidden();
    const reopened = await openMenu(page);
    await finishMenuSave(page, reopened, release, outcome === 'success' ? { message: '旧导航保存完成' } : { error: '旧导航保存失败' });
    await expect(reopened).toBeVisible();
    await expect(dashboardName(reopened)).toHaveValue('同一提交名称');
    await expect(page.getByText(outcome === 'success' ? '菜单配置已保存，全局生效' : '旧导航保存失败', { exact: true })).not.toBeVisible();
    expect(api.requests.filter(request => request.method === 'POST' && request.path === menuPath)).toHaveLength(1);
  });
}

test('a settings search awaiting real navigation cannot close another page search', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => {
    localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme: 'light', glassEnabled: false }));
    localStorage.setItem('etk-wallpaper-settings', JSON.stringify({ mode: 'none' }));
  });
  await page.goto('/settings/general');
  await page.getByRole('button', { name: '打开搜索设置项', exact: true }).click();
  const search = page.locator('.v-dialog.v-overlay--active').filter({ has: page.locator('.navbar-search-mobile') });
  await search.getByRole('searchbox', { name: '搜索设置项', exact: true }).fill('媒体元数据目录');
  // Settings search really returns router.push's Promise. Retain that router
  // and component context while a navigation guard delays the first result.
  await page.evaluate(() => {
    const router = document.getElementById('app').__vue_app__.config.globalProperties.$router;
    let first = true, started;
    window.__shellNavigationStarted = new Promise(resolve => { started = resolve; });
    const hold = new Promise(resolve => { window.__releaseShellNavigation = resolve; });
    window.__removeShellGuard = router.beforeEach(to => {
      if (first && to.name === 'settings-general' && to.query.tab === 'directory') {
        first = false; started(); return hold;
      }
      return true;
    });
    const push = router.push.bind(router);
    router.push = to => {
      const result = push(to);
      if (to.name === 'settings-general' && to.query?.tab === 'directory') window.__shellOldNavigation = result;
      return result;
    };
  });
  await search.getByRole('button', { name: '提交搜索', exact: true }).click();
  await page.evaluate(() => window.__shellNavigationStarted);
  await search.getByRole('button', { name: '关闭搜索', exact: true }).click();
  await page.evaluate(async () => {
    await document.getElementById('app').__vue_app__.config.globalProperties.$router.push('/review');
  });
  await expect(page).toHaveURL(/\/review$/);
  await page.getByRole('button', { name: '打开搜索全部媒体', exact: true }).click();
  const nextSearch = page.locator('.v-dialog.v-overlay--active').filter({ has: page.locator('.navbar-search-mobile') });
  await nextSearch.getByRole('searchbox', { name: '搜索全部媒体', exact: true }).fill('新页面查询');
  await page.evaluate(async () => {
    window.__releaseShellNavigation(true);
    await window.__shellOldNavigation;
    await new Promise(resolve => setTimeout(resolve, 0));
    window.__removeShellGuard();
  });
  await expect(page).toHaveURL(/\/review$/);
  await expect(nextSearch).toBeVisible();
  await expect(nextSearch.getByRole('searchbox', { name: '搜索全部媒体', exact: true })).toHaveValue('新页面查询');
});
