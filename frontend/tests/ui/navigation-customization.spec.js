import { test, expect } from './fixtures.js';

test.describe.configure({ timeout: 120_000 });

async function openEditor(page) {
  await page.getByRole('button', { name: 'UI Test 的用户菜单', exact: true }).click();
  await page.getByText('自定义导航', { exact: true }).click();
  return page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '保存', exact: true }) });
}

async function recordNativeDrag(editor) {
  await editor.evaluate(element => {
    window.navigationDragEvents = [];
    for (const type of ['dragstart', 'drop', 'dragend']) element.addEventListener(type, event => {
      window.navigationDragEvents.push({ type, source: event.target.closest('[data-menu-key]')?.dataset.menuKey,
        target: event.target.closest('[data-before-key]')?.dataset.beforeKey, transferredKey: event.dataTransfer?.getData('text/plain') });
    }, true);
  });
}

test('sidebar sections use MP dividers and directly visible navigation items', async ({ page }) => {
  await page.goto('/dashboard');
  const drawer = page.locator('.v-navigation-drawer');
  await expect(drawer.getByRole('heading')).toHaveText(['发现', '整理', '系统']);
  await expect(drawer.locator('.v-list-group')).toHaveCount(0);
  await expect(drawer.getByText('设置', { exact: true })).toBeVisible();
  await drawer.getByText('设置', { exact: true }).click();
  await expect(page).toHaveURL(/\/settings\/general$/);
  await expect(drawer.getByRole('heading')).toHaveText(['发现', '整理', '系统']);
  await expect(drawer.getByText('媒体整理', { exact: true })).toBeVisible();
  const divider = await drawer.locator('.nav-section-title .title-text').first().evaluate(element =>
    getComputedStyle(element, '::after').borderBottomWidth);
  expect(divider).toBe('1px');
});

test('custom section titles survive saving and reloading with their items directly visible', async ({ page }) => {
  await page.goto('/dashboard');
  const editor = await openEditor(page);
  await editor.locator('[data-menu-key="group-system"]').getByLabel('分区名称', { exact: true }).fill('系统工具');
  await editor.getByRole('button', { name: '保存', exact: true }).click();
  await page.reload();
  const drawer = page.locator('.v-navigation-drawer');
  await expect(drawer.getByRole('heading', { name: '系统工具', exact: true })).toBeVisible();
  await expect(drawer.getByText('用户管理', { exact: true })).toBeVisible();
  await expect(drawer.locator('.v-list-group')).toHaveCount(0);
});

test('legacy collapsed configuration cannot hide flat section items', async ({ page, api }) => {
  api.respond('GET', '/api/system/menu_config', [{ key: 'group-system', label: '系统', expanded: false }]);
  await page.goto('/dashboard');
  const drawer = page.locator('.v-navigation-drawer');
  await expect(drawer.getByText('用户管理', { exact: true })).toBeVisible();
  const editor = await openEditor(page);
  await expect(editor.getByLabel('默认展开', { exact: true })).toHaveCount(0);
  await editor.getByRole('button', { name: '保存', exact: true }).click();
  const saved = api.requests.find(request => request.method === 'POST' && request.path === '/api/system/menu_config').body;
  expect(saved.find(item => item.key === 'group-system').expanded).toBe(false);
  await page.reload();
  await expect(drawer.getByText('用户管理', { exact: true })).toBeVisible();
  await expect(drawer.locator('.v-list-group')).toHaveCount(0);
});

