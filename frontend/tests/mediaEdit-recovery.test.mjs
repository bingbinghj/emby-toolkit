import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
const record = { id: 'saved-recovery', directory: '/config/backups/media-info-reload/saved-recovery', target_ids: ['101'], reason: '前次媒体刷新尚未完成验收', can_recover: true };
const pendingState = () => ({ pending: true, records: [structuredClone(record)] });
const result = { message: '恢复已通过验收', recovered_id: record.id, pending: false, backup_deleted: true };
const cleanupRecord = { ...record, kind: 'verified-cleanup', reason: '媒体与原配置已验收，备份清理尚未完成，可重试删除' };
const functionSource = name => {
  const start = page.indexOf(`const ${name} = `);
  assert.ok(start >= 0, `missing actual ${name} handler`);
  return page.slice(start, page.indexOf('\n};', start) + 4);
};
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
function fixture({ get = async () => ({ data: { pending: false, records: [] } }), post = async () => ({ data: result }) } = {}) {
  const messages = [], requests = [], reloads = [], dialogs = [];
  const draft = [{ MediaSourceInfo: { MediaStreams: [{ Type: 'Audio', Title: '原标题' }] } }];
  const context = vm.createContext({
    mediaRecoveryState: { value: pendingState() }, mediaRecoveryError: { value: '' },
    isCheckingMediaRecovery: { value: false }, isRecoveringMediaInfo: { value: false },
    isConfirmingMediaRecovery: { value: false }, isSavingMediaInfo: { value: false },
    mediaRecoveryLoadSequence: 0, mediaRecoverySession: 0, mediaRecoveryDialog: null,
    currentEditMediaId: { value: '101' }, mediaInfoLoadSequence: 1,
    mediaInfoData: { value: draft }, mediaInfoLoadedSnapshot: JSON.stringify(draft),
    mediaInfoSaveFeedback: { value: null }, showMediaInfoEditor: { value: true },
    mediaOperationFeedback: { value: null },
    openMediaInfoEditor: async id => { reloads.push(id); context.mediaInfoLoadSequence += 1; },
    axios: {
      get: async url => { requests.push({ method: 'GET', url }); return get(); },
      post: async (url, body) => { requests.push({ method: 'POST', url, body: structuredClone(body) }); return post(); },
    },
    message: Object.fromEntries(['success', 'warning', 'error'].map(type => [type, text => messages.push({ type, text })])),
    dialog: { warning: options => { dialogs.push(options); return { destroy() { options.onClose?.(); } }; } },
  });
  vm.runInContext(['mediaOperationDetails', 'loadMediaRecovery', 'recoveryConflictValue', 'recoveryJsonConflictHashes', 'recoverMediaInfo', 'confirmMediaRecovery'].map(functionSource).join('\n') + '\nthis.load = loadMediaRecovery; this.recover = recoverMediaInfo; this.confirm = confirmMediaRecovery;', context);
  return { context, requests, messages, reloads, dialogs };
}

test('recovery status discovers the actual pending record and directory', async () => {
  const f = fixture({ get: async () => ({ data: pendingState() }) });
  assert.equal(await f.context.load(), true);
  assert.equal(f.context.mediaRecoveryState.value.pending, true);
  assert.equal(f.context.mediaRecoveryState.value.records[0].directory, record.directory);
  assert.equal(f.context.isCheckingMediaRecovery.value, false);
});

for (const get of [async () => { throw { response: { data: { error: '恢复目录无法读取' } } }; }, async () => ({ data: { status: 'ok' } })]) {
  test('unreadable or invalid status retains pending recovery instead of unlocking saving', async () => {
    const f = fixture({ get });
    assert.equal(await f.context.load(), false);
    assert.equal(f.context.mediaRecoveryState.value.pending, true);
    assert.equal(f.context.mediaRecoveryState.value.records[0].id, record.id);
    assert.ok(f.context.mediaRecoveryError.value);
    assert.equal(f.context.isCheckingMediaRecovery.value, false);
  });
}

