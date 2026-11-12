import { test, expect } from './fixtures.js';

const recoveryPath = '/api/media_info/recovery';
const recoveryId = 'media:saved-71b4d21e-f026-44c4-aa70-944c9c370e53';
const currentHash = 'a'.repeat(64);
const changedHash = 'b'.repeat(64);
const currentPath = '/media/Tiga-mediainfo.json';
const preservedDirectory = '/config/backups/media-info-conflicts/preserved-current-61616';
const conflict = {
  item_id: '61616', path: currentPath, current_sha256: currentHash,
  changes: [{ path: '/0/MediaSourceInfo/MediaStreams/1/Title', expected: '原标题', current: '用户修改' }],
};
const record = {
  id: recoveryId, kind: 'media-reload', target_ids: ['61616'],
  directory: '/config/backups/media-info-reload/saved-71b4d21e-f026-44c4-aa70-944c9c370e53',
  reason: '当前媒体 JSON 与恢复备份内容不同，需要先保留当前文件再恢复',
  can_recover: true, json_conflicts: [conflict],
};
const pendingState = { pending: true, records: [record] };
const success = {
  recovered_id: recoveryId, pending: false, backup_deleted: true, warnings: 0, warning_details: [],
  message: '媒体与原配置已通过验收，原恢复备份已删除', conflict_backup_directories: [preservedDirectory],
};
const editor = page => page.locator('.v-overlay--active').filter({ has: page.getByTestId('media-info-save') });
const recoveryPosts = api => api.requests.filter(request => request.method === 'POST' && request.path === recoveryPath);
const recoveryReads = api => api.requests.filter(request => request.method === 'GET' && request.path === recoveryPath);
const action = page => editor(page).getByRole('button', { name: '备份当前 JSON 并恢复', exact: true });
const confirmation = page => page.locator('.v-overlay--active').filter({ has: page.getByRole('button', { name: '备份并恢复', exact: true }) });
async function openEditor(page, api, state = pendingState) {
  api.respond('GET', recoveryPath, state);
  await page.goto('/edit-media/101');
  await expect(page.getByTestId('media-recovery-alert')).toBeVisible();
  await page.getByTestId('media-info-open').click();
  await expect(editor(page).getByTestId('media-info-save')).toBeDisabled();
}
async function confirmRestore(page) {
  await action(page).click();
  await confirmation(page).getByRole('button', { name: '备份并恢复', exact: true }).click();
}

test('JSON conflict diagnostics identify the actual changed file and cancel does not authorize an overwrite', async ({ page, api }) => {
  api.respond('GET', recoveryPath, pendingState);
  await page.goto('/edit-media/101');
  const pageAlert = page.getByTestId('media-recovery-alert');
  await expect(pageAlert).toContainText(currentPath);
  await expect(pageAlert).toContainText(currentHash);
  await expect(pageAlert).toContainText(conflict.changes[0].path);
  await expect(pageAlert).toContainText('原标题');
  await expect(pageAlert).toContainText('用户修改');
  await page.getByTestId('media-info-open').click();
  await expect(editor(page).getByTestId('media-recovery-alert')).toContainText(currentPath);
  await expect(editor(page).getByTestId('media-recovery-alert')).toContainText(currentHash);
  await action(page).click();
  await expect(confirmation(page)).toContainText('备份');
  await expect(confirmation(page)).toContainText('当前 JSON');
  await confirmation(page).getByRole('button', { name: '取消', exact: true }).click();
  await expect(action(page)).toBeEnabled();
  expect(recoveryPosts(api)).toEqual([]);
  await expect(editor(page).getByTestId('media-info-save')).toBeDisabled();
});

test('explicit confirmation submits the exact current hash and successful recovery shows the retained current JSON', async ({ page, api }) => {
  let verified = false;
  api.queueTask('POST', recoveryPath, success, { id: 'conflict-restore-task', status: 200, state: 'completed', polls: 0 });
  api.respond('GET', '/api/tasks/result/conflict-restore-task', () => {
    verified = true;
    return { task_id: 'conflict-restore-task', state: 'completed', result_status: 200, result: success };
  });
  await openEditor(page, api);
  api.respond('GET', recoveryPath, () => verified ? { pending: false, records: [] } : pendingState);
  const title = editor(page).getByRole('textbox', { name: /音轨.*1.*标题/ });
  await title.fill('保留当前未保存的编辑草稿');
  await confirmRestore(page);
  await expect(editor(page)).toContainText(preservedDirectory);
  await expect(title).toHaveValue('保留当前未保存的编辑草稿');
  await expect(editor(page).getByTestId('media-info-save')).toBeEnabled();
  await expect(editor(page).getByTestId('media-recovery-alert')).toHaveCount(0);
  expect(recoveryPosts(api)).toHaveLength(1);
  expect(recoveryPosts(api)[0].body).toEqual({ recovery_id: recoveryId, current_json_hashes: { '61616': currentHash } });
  expect(api.requests.some(request => request.path === '/api/tasks/result/conflict-restore-task')).toBe(true);
  expect(api.requests.filter(request => request.method === 'POST' && request.path === '/api/media_info/edit/101')).toEqual([]);
});