test('a standalone menu keeps its name, visibility and order and can return to a group', async ({ page, api }) => {
  await page.goto('/dashboard');
  let editor = await openEditor(page);
  const task = editor.locator('.menu-editor-item[data-menu-key="settings-scheduler"]');
  await task.getByRole('button', { name: '将 任务中心 移出分区', exact: true }).click();
  let standalone = editor.locator('.menu-editor-standalone[data-menu-key="settings-scheduler"]');
  await standalone.getByLabel('菜单名称', { exact: true }).fill('我的任务');
  await standalone.getByLabel('显示 任务中心', { exact: true }).uncheck();
  await standalone.getByRole('button', { name: '上移导航', exact: true }).click();
  await editor.getByRole('button', { name: '保存', exact: true }).click();
  const saved = api.requests.find(request => request.method === 'POST' && request.path === '/api/system/menu_config').body;
  expect(saved.map(item => item.key)).toEqual(['group-discovery', 'group-management', 'settings-scheduler', 'group-system']);
  expect(saved[2]).toMatchObject({ key: 'settings-scheduler', label: '我的任务', visible: false });
  expect(saved.flatMap(item => item.children || [item]).filter(item => item.key === 'settings-scheduler')).toHaveLength(1);
  await page.reload();
  await expect(page.locator('.v-navigation-drawer').getByText('我的任务', { exact: true })).toHaveCount(0);
  editor = await openEditor(page);
  standalone = editor.locator('.menu-editor-standalone[data-menu-key="settings-scheduler"]');
  await expect(standalone.getByLabel('菜单名称', { exact: true })).toHaveValue('我的任务');
  await expect(standalone.getByLabel('显示 任务中心', { exact: true })).not.toBeChecked();
  await standalone.getByLabel('显示 任务中心', { exact: true }).check();
  await standalone.getByRole('button', { name: '将 我的任务 移入分区', exact: true }).click();
  await page.locator('.v-menu .v-list-item').filter({ hasText: '整理' }).click();
  await expect(editor.locator('.menu-editor-standalone[data-menu-key="settings-scheduler"]')).toHaveCount(0);
  await expect(editor.locator('[data-menu-key="group-management"] .menu-editor-item[data-menu-key="settings-scheduler"]')).toHaveCount(1);
  await editor.getByRole('button', { name: '保存', exact: true }).click();
  await page.reload();
  await expect(page.locator('.v-navigation-drawer').getByText('我的任务', { exact: true })).toBeVisible();
});

test('native dragging moves a child out to the root then back into another group', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 1400 });
  await page.goto('/dashboard');
  const editor = await openEditor(page);
  await recordNativeDrag(editor);
  await editor.locator('.menu-editor-item[data-menu-key="MediaOrganize"] .menu-drag-handle')
    .dragTo(editor.locator('.menu-editor-root-dropzone[data-before-key="group-management"]'));
  await test.info().attach('native-promotion-events', { body: JSON.stringify(await page.evaluate(() => window.navigationDragEvents)), contentType: 'application/json' });
  const standalone = editor.locator('.menu-editor-standalone[data-menu-key="MediaOrganize"]');
  await expect(standalone).toHaveCount(1);
  await standalone.locator('.menu-drag-handle').dragTo(editor.locator('.menu-editor-item[data-menu-key="Dashboard"]'), { targetPosition: { x: 32, y: 8 } });
  await expect(editor.locator('.menu-editor-standalone[data-menu-key="MediaOrganize"]')).toHaveCount(0);
  await expect(editor.locator('[data-menu-key="group-discovery"] .menu-editor-item')).toHaveCount(2);
  await editor.getByRole('button', { name: '保存', exact: true }).click();
  const saved = api.requests.find(request => request.method === 'POST' && request.path === '/api/system/menu_config').body;
  expect(saved.find(item => item.key === 'group-discovery').children.map(item => item.key)).toEqual(['MediaOrganize', 'Dashboard']);
  await page.reload();
  const drawer = page.locator('.v-navigation-drawer');
  await expect(drawer.getByText('媒体整理', { exact: true })).toHaveCount(1);
  await expect(drawer.getByText('媒体整理', { exact: true })).toBeVisible();
});

test('native dragging reorders root groups and keeps the order after reload', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 1400 });
  await page.goto('/dashboard');
  const editor = await openEditor(page);
  await recordNativeDrag(editor);
  await editor.locator('[data-menu-key="group-system"] .menu-editor-group-heading > .menu-drag-handle')
    .dragTo(editor.locator('.menu-editor-root-dropzone[data-before-key="group-discovery"]'));
  await test.info().attach('native-root-order-events', { body: JSON.stringify(await page.evaluate(() => window.navigationDragEvents)), contentType: 'application/json' });
  await expect(editor.locator('.menu-editor-node').first()).toHaveAttribute('data-menu-key', 'group-system');
  await editor.getByRole('button', { name: '保存', exact: true }).click();
  const saved = api.requests.find(request => request.method === 'POST' && request.path === '/api/system/menu_config').body;
  expect(saved.map(item => item.key)).toEqual(['group-system', 'group-discovery', 'group-management']);
  await page.reload();
  await expect(page.locator('.v-navigation-drawer .nav-section-title').first()).toHaveText('系统');
});