test('the latest recovery status wins over a late earlier read', async () => {
  const old = deferred();
  let reads = 0;
  const f = fixture({ get: () => ++reads === 1 ? old.promise : Promise.resolve({ data: { pending: false, records: [] } }) });
  const first = f.context.load();
  await f.context.load();
  old.resolve({ data: pendingState() });
  await first;
  assert.equal(f.context.mediaRecoveryState.value.pending, false);
  assert.equal(f.context.mediaRecoveryError.value, '');
});

test('successful task unlocks saving only after status confirms no pending records', async () => {
  const f = fixture();
  await f.context.recover(record);
  assert.deepEqual(f.requests.map(request => request.method), ['POST', 'GET']);
  assert.equal(f.requests[0].body.recovery_id, record.id);
  assert.equal(f.context.mediaRecoveryState.value.pending, false);
  assert.deepEqual(f.reloads, ['101']);
  assert.equal(f.messages.at(-1).type, 'success');
});

test('other pending records retain the restriction after one record is restored', async () => {
  const f = fixture({ get: async () => ({ data: pendingState() }) });
  await f.context.recover(record);
  assert.equal(f.context.mediaRecoveryState.value.pending, true);
  assert.equal(f.messages.at(-1).type, 'warning');
  assert.deepEqual(f.reloads, []);
});

test('recovery cleanup warning rechecks status and retains the cleanup action', async () => {
  const warning = `临时备份清理失败，恢复材料仍保留：${record.directory}`;
  const f = fixture({
    post: async () => ({ status: 207, data: { ...result, pending: true, backup_deleted: false, warnings: 1, warning_details: [warning] } }),
    get: async () => ({ data: { pending: true, records: [cleanupRecord] } }),
  });
  await f.context.recover(record);
  assert.equal(f.context.mediaRecoveryState.value.pending, true);
  assert.equal(f.context.mediaRecoveryState.value.records[0].kind, 'verified-cleanup');
  assert.equal(f.messages.at(-1).type, 'warning');
  assert.deepEqual(f.requests.map(request => request.method), ['POST', 'GET']);
  assert.equal(f.context.mediaOperationFeedback.value.details[0], warning);
  assert.equal(f.context.mediaInfoSaveFeedback.value.details[0], warning);
  assert.equal(f.context.mediaRecoveryError.value, warning);
  assert.deepEqual(f.reloads, []);
});

test('verified cleanup confirmation only asks to delete already verified backups', () => {
  const f = fixture();
  f.context.mediaRecoveryState.value.records = [cleanupRecord];
  f.context.confirm(cleanupRecord);
  assert.equal(f.dialogs[0].title, '清理恢复备份');
  assert.equal(f.dialogs[0].positiveText, '开始清理');
  assert.match(f.dialogs[0].content, /原神医配置已通过验收/);
  assert.match(f.dialogs[0].content, /删除已验收的恢复备份/);
});

test('retrying cleanup deletes the backup without reloading or replacing a media draft', async () => {
  let attempts = 0;
  const warning = '备份删除失败：目录不可写';
  const f = fixture({
    post: async () => ++attempts === 1
      ? { status: 207, data: { ...result, pending: true, backup_deleted: false, warnings: 1, warning_details: [warning] } }
      : { status: 200, data: { ...result, message: '恢复备份已清理' } },
    get: async () => ({ data: attempts === 1 ? { pending: true, records: [cleanupRecord] } : { pending: false, records: [] } }),
  });
  const draft = JSON.stringify(f.context.mediaInfoData.value);
  await f.context.recover(record);
  await f.context.recover(f.context.mediaRecoveryState.value.records[0]);
  assert.equal(f.context.mediaRecoveryState.value.pending, false);
  assert.equal(f.context.mediaRecoveryError.value, '');
  assert.equal(f.context.mediaOperationFeedback.value, null);
  assert.equal(f.context.mediaInfoSaveFeedback.value, null);
  assert.deepEqual(f.requests.map(request => request.method), ['POST', 'GET', 'POST', 'GET']);
  assert.equal(f.requests[0].body.recovery_id, f.requests[2].body.recovery_id);
  assert.deepEqual(f.reloads, []);
  assert.equal(JSON.stringify(f.context.mediaInfoData.value), draft);
  assert.equal(f.messages.at(-1).type, 'success');
});

