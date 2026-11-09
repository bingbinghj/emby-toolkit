import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
const start = page.indexOf('const repairCurrentAudioTracks = ');
const end = page.indexOf('\n};', start) + 4;
async function repair(data, reject = false) {
  const messages = [], reloads = [];
  const context = vm.createContext({
    currentEditMediaId: { value: '101' }, isRepairingTruehdCore: { value: false },
    axios: { post: async () => { if (reject) throw { response: { data, status: 500 } }; return { data }; } },
    openMediaInfoEditor: async id => reloads.push(id),
    message: {
      loading: () => ({ destroy() {} }),
      ...Object.fromEntries(['success', 'warning', 'error', 'info'].map(type => [type, text => messages.push({ type, text })])),
    },
  });
  vm.runInContext(`${page.slice(start, end)}\nthis.repair = repairCurrentAudioTracks;`, context);
  await context.repair();
  return { messages, reloads };
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