test('the sidebar material fills its content height and user management follows cover generation', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 1080 });
  await page.goto('/dashboard');
  const drawer = page.locator('.v-navigation-drawer');
  await expect(drawer.getByText('用户管理', { exact: true })).toBeVisible();
  const labels = await drawer.locator('.v-list-item-title').allTextContents();
  expect(labels.at(-1)).toBe('用户管理');
  expect(labels.indexOf('用户管理')).toBeGreaterThan(labels.indexOf('封面生成'));
  const geometry = await drawer.evaluate(element => {
    const content = element.querySelector('.v-navigation-drawer__content').getBoundingClientRect();
    const items = element.querySelector('.nav-items').getBoundingClientRect();
    return { gap: content.bottom - items.bottom, widthGap: content.width - items.width, position: getComputedStyle(element).position };
  });
  expect(geometry.gap).toBeLessThanOrEqual(1);
  expect(geometry.widthGap).toBeLessThanOrEqual(1);
  expect(geometry.position).toBe('fixed');
});

test('mobile navigation editing fits the viewport and supports promotion without drag gestures', async ({ page, api }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/dashboard');
  const editor = await openEditor(page);
  await editor.locator('.menu-editor-item[data-menu-key="MediaOrganize"]').getByRole('button', { name: '将 媒体整理 移出分区', exact: true }).click();
  await expect(editor.locator('.menu-editor-standalone[data-menu-key="MediaOrganize"]')).toHaveCount(1);
  const overflow = await editor.evaluate(element => {
    const card = element.querySelector('.v-card'), box = card.getBoundingClientRect();
    return { width: box.width, left: box.left, right: box.right, overflow: card.scrollWidth - card.clientWidth };
  });
  expect(overflow.left).toBeGreaterThanOrEqual(0);
  expect(overflow.right).toBeLessThanOrEqual(390);
  expect(overflow.overflow).toBeLessThanOrEqual(1);
  await editor.getByRole('button', { name: '保存', exact: true }).click();
  const saved = api.requests.find(request => request.method === 'POST' && request.path === '/api/system/menu_config').body;
  expect(saved.filter(item => item.key === 'MediaOrganize')).toHaveLength(1);
});

test('native navigation dragging continuously scrolls the dialog at either edge and stops away from it', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 650 });
  await page.goto('/dashboard');
  const editor = await openEditor(page);
  const content = editor.locator('.menu-editor-content');
  const handle = editor.locator('[data-menu-key="Dashboard"] .menu-drag-handle');
  await handle.scrollIntoViewIfNeeded();
  const source = await handle.boundingBox(), bounds = await content.boundingBox();
  await page.mouse.move(source.x + source.width / 2, source.y + source.height / 2);
  await page.mouse.down();
  await page.mouse.move(source.x + source.width / 2 + 20, source.y + source.height / 2 + 20, { steps: 5 });
  await expect(editor.locator('.menu-editor-roots--dragging')).toBeVisible();
  const x = bounds.x + bounds.width / 2;
  const move = y => page.mouse.move(x, y);
  await move(bounds.y + bounds.height - 4);
  await expect.poll(() => content.evaluate(element => element.scrollTop)).toBeGreaterThan(40);
  const first = await content.evaluate(element => element.scrollTop);
  await expect.poll(() => content.evaluate(element => element.scrollTop)).toBeGreaterThan(first + 40);
  await move(bounds.y + bounds.height / 2);
  const stopped = await content.evaluate(element => element.scrollTop);
  await page.waitForTimeout(150);
  expect(await content.evaluate(element => element.scrollTop)).toBeCloseTo(stopped, 0);
  await move(bounds.y + 4);
  await expect.poll(() => content.evaluate(element => element.scrollTop)).toBeLessThan(stopped - 40);
  await expect.poll(() => content.evaluate(element => element.scrollTop)).toBe(0);
  const gap = await editor.locator('[data-before-key="group-management"]').boundingBox();
  await page.mouse.move(gap.x + gap.width / 2, gap.y + gap.height / 2);
  await page.mouse.up();
  await expect(editor.locator('.menu-editor-standalone[data-menu-key="Dashboard"]')).toHaveCount(1);
  const dropped = await content.evaluate(element => element.scrollTop);
  await page.waitForTimeout(150);
  expect(await content.evaluate(element => element.scrollTop)).toBeCloseTo(dropped, 0);
  await editor.getByRole('button', { name: '保存', exact: true }).click();
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/system/menu_config').at(-1).body
    .map(item => item.key)).toEqual(['group-discovery', 'Dashboard', 'group-management', 'group-system']);
  await page.reload();
  await expect(page.locator('.v-navigation-drawer').getByText('仪表盘', { exact: true })).toHaveCount(1);
});