test('failed final task keeps recovery entry and the actual failure', async () => {
  const f = fixture({ post: async () => { throw { response: { data: { error: '61616：章节图片尚未恢复' } } }; }, get: async () => ({ data: pendingState() }) });
  const draft = JSON.stringify(f.context.mediaInfoData.value);
  await f.context.recover(record);
  assert.equal(f.context.mediaRecoveryState.value.pending, true);
  assert.match(f.context.mediaRecoveryError.value, /章节图片尚未恢复/);
  assert.equal(f.messages.at(-1).type, 'error');
  assert.equal(JSON.stringify(f.context.mediaInfoData.value), draft);
  assert.deepEqual(f.reloads, []);
  assert.equal(f.context.isRecoveringMediaInfo.value, false);
});

test('a task response without a recovery receipt cannot claim success', async () => {
  const f = fixture({ post: async () => ({ data: { message: '已提交刷新' } }) });
  await f.context.recover(record);
  assert.equal(f.context.mediaRecoveryState.value.pending, true);
  assert.equal(f.messages.at(-1).type, 'error');
  assert.deepEqual(f.reloads, []);
});

test('a failed status read after the task preserves recovery and avoids false success', async () => {
  const f = fixture({ get: async () => { throw new Error('状态回读失败'); } });
  await f.context.recover(record);
  assert.equal(f.context.mediaRecoveryState.value.pending, true);
  assert.match(f.context.mediaRecoveryError.value, /状态回读失败/);
  assert.deepEqual(f.messages, []);
  assert.deepEqual(f.reloads, []);
});

for (const edit of ['before', 'during']) {
  test(`recovery preserves an unsaved media draft edited ${edit} the request`, async () => {
    const work = deferred();
    const f = fixture({ post: () => work.promise });
    const change = () => { f.context.mediaInfoData.value[0].MediaSourceInfo.MediaStreams[0].Title = '保留用户标题'; };
    if (edit === 'before') change();
    const recovering = f.context.recover(record);
    if (edit === 'during') change();
    work.resolve({ data: result });
    await recovering;
    assert.deepEqual(f.reloads, []);
    assert.equal(f.context.mediaInfoData.value[0].MediaSourceInfo.MediaStreams[0].Title, '保留用户标题');
    assert.match(f.context.mediaInfoSaveFeedback.value.message, /草稿已保留/);
  });
}

for (const change of ['version', 'closed', 'reopened']) {
  test(`a recovery result cannot reload an editor that was ${change}`, async () => {
    const work = deferred();
    const f = fixture({ post: () => work.promise });
    const recovering = f.context.recover(record);
    if (change === 'version') f.context.currentEditMediaId.value = '102';
    if (change === 'closed') f.context.showMediaInfoEditor.value = false;
    f.context.mediaInfoLoadSequence += 1;
    work.resolve({ data: result });
    await recovering;
    assert.deepEqual(f.reloads, []);
    assert.equal(f.context.mediaInfoSaveFeedback.value, null);
  });
}

for (const failed of [false, true]) {
  test(`late recovery ${failed ? 'failure' : 'success'} stays out of a different page session`, async () => {
    const work = deferred();
    const f = fixture({ post: () => work.promise });
    const recovering = f.context.recover(record);
    f.context.mediaRecoverySession += 1;
    if (failed) work.reject(new Error('旧恢复失败'));
    else work.resolve({ data: result });
    await recovering;
    assert.deepEqual(f.messages, []);
    assert.deepEqual(f.reloads, []);
    assert.equal(f.context.mediaRecoveryError.value, '');
    assert.equal(f.context.mediaRecoveryState.value.pending, true);
  });
}

