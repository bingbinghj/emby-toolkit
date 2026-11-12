import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
function definition(name, optional = false) {
  const start = page.indexOf(`const ${name} = `);
  if (optional && start < 0) return '';
  assert.ok(start >= 0, `${name} must be an actual page function`);
  return page.slice(start, page.indexOf('\n};', start) + 4);
}

async function operate(name, data, status = 200, reject = false) {
  const messages = [], routes = [], reloads = [];
  const context = vm.createContext({
    itemId: { value: '101' }, currentEditMediaId: { value: '101' }, renameTargetId: { value: '101' },
    currentMediaInfoVersion: { value: { emby_id: '101' } }, mediaInfoVersionOptions: { value: [{}, {}] },
    showMediaInfoEditor: { value: true }, mediaOperationFeedback: { value: null },
    isReplacingEpisodeImages: { value: false }, isDeletingMediaVersion: { value: false },
    imageRefreshTokens: { value: { poster: 1 } },
    mediaDetailsLoadSequence: 1, mediaRecoverySession: 1, mediaInfoLoadSequence: 1,
    axios: { post: async () => {
      if (reject) throw { response: { data, status } };
      return { data, status };
    } },
    router: { replace: async value => routes.push(value) },
    fetchMediaDetails: async () => reloads.push(context.itemId.value),
    message: {
      loading: () => ({ destroy() {} }),
      ...Object.fromEntries(['success', 'warning', 'error'].map(type => [type, text => messages.push({ type, text })])),
    },
  });
  vm.runInContext(`${definition('mediaOperationDetails', true)}\n${definition(name)}\nthis.run = ${name};`, context);
  await context.run();
  return { context, messages, routes, reloads };
}

for (const [data, status] of [
  [{ message: '剧集图处理完成：失败1项', failures: ['第3集未生成新截图'] }, 207],
  [{ warnings: 1, warning_details: ['恢复配置失败'] }, 200],
  [{ message: '部分分集未完成' }, 207],
]) {
  test(`image warnings remain visible for ${JSON.stringify(data)}`, async () => {
    const f = await operate('replaceEpisodeImages', data, status);
    assert.equal(f.messages.at(-1).type, 'warning');
    assert.equal(f.context.mediaOperationFeedback.value.type, 'warning');
    assert.ok(f.context.mediaOperationFeedback.value.details.length > 0);
    assert.equal(f.context.imageRefreshTokens.value.poster, 1);
    assert.equal(f.context.isReplacingEpisodeImages.value, false);
  });
}

test('confirmed image replacement refreshes the poster without discarding actor drafts', async () => {
  const f = await operate('replaceEpisodeImages', { message: '已生成1/1集', refreshed_count: 1, episode_count: 1 });
  assert.equal(f.messages.at(-1).type, 'success');
  assert.notEqual(f.context.imageRefreshTokens.value.poster, 1);
  assert.deepEqual(f.reloads, []);
});

test('failed image replacement exposes the recovery reason', async () => {
  const f = await operate('replaceEpisodeImages', { error: '截图恢复失败', failures: [{ item_id: '101', reason: '原配置仍保留' }] }, 500, true);
  assert.equal(f.messages.at(-1).type, 'error');
  assert.match(f.context.mediaOperationFeedback.value.details.join('\n'), /101.*原配置仍保留/);
  assert.equal(f.context.imageRefreshTokens.value.poster, 1);
});

test('confirmed deletion with merge warnings leaves a visible warning on the remaining version', async () => {
  const f = await operate('deleteCurrentMediaVersion', {
    deleted_item_id: '101', remaining_item_id: '102', warnings: 1,
    warning_details: ['剩余版本重新合并失败'],
  });
  assert.equal(f.messages.at(-1).type, 'warning');
  assert.match(f.context.mediaOperationFeedback.value.details.join('\n'), /重新合并失败/);
  assert.equal(f.context.showMediaInfoEditor.value, false, 'the deleted version must no longer be editable');
  assert.equal(f.context.currentEditMediaId.value, '102');
  assert.equal(f.routes[0].params.itemId, '102');
  assert.deepEqual(f.reloads, ['102']);
});

test('unconfirmed deletion warnings preserve the current editor and route', async () => {
  const f = await operate('deleteCurrentMediaVersion', { warnings: 1, warning_details: ['尚未确认删除'] }, 207);
  assert.equal(f.messages.at(-1).type, 'warning');
  assert.equal(f.context.showMediaInfoEditor.value, true);
  assert.equal(f.context.currentEditMediaId.value, '101');
  assert.deepEqual(f.routes, []);
  assert.deepEqual(f.reloads, []);
});

test('successful deletion switches only to the valid surviving version', async () => {
  const f = await operate('deleteCurrentMediaVersion', { deleted_item_id: '101', remaining_item_id: '102' });
  assert.equal(f.messages.at(-1).type, 'success');
  assert.equal(f.context.showMediaInfoEditor.value, false);
  assert.equal(f.routes[0].params.itemId, '102');
  assert.deepEqual(f.reloads, ['102']);
});

test('a deleted ID returned as the remaining ID never becomes the navigation target', async () => {
  const f = await operate('deleteCurrentMediaVersion', { deleted_item_id: '101', remaining_item_id: '101' });
  assert.equal(f.messages.at(-1).type, 'warning');
  assert.deepEqual(f.routes, []);
  assert.deepEqual(f.reloads, []);
  assert.equal(f.context.showMediaInfoEditor.value, false);
});

test('failed deletion preserves the editor and shows its detailed cause', async () => {
  const f = await operate('deleteCurrentMediaVersion', { error: '删除失败', failures: ['版本回滚未完成'] }, 500, true);
  assert.equal(f.messages.at(-1).type, 'error');
  assert.match(f.context.mediaOperationFeedback.value.details.join('\n'), /回滚未完成/);
  assert.equal(f.context.showMediaInfoEditor.value, true);
  assert.deepEqual(f.routes, []);
});
