import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../src/components/settings/general/LocalPathField.vue', import.meta.url), 'utf8');
const handlers = source.slice(source.indexOf('const loadDirectory ='), source.indexOf('watch(menuOpen'));
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

function fixture(get = async (_, { params }) => ({ data: { code: 200, data: [], current_path: params.path } })) {
  const requests = [], updates = [];
  const context = vm.createContext({
    menuOpen: { value: true }, loading: { value: false }, error: { value: '' },
    directories: { value: [] }, currentPath: { value: '' }, requestedPath: { value: '/' }, requestVersion: 0,
    emit: (event, value) => updates.push({ event, value }),
    axios: { get: async (url, options) => { requests.push({ url, options }); return get(url, options); } },
  });
  vm.runInContext(handlers + '\nthis.load = loadDirectory; this.choose = chooseDirectory;', context);
  return { context, requests, updates };
}

test('browsing parent and child directories performs GET reads without changing the input', async () => {
  const f = fixture();
  await f.context.load('/config');
  await f.context.load('/config/metadata');
  await f.context.load('/config');
  assert.deepEqual(f.requests.map(request => request.url), Array(3).fill('/api/p115/system/directories'));
  assert.deepEqual(f.requests.map(request => request.options.params.path), ['/config', '/config/metadata', '/config']);
  assert.deepEqual(f.updates, []);
  f.context.choose();
  assert.deepEqual(f.updates, [{ event: 'update:modelValue', value: '/config' }]);
  assert.equal(f.context.menuOpen.value, false);
});

test('empty input opens the filesystem root and allows selecting an empty directory', async () => {
  const f = fixture();
  await f.context.load('');
  assert.equal(f.requests[0].options.params.path, '/');
  assert.equal(f.context.currentPath.value, '/');
  assert.equal(f.context.directories.value.length, 0);
  f.context.choose();
  assert.equal(f.updates[0].value, '/');
});

test('a failed read preserves the input and retries only the requested directory', async () => {
  let reads = 0;
  const f = fixture(async (_, { params }) => {
    if (!reads++) throw { response: { data: { message: '目录未挂载' } } };
    return { data: { code: 200, data: [], current_path: params.path } };
  });
  await f.context.load('/metadata');
  assert.equal(f.context.error.value, '目录未挂载');
  assert.equal(f.context.currentPath.value, '');
  f.context.choose();
  assert.deepEqual(f.updates, []);
  await f.context.load(f.context.requestedPath.value);
  assert.equal(f.context.currentPath.value, '/metadata');
  assert.equal(f.context.error.value, '');
});

for (const data of [
  { code: 403, message: '没有目录访问权限' },
  { code: 200, current_path: '/config', data: [{ name: 'invalid', path: null }] },
  { code: 200, current_path: null, data: [] },
]) {
  test('invalid directory responses cannot select or overwrite a path', async () => {
    const f = fixture(async () => ({ data }));
    await f.context.load('/config');
    assert.ok(f.context.error.value);
    f.context.choose();
    assert.deepEqual(f.updates, []);
  });
}

test('a late earlier directory response cannot replace the latest directory', async () => {
  const old = deferred();
  const f = fixture((_, { params }) => params.path === '/old' ? old.promise
    : Promise.resolve({ data: { code: 200, data: [], current_path: params.path } }));
  const first = f.context.load('/old');
  await f.context.load('/new');
  old.resolve({ data: { code: 200, data: [], current_path: '/old' } });
  await first;
  assert.equal(f.context.currentPath.value, '/new');
  assert.equal(f.context.loading.value, false);
});

test('closing the picker prevents a pending read from applying or selecting a path', async () => {
  const work = deferred();
  const f = fixture(() => work.promise);
  const request = f.context.load('/config');
  f.context.menuOpen.value = false;
  work.resolve({ data: { code: 200, data: [], current_path: '/config' } });
  await request;
  f.context.choose();
  assert.equal(f.context.currentPath.value, '');
  assert.deepEqual(f.updates, []);
});