test('duplicate recovery clicks submit exactly one request', async () => {
  const work = deferred();
  const f = fixture({ post: () => work.promise });
  const first = f.context.recover(record);
  await f.context.recover(record);
  assert.equal(f.requests.filter(request => request.method === 'POST').length, 1);
  assert.equal(f.context.isRecoveringMediaInfo.value, true);
  work.resolve({ data: result });
  await first;
  assert.equal(f.context.isRecoveringMediaInfo.value, false);
});

test('only one confirmation opens and an old confirmation cannot recover on a new page', async () => {
  const f = fixture();
  f.context.confirm(record);
  f.context.confirm(record);
  assert.equal(f.dialogs.length, 1);
  assert.match(f.dialogs[0].content, /草稿会保留/);
  f.context.mediaRecoverySession += 1;
  await f.dialogs[0].onPositiveClick();
  assert.deepEqual(f.requests, []);
  f.dialogs[0].onClose();
  assert.equal(f.context.isConfirmingMediaRecovery.value, false);
});

const currentHash = 'a'.repeat(64);
const nextHash = 'b'.repeat(64);
const preservedDirectory = '/config/backups/media-info-conflicts/retained-current-json';
const jsonConflictRecord = (hash = currentHash) => ({
  ...record,
  json_conflicts: [{
    item_id: '101', path: '/media/Movie-mediainfo.json', current_sha256: hash,
    changes: [{ path: '/0/MediaSourceInfo/MediaStreams/0/Title', expected: '原标题', current: '用户修改' }],
    change_count: 1,
  }],
});

function setConflict(f, record = jsonConflictRecord()) {
  f.context.mediaRecoveryState.value = { pending: true, records: [record] };
  return record;
}

test('a real JSON conflict cannot be overwritten by the default recovery action', async () => {
  const f = fixture();
  const changed = setConflict(f);
  await f.context.recover(changed);
  assert.deepEqual(f.requests, []);
  assert.equal(f.context.mediaRecoveryState.value.pending, true);
  assert.match(f.context.mediaRecoveryError.value, /确认备份后恢复/);
});

test('explicit conflict confirmation submits the exact shown hash and retains the current JSON backup path', async () => {
  const f = fixture({ post: async () => ({ data: { ...result, conflict_backup_directories: [preservedDirectory] } }) });
  const changed = setConflict(f);
  f.context.mediaInfoData.value[0].MediaSourceInfo.MediaStreams[0].Title = '保留本地草稿';
  f.context.confirm(changed);
  assert.equal(f.dialogs[0].title, '备份当前 JSON 并恢复');
  assert.equal(f.dialogs[0].positiveText, '备份并恢复');
  assert.match(f.dialogs[0].content, /当前文件备份到独立留存目录/);
  assert.ok(f.dialogs[0].content.includes(currentHash));
  await f.dialogs[0].onPositiveClick();
  assert.deepEqual(f.requests[0].body, { recovery_id: record.id, current_json_hashes: { '101': currentHash } });
  assert.equal(f.context.mediaRecoveryState.value.pending, false);
  assert.deepEqual(f.reloads, []);
  assert.equal(f.context.mediaInfoData.value[0].MediaSourceInfo.MediaStreams[0].Title, '保留本地草稿');
  assert.ok(f.context.mediaInfoSaveFeedback.value.details.some(detail => detail.includes(preservedDirectory)));
  assert.ok(f.context.mediaOperationFeedback.value.details.some(detail => detail.includes(preservedDirectory)));
});

