import { test, expect } from './fixtures.js';

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
  await page.getByRole('button', { name: '保存修改', exact: true }).click();
  await pending.started;
  await leaveEditor(page);
  await finishRequest(page, pending.release, { message: '演员已保存' });
  await expect(page).toHaveURL(/\/dashboard$/);
  await expect(page.getByText('修改已保存，Emby将自动刷新。', { exact: true })).not.toBeVisible();
});

test('leaving the editor cancels already scheduled actor save navigation', async ({ page }) => {
  await page.goto('/edit-media/101');
  await page.getByRole('button', { name: '保存修改', exact: true }).click();
  await expect(page.getByText('修改已保存，Emby将自动刷新。', { exact: true })).toBeVisible();
  await leaveEditor(page);
  await page.waitForTimeout(1800);
  await expect(page).toHaveURL(/\/dashboard$/);
});

test('an older actor save cannot navigate after the same media is reopened', async ({ page, api }) => {
  const pending = api.defer('POST', actorSavePath);
  await page.goto('/edit-media/101');
  await page.getByRole('button', { name: '保存修改', exact: true }).click();
  await pending.started;
  await leaveEditor(page);
  await page.goBack();
  await expect(page).toHaveURL(/\/edit-media\/101$/);
  await expect(page.getByRole('button', { name: '保存修改', exact: true })).toBeVisible();
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
  await page.getByRole('button', { name: '保存修改', exact: true }).click();
  await started;
  await leaveEditor(page);
  await finishRequest(page, release, { error: '旧演员保存失败' });
  await expect(page).toHaveURL(/\/dashboard$/);
  await expect(page.getByText('旧演员保存失败', { exact: true })).not.toBeVisible();
});
