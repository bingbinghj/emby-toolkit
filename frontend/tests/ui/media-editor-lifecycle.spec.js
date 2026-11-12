import { test, expect, mediaDetails, mediaInfo } from './fixtures.js';

const actorSavePath = '/api/update_media_cast_sa/101';

async function leaveEditor(page) {
  await page.locator('.v-navigation-drawer').getByText('仪表盘', { exact: true }).click();
  await expect(page).toHaveURL(/\/dashboard$/);
  await expect(page.locator('.dashboard-page')).toBeVisible();
}

async function finishRequest(page, release, result) {
  const response = page.waitForResponse(value => value.url().endsWith(actorSavePath) && value.request().method() === 'POST');
  release(result);
  await response;
  // The former editor scheduled its navigation 1.5 seconds after a successful save.
  await page.waitForTimeout(1800);
}

test('late actor save cannot navigate after the editor is unmounted', async ({ page, api }) => {
  const pending = api.defer('POST', actorSavePath);
  await page.goto('/edit-media/101');
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await pending.started;
  await leaveEditor(page);
  await finishRequest(page, pending.release, { message: '演员已保存' });
  await expect(page).toHaveURL(/\/dashboard$/);
  await expect(page.getByText('修改已保存，Emby将自动刷新。', { exact: true })).not.toBeVisible();
});

test('leaving the editor cancels already scheduled actor save navigation', async ({ page }) => {
  await page.goto('/edit-media/101');
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await expect(page.getByText('修改已保存，Emby将自动刷新。', { exact: true })).toBeVisible();
  await leaveEditor(page);
  await page.waitForTimeout(1800);
  await expect(page).toHaveURL(/\/dashboard$/);
});

test('an older actor save cannot navigate after the same media is reopened', async ({ page, api }) => {
  const pending = api.defer('POST', actorSavePath);
  await page.goto('/edit-media/101');
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await pending.started;
  await leaveEditor(page);
  await page.goBack();
  await expect(page).toHaveURL(/\/edit-media\/101$/);
  await expect(page.getByRole('button', { name: '保存', exact: true })).toBeVisible();
  await finishRequest(page, pending.release, { message: '旧会话演员已保存' });
  await expect(page).toHaveURL(/\/edit-media\/101$/);
  await expect(page.getByText('修改已保存，Emby将自动刷新。', { exact: true })).not.toBeVisible();
});

test('late actor failure stays with the editor that submitted it', async ({ page, api }) => {
  let received, release;
  const started = new Promise(resolve => { received = resolve; });
  const completion = new Promise(resolve => { release = resolve; });
  api.respond('POST', actorSavePath, async () => { received(); return completion; }, 500);
  await page.goto('/edit-media/101');
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await started;
  await leaveEditor(page);
  await finishRequest(page, release, { error: '旧演员保存失败' });
  await expect(page).toHaveURL(/\/dashboard$/);
  await expect(page.getByText('旧演员保存失败', { exact: true })).not.toBeVisible();
});

test('a delayed version deletion cannot redirect a different media opened by application navigation', async ({ page, api }) => {
  const deletionPath = '/api/media_info/delete_version/101';
  const pending = api.defer('POST', deletionPath);
  api.respond('GET', '/api/media_for_editing/200', { ...mediaDetails, item_id: '200', item_name: '新页面媒体' });
  api.respond('GET', '/api/media_for_editing/102', { ...mediaDetails, item_id: '102', item_name: '旧媒体剩余版本' });
  api.respond('GET', '/api/media_info/edit/101', {
    ...mediaInfo,
    versions: [...mediaInfo.versions, { ...mediaInfo.versions[0], emby_id: '102', file_name: 'Remaining.Movie.mkv' }],
  });

  await page.goto('/edit-media/101');
  await expect(page).toHaveURL(/\/edit-media\/101$/);
  await page.getByTestId('media-info-open').click();
  const editor = page.locator('.v-overlay--active').filter({ has: page.getByTestId('media-info-save') });
  await editor.getByRole('button', { name: '删除当前版本', exact: true }).click();
  const confirmation = page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '确认', exact: true }) });
  await confirmation.getByRole('button', { name: '确认', exact: true }).click();
  await pending.started;

  // Busy confirmations absorb browser Back. A separate application navigation
  // still runs the real router, route watchers and unmount hooks in this context.
  await page.evaluate(async () => {
    await document.getElementById('app').__vue_app__.config.globalProperties.$router.push('/edit-media/200');
  });
  await expect(page).toHaveURL(/\/edit-media\/200$/);
  await expect(page.locator('.editor-page')).toContainText('新页面媒体');
  const finished = page.waitForResponse(response => response.url().endsWith(deletionPath) && response.request().method() === 'POST');
  pending.release({ deleted_item_id: '101', remaining_item_id: '102', message: '旧版本删除完成' });
  await finished;
  // The confirmation awaits the whole deletion handler, including any redirect.
  await expect(confirmation).toBeHidden();
  await expect(page).toHaveURL(/\/edit-media\/200$/);
  await expect(page.locator('.editor-page')).toContainText('新页面媒体');
  await expect(page.getByText('旧版本删除完成', { exact: true })).not.toBeVisible();
  expect(api.requests.filter(request => request.path === '/api/media_for_editing/102')).toHaveLength(0);
  expect(api.requests.filter(request => request.method === 'POST' && request.path === deletionPath)).toHaveLength(1);
});