test('a pristine editor reload keeps the separate current JSON retention directory visible', async () => {
  const f = fixture({ post: async () => ({ data: { ...result, conflict_backup_directories: [preservedDirectory] } }) });
  f.context.confirm(setConflict(f));
  await f.dialogs[0].onPositiveClick();
  assert.deepEqual(f.reloads, ['101']);
  assert.equal(f.context.mediaInfoSaveFeedback.value.type, 'info');
  assert.ok(f.context.mediaInfoSaveFeedback.value.details.some(detail => detail.includes(preservedDirectory)));
});

test('a confirmation for an older shown JSON hash cannot submit after status changes', async () => {
  const f = fixture();
  f.context.confirm(setConflict(f));
  setConflict(f, jsonConflictRecord(nextHash));
  await f.dialogs[0].onPositiveClick();
  assert.deepEqual(f.requests, []);
  assert.equal(f.context.mediaRecoveryState.value.records[0].json_conflicts[0].current_sha256, nextHash);
  assert.match(f.context.mediaRecoveryError.value, /重新检查差异/);
});

test('a conflicting JSON without a complete current hash cannot open an overwrite confirmation', () => {
  const f = fixture();
  f.context.confirm(setConflict(f, jsonConflictRecord('')));
  assert.deepEqual(f.dialogs, []);
  assert.equal(f.context.isConfirmingMediaRecovery.value, false);
  assert.match(f.context.mediaRecoveryError.value, /缺少有效哈希/);
});

test('a hash conflict from the server refreshes diagnostics before a new explicit confirmation', async () => {
  let attempts = 0;
  const f = fixture({
    post: async () => {
      if (++attempts === 1) throw { response: { status: 409, data: { error: '当前 JSON 哈希已变化，未覆盖文件' } } };
      return { data: { ...result, conflict_backup_directories: [preservedDirectory] } };
    },
    get: async () => ({ data: attempts === 1 ? { pending: true, records: [jsonConflictRecord(nextHash)] } : { pending: false, records: [] } }),
  });
  f.context.confirm(setConflict(f));
  await f.dialogs[0].onPositiveClick();
  assert.equal(f.context.mediaRecoveryState.value.records[0].json_conflicts[0].current_sha256, nextHash);
  assert.match(f.context.mediaRecoveryError.value, /未覆盖文件/);
  assert.deepEqual(f.requests.map(request => request.method), ['POST', 'GET']);
  f.dialogs[0].onClose();
  f.context.confirm(f.context.mediaRecoveryState.value.records[0]);
  await f.dialogs[1].onPositiveClick();
  assert.deepEqual(f.requests[2].body.current_json_hashes, { '101': nextHash });
  assert.equal(f.context.mediaRecoveryState.value.pending, false);
});

test('every conflicting item hash is captured in one explicit confirmation', async () => {
  const f = fixture();
  const changed = jsonConflictRecord();
  changed.json_conflicts.push({ ...changed.json_conflicts[0], item_id: '102', path: '/media/Second-mediainfo.json', current_sha256: nextHash });
  f.context.confirm(setConflict(f, changed));
  await f.dialogs[0].onPositiveClick();
  assert.deepEqual(f.requests[0].body.current_json_hashes, { '101': currentHash, '102': nextHash });
});

test('backup deletion warnings still expose the separately retained current JSON directory', async () => {
  const f = fixture({
    post: async () => ({ status: 207, data: { ...result, pending: true, backup_deleted: false, warnings: 1, warning_details: ['原恢复目录删除失败'], conflict_backup_directories: [preservedDirectory] } }),
    get: async () => ({ data: { pending: true, records: [{ ...cleanupRecord, conflict_backup_directories: [preservedDirectory] }] } }),
  });
  f.context.confirm(setConflict(f));
  await f.dialogs[0].onPositiveClick();
  assert.equal(f.context.mediaRecoveryState.value.pending, true);
  assert.equal(f.messages.at(-1).type, 'warning');
  assert.ok(f.context.mediaInfoSaveFeedback.value.details.some(detail => detail.includes(preservedDirectory)));
});