test('verified cleanup removes the old conflict backup notice while preserving the unsaved draft', async ({ page, api }) => {
  const withBackup = { pending: true, records: [{ ...record, conflict_backup_directories: [preservedDirectory] }] };
  await openEditor(page, api, withBackup);
  const title = editor(page).getByRole('textbox', { name: /音轨.*1.*标题/ });
  await title.fill('清理后保留草稿');
  await expect(editor(page)).toContainText(preservedDirectory);
  api.respond('POST', recoveryPath, () => {
    api.respond('GET', recoveryPath, { pending: false, records: [] });
    return { ...success, conflict_backup_directories: [], deleted_conflict_backup_directories: [preservedDirectory] };
  });
  await action(page).click();
  await expect(confirmation(page)).toContainText('完整验收后删除原恢复备份及关联的冲突 JSON 备份');
  await confirmation(page).getByRole('button', { name: '备份并恢复', exact: true }).click();
  await expect(editor(page).getByTestId('media-info-save')).toBeEnabled();
  await expect(editor(page)).not.toContainText(preservedDirectory);
  await expect(page.getByText('当前 JSON 备份已保留', { exact: true })).toHaveCount(0);
  await expect(title).toHaveValue('清理后保留草稿');
});

test('a current JSON change after confirmation retains the draft and requires the newly read hash for retry', async ({ page, api }) => {
  const changedState = {
    pending: true,
    records: [{ ...record, json_conflicts: [{ ...conflict, current_sha256: changedHash, changes: [{ ...conflict.changes[0], current: '再次修改的标题' }] }] }],
  };
  const failed = { error: '当前 JSON 在确认后再次变化，请重新检查后确认；所有备份已保留' };
  let changed = false;
  api.queueTask('POST', recoveryPath, failed, { id: 'changed-json-task', status: 409, state: 'failed', polls: 0 });
  api.respond('GET', '/api/tasks/result/changed-json-task', () => {
    changed = true;
    return { task_id: 'changed-json-task', state: 'failed', result_status: 409, result: failed };
  });
  await openEditor(page, api);
  api.respond('GET', recoveryPath, () => changed ? changedState : pendingState);
  const readsBefore = recoveryReads(api).length;
  const title = editor(page).getByRole('textbox', { name: /音轨.*1.*标题/ });
  await title.fill('保留失败后的草稿');
  await confirmRestore(page);
  await expect(editor(page).getByTestId('media-recovery-alert')).toContainText('再次变化');
  await expect(editor(page).getByTestId('media-recovery-alert')).toContainText(changedHash);
  await expect(editor(page).getByTestId('media-recovery-alert')).toContainText('再次修改的标题');
  await expect.poll(() => recoveryReads(api).length).toBeGreaterThan(readsBefore);
  await expect(action(page)).toBeEnabled();
  await expect(editor(page).getByTestId('media-info-save')).toBeDisabled();
  await expect(title).toHaveValue('保留失败后的草稿');
  expect(recoveryPosts(api)).toHaveLength(1);
  expect(recoveryPosts(api)[0].body.current_json_hashes).toEqual({ '61616': currentHash });

  api.respond('POST', recoveryPath, () => {
    api.respond('GET', recoveryPath, { pending: false, records: [] });
    return success;
  });
  await confirmRestore(page);
  await expect(editor(page).getByTestId('media-info-save')).toBeEnabled();
  await expect(editor(page)).toContainText(preservedDirectory);
  await expect(title).toHaveValue('保留失败后的草稿');
  expect(recoveryPosts(api)).toHaveLength(2);
  expect(recoveryPosts(api)[1].body).toEqual({ recovery_id: recoveryId, current_json_hashes: { '61616': changedHash } });
});

test('an incomplete current JSON hash cannot authorize a conflict overwrite', async ({ page, api }) => {
  const unreadableState = { pending: true, records: [{ ...record, json_conflicts: [{ ...conflict, current_sha256: '' }] }] };
  await openEditor(page, api, unreadableState);
  await expect(editor(page).getByTestId('media-recovery-alert')).toContainText(currentPath);
  await expect(action(page)).toBeDisabled();
  await expect(editor(page).getByTestId('media-info-save')).toBeDisabled();
  expect(recoveryPosts(api)).toEqual([]);
});

test('duplicate conflict confirmation clicks submit only one preservation and restoration request', async ({ page, api }) => {
  const operation = api.defer('POST', recoveryPath);
  await openEditor(page, api);
  const title = editor(page).getByRole('textbox', { name: /音轨.*1.*标题/ });
  await title.fill('重复确认期间保留的草稿');
  await action(page).click();
  const confirm = confirmation(page).getByRole('button', { name: '备份并恢复', exact: true });
  await confirm.evaluate(button => { button.click(); button.click(); });
  await operation.started;
  await expect(confirm).toBeDisabled();
  expect(recoveryPosts(api)).toHaveLength(1);
  expect(recoveryPosts(api)[0].body.current_json_hashes).toEqual({ '61616': currentHash });
  api.respond('GET', recoveryPath, { pending: false, records: [] });
  operation.release(success);
  await expect(editor(page).getByTestId('media-info-save')).toBeEnabled();
  await expect(title).toHaveValue('重复确认期间保留的草稿');
  await expect(editor(page)).toContainText(preservedDirectory);
  expect(recoveryPosts(api)).toHaveLength(1);
});
