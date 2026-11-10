import { test, expect } from './fixtures.js';

const path = '/api/media_info/recovery';
const record = {
  id: 'saved-71b4d21e-f026-44c4-aa70-944c9c370e53',
  directory: '/config/backups/media-info-reload/saved-71b4d21e-f026-44c4-aa70-944c9c370e53',
  target_ids: ['61616'], reason: '前次媒体刷新尚未完成验收，暂不修改神医配置', can_recover: true,
};
const pendingState = { pending: true, records: [record] };
const recoveryResult = { message: '媒体信息已恢复并通过回读验收', recovered_id: record.id, pending: false };
const editor = page => page.locator('.v-overlay--active').filter({ has: page.getByTestId('media-info-save') });
const posts = api => api.requests.filter(request => request.method === 'POST' && request.path === path);
async function openPending(page, api) {
  api.respond('GET', path, pendingState);
  await page.goto('/edit-media/101');
  await expect(page.getByTestId('media-recovery-alert')).toBeVisible();
  await page.getByTestId('media-info-open').click();
  await expect(editor(page).getByTestId('media-info-save')).toBeDisabled();
}
async function confirmRecovery(page) {
  await editor(page).getByRole('button', { name: '恢复媒体信息', exact: true }).click();
  await page.getByRole('button', { name: '开始恢复', exact: true }).click();
}

test('pending recovery appears on the edit page and in the media dialog with a recovery action', async ({ page, api }) => {
  api.respond('GET', path, pendingState);
  await page.goto('/edit-media/101');
  await expect(page.getByTestId('media-recovery-alert')).toContainText(record.directory);
  await expect(page.getByRole('button', { name: '恢复媒体信息', exact: true })).toBeEnabled();
  await page.getByTestId('media-info-open').click();
  await expect(editor(page).getByTestId('media-recovery-alert')).toContainText(record.reason);
  await expect(editor(page).getByTestId('media-info-save')).toBeDisabled();
  expect(posts(api)).toEqual([]);
});

test('a background recovery is verified again before the editor resumes saving', async ({ page, api }) => {
  let restored = false;
  api.queueTask('POST', path, recoveryResult, { id: 'recovery-task', status: 200, state: 'completed', polls: 0 });
  api.respond('GET', '/api/tasks/result/recovery-task', () => {
    restored = true;
    return { task_id: 'recovery-task', state: 'completed', result_status: 200, result: recoveryResult };
  });
  await openPending(page, api);
  api.respond('GET', path, () => restored ? { pending: false, records: [] } : pendingState);
  await confirmRecovery(page);
  await expect(page.getByText(recoveryResult.message, { exact: true })).toBeVisible();
  await expect(editor(page).getByTestId('media-info-save')).toBeEnabled();
  await expect(editor(page).getByTestId('media-recovery-alert')).toHaveCount(0);
  expect(posts(api)).toHaveLength(1);
  expect(posts(api)[0].body).toEqual({ recovery_id: record.id });
  expect(api.requests.some(request => request.path === '/api/tasks/result/recovery-task')).toBe(true);
});

test('recovery failure retains its button, directory, draft and save restriction', async ({ page, api }) => {
  api.queueTask('POST', path, { error: '61616：章节图片尚未恢复；原备份已保留' }, { id: 'failed-recovery', status: 409, state: 'failed', polls: 0 });
  await openPending(page, api);
  const title = editor(page).getByRole('textbox', { name: /音轨.*1.*标题/ });
  await title.fill('保留恢复期间的草稿');
  await confirmRecovery(page);
  await expect(editor(page).getByTestId('media-recovery-alert')).toContainText('章节图片尚未恢复');
  await expect(editor(page).getByRole('button', { name: '恢复媒体信息', exact: true })).toBeEnabled();
  await expect(editor(page).getByTestId('media-info-save')).toBeDisabled();
  await expect(title).toHaveValue('保留恢复期间的草稿');
  await expect(editor(page).getByTestId('media-recovery-alert')).toContainText(record.directory);
  expect(posts(api)).toHaveLength(1);
});

test('verified recovery with a cleanup warning retains its directory details and permits saving', async ({ page, api }) => {
  const warning = `临时备份清理失败，恢复材料仍保留：${record.directory}`;
  await openPending(page, api);
  api.respond('POST', path, () => {
    api.respond('GET', path, { pending: false, records: [] });
    return { ...recoveryResult, warnings: 1, warning_details: [warning] };
  });
  await confirmRecovery(page);
  await expect(editor(page).getByText('恢复已通过验收，仍有收尾提醒', { exact: true })).toBeVisible();
  await expect(editor(page).getByText(warning, { exact: true })).toBeVisible();
  await expect(editor(page).getByTestId('media-info-save')).toBeEnabled();
  await expect(editor(page).getByTestId('media-recovery-alert')).toHaveCount(0);
});

test('an unavailable recovery API blocks saving until its status can be checked', async ({ page, api }) => {
  api.respond('GET', path, { error: '恢复状态接口暂不可用' }, 404);
  await page.goto('/edit-media/101');
  await page.getByTestId('media-info-open').click();
  await expect(editor(page).getByTestId('media-recovery-alert')).toContainText('恢复状态接口暂不可用');
  await expect(editor(page).getByTestId('media-info-save')).toBeDisabled();
  api.respond('GET', path, { pending: false, records: [] });
  await editor(page).getByRole('button', { name: '重新检查恢复状态', exact: true }).click();
  await expect(editor(page).getByTestId('media-recovery-alert')).toHaveCount(0);
  await expect(editor(page).getByTestId('media-info-save')).toBeEnabled();
});

test('successful recovery preserves an existing unsaved title draft', async ({ page, api }) => {
  const operation = api.defer('POST', path);
  await openPending(page, api);
  const title = editor(page).getByRole('textbox', { name: /音轨.*1.*标题/ });
  await title.fill('恢复前的新标题');
  await confirmRecovery(page);
  await operation.started;
  api.respond('GET', path, { pending: false, records: [] });
  operation.release(recoveryResult);
  await expect(editor(page).getByText('恢复已完成，当前编辑草稿已保留', { exact: true })).toBeVisible();
  await expect(title).toHaveValue('恢复前的新标题');
  await expect(editor(page).getByTestId('media-info-save')).toBeEnabled();
  expect(posts(api)).toHaveLength(1);
});

test('double confirmation clicks cannot submit a second recovery task', async ({ page, api }) => {
  const operation = api.defer('POST', path);
  await openPending(page, api);
  await editor(page).getByRole('button', { name: '恢复媒体信息', exact: true }).click();
  const confirm = page.getByRole('button', { name: '开始恢复', exact: true });
  await confirm.evaluate(button => { button.click(); button.click(); });
  await operation.started;
  await expect(confirm).toBeDisabled();
  expect(posts(api)).toHaveLength(1);
  api.respond('GET', path, { pending: false, records: [] });
  operation.release(recoveryResult);
  await expect(editor(page).getByTestId('media-info-save')).toBeEnabled();
});
