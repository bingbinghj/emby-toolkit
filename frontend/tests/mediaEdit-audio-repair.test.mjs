import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
const start = page.indexOf('const repairCurrentAudioTracks = ');
const end = page.indexOf('\n};', start) + 4;
function repairFixture({ data, reject = false, post } = {}) {
  const messages = [], reloads = [], requests = [];
  let loadingDestroyed = 0;
  const context = vm.createContext({
    currentEditMediaId: { value: '101' }, isRepairingTruehdCore: { value: false },
    showMediaInfoEditor: { value: true }, mediaInfoLoadSequence: 1,
    axios: { post: async url => {
      requests.push(url);
      if (post) return post(url);
      if (reject) throw { response: { data, status: 500 } };
      return { data };
    } },
    openMediaInfoEditor: async id => reloads.push(id),
    message: {
      loading: () => ({ destroy() { loadingDestroyed += 1; } }),
      ...Object.fromEntries(['success', 'warning', 'error', 'info'].map(type => [type, text => messages.push({ type, text })])),
    },
  });
  vm.runInContext(`${page.slice(start, end)}\nthis.repair = repairCurrentAudioTracks;`, context);
  return { context, messages, reloads, requests, run: () => context.repair(), get loadingDestroyed() { return loadingDestroyed; } };
}
async function repair(data, reject = false) {
  const fixture = repairFixture({ data, reject });
  await fixture.run();
  return fixture;
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}
for (const data of [{ repaired: 1 }, { result: { repaired: 1 } }]) {
  test(`successful repair reloads the editor for ${JSON.stringify(data)}`, async () => {
    const f = await repair(data);
    assert.deepEqual(f.reloads, ['101']);
    assert.equal(f.messages.at(-1).type, 'success');
  });
}
test('ambiguous report warns with its reason and keeps original data', async () => {
  const f = await repair({ repaired: 0, ambiguous: 1, message: '纠正0项，歧义1项', ambiguous_items: [{ reason: '缺少可信原始探测' }] });
  assert.equal(f.messages.at(-1).type, 'warning');
  assert.match(f.messages.at(-1).text, /缺少可信原始探测/);
  assert.deepEqual(f.reloads, []);
});
test('ordinary skipped repair remains informational', async () => {
  const f = await repair({ repaired: 0, skipped: 1, message: '未发现高清兼容核心结构' });
  assert.equal(f.messages.at(-1).type, 'info');
  assert.deepEqual(f.reloads, []);
});
test('failed repair exposes rollback failure instead of only its counts', async () => {
  const f = await repair({ failed: 1, message: '纠正0项，失败1项', failed_items: [{ reason: '原文件或缓存回滚未完成' }] }, true);
  assert.equal(f.messages.at(-1).type, 'error');
  assert.match(f.messages.at(-1).text, /回滚未完成/);
  assert.deepEqual(f.reloads, []);
});
for (const failed of [false, true]) {
  test(`late repair ${failed ? 'failure' : 'success'} cannot affect another version`, async () => {
    const pending = deferred();
    const f = repairFixture({ post: () => pending.promise });
    const work = f.run();
    f.context.currentEditMediaId.value = '202';
    f.context.mediaInfoLoadSequence += 1;
    if (failed) pending.reject({ response: { status: 500, data: { error: '旧版本恢复失败' } } });
    else pending.resolve({ data: { repaired: 1 } });
    await work;
    assert.deepEqual(f.requests, ['/api/media_info/repair_truehd_core/101']);
    assert.deepEqual(f.reloads, []);
    assert.deepEqual(f.messages, []);
    assert.equal(f.context.currentEditMediaId.value, '202');
    assert.equal(f.context.isRepairingTruehdCore.value, false);
    assert.equal(f.loadingDestroyed, 1);
  });
}
test('late repair does not reopen an editor that was closed', async () => {
  const pending = deferred();
  const f = repairFixture({ post: () => pending.promise });
  const work = f.run();
  f.context.showMediaInfoEditor.value = false;
  f.context.mediaInfoLoadSequence += 1;
  pending.resolve({ data: { result: { repaired: 1 } } });
  await work;
  assert.deepEqual(f.reloads, []);
  assert.deepEqual(f.messages, []);
  assert.equal(f.context.showMediaInfoEditor.value, false);
  assert.equal(f.loadingDestroyed, 1);
});
test('late repair cannot reload a newly opened editor for the same media', async () => {
  const pending = deferred();
  const f = repairFixture({ post: () => pending.promise });
  const work = f.run();
  f.context.mediaInfoLoadSequence += 2;
  pending.resolve({ data: { repaired: 1 } });
  await work;
  assert.deepEqual(f.reloads, []);
  assert.deepEqual(f.messages, []);
  assert.equal(f.context.showMediaInfoEditor.value, true);
  assert.equal(f.context.currentEditMediaId.value, '101');
  assert.equal(f.loadingDestroyed, 1);
});
test('duplicate repair calls share one in-flight operation', async () => {
  const pending = deferred();
  const f = repairFixture({ post: () => pending.promise });
  const work = f.run();
  await f.run();
  assert.deepEqual(f.requests, ['/api/media_info/repair_truehd_core/101']);
  pending.resolve({ data: { repaired: 1 } });
  await work;
  assert.deepEqual(f.reloads, ['101']);
  assert.equal(f.messages.length, 1);
  assert.equal(f.messages[0].type, 'success');
  assert.equal(f.context.isRepairingTruehdCore.value, false);
  assert.equal(f.loadingDestroyed, 1);
});
