import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../src/components/settings/general/LocalPathField.vue', import.meta.url), 'utf8');
const handlers = source.slice(source.indexOf('const nodeFor'), source.indexOf('watch(menuOpen'));
const deferred = () => {
  let resolve;
  const promise = new Promise(yes => { resolve = yes; });
  return { promise, resolve };
};
function fixture(get = async (_, { params }) => ({ data: { code: 200, data: [], current_path: params.path } })) {
  const requests = [], updates = [], context = vm.createContext({
    menuOpen: { value: true }, failure: { value: null }, props: { modelValue: '', multiple: false },
    emit: (event, value) => updates.push(value),
    axios: { get: async (url, options) => { requests.push(options.params.path); return get(url, options); } },
  });
  vm.runInContext(handlers + '\nthis.read = readChildren; this.choose = choose; this.node = nodeFor; this.close = () => {generation++;menuOpen.value = false};', context);
  return { context, requests, updates };
}
test('opening branches reads once per path and updates the draft only on selection', async () => {
  const f = fixture(), root = f.context.node('/', '/'), child = f.context.node('/media', 'media');
  await Promise.all([f.context.read(root), f.context.read(root)]);
  await f.context.read(child);
  assert.deepEqual(f.updates, []);
  await f.context.choose(child);
  assert.deepEqual(f.requests, ['/', '/media']);
  assert.deepEqual(f.updates, ['/media']);
});
for (const data of [{ code: 403 }, { code: 200, current_path: null, data: [] }, { code: 200, current_path: '/', data: [{ name: 'bad', path: null }] }]) {
  test('invalid directory replies preserve the field and allow retry', async () => {
    let reads = 0;
    const f = fixture(async () => ({ data: reads++ ? { code: 200, current_path: '/', data: [] } : data }));
    const root = f.context.node('/', '/');
    await f.context.choose(root);
    assert.ok(f.context.failure.value); assert.deepEqual(f.updates, []);
    await f.context.choose(root);
    assert.deepEqual(f.updates, ['/']); assert.equal(f.context.failure.value, null);
  });
}
test('closing a pending branch prevents both stale contents and selection', async () => {
  const wait = deferred(), f = fixture(() => wait.promise), branch = f.context.node('/old', 'old');
  const pending = f.context.choose(branch);
  f.context.close();
  wait.resolve({ data: { code: 200, current_path: '/old', data: [] } });
  await pending;
  assert.deepEqual(f.updates, []);
  assert.equal(branch.loaded, undefined);
});
test('multi-path selection preserves and deduplicates existing paths', async () => {
  const f = fixture(); f.context.props = { multiple: true, modelValue: ['/a', '/b'] };
  await f.context.choose(f.context.node('/b', 'b'));
  assert.deepEqual([...f.updates[0]], ['/a', '/b']);
});
