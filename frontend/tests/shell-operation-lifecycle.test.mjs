import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import axios from 'axios';
import { ref, shallowRef, watch } from 'vue';

const source = readFileSync(new URL('../src/MainLayout.vue', import.meta.url), 'utf8');
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
function definition(name, declaration = 'const') {
  const start = source.indexOf(declaration === 'const' ? `const ${name} = ` : `${declaration} ${name}(`);
  assert.ok(start >= 0, name);
  const ending = declaration === 'const' ? '\n};' : '\n}';
  return source.slice(start, source.indexOf(ending, start) + ending.length);
}
function installWatcher(t, context, name) {
  const match = source.match(new RegExp(`watch\\(${name},[^\\n]+\\);`));
  if (!match) return;
  context.watch = (...args) => { const stop = watch(...args); t.after(stop); return stop; };
  vm.runInContext(match[0], context);
}
function menuFixture(t) {
  const pending = deferred(), requests = [], messages = [];
  const client = axios.create({ adapter: async config => {
    requests.push({ path: config.url, body: JSON.parse(config.data) });
    await pending.promise;
    return { data: {}, status: 200, statusText: 'OK', headers: {}, config };
  } });
  const context = vm.createContext({
    authStore: { isLoggedIn: true, user: { id: 'admin-A' } },
    menuConfigLoaded: ref(true), isSavingMenu: ref(false), menuEditorSession: 0,
    isMenuEditorVisible: ref(true), menuConfigTree: ref([{ key: 'group-discovery', label: 'submitted A', children: [] }]),
    clearDragState() {}, axios: client,
    message: Object.fromEntries(['success', 'error', 'info'].map(level => [level, text => messages.push({ level, text })])),
  });
  installWatcher(t, context, 'isMenuEditorVisible');
  vm.runInContext(`${definition('saveMenuConfig')}\nthis.save = saveMenuConfig;`, context);
  return { context, pending, requests, messages };
}

test('navigation saves the submitted snapshot while retaining newer draft edits', async t => {
  const f = menuFixture(t), saving = f.context.save();
  f.context.menuConfigTree.value[0].label = 'new unsaved B';
  f.pending.resolve(); await saving;
  assert.equal(f.requests.length, 1);
  assert.equal(f.requests[0].body[0].label, 'submitted A');
  assert.equal(f.context.menuConfigTree.value[0].label, 'new unsaved B');
  assert.equal(f.context.isMenuEditorVisible.value, true);
  assert.deepEqual(f.messages.map(item => item.level), ['info']);
  assert.match(f.messages[0].text, /当前.*未保存/);
});
test('navigation failure identifies the submitted request and retains newer edits', async t => {
  const f = menuFixture(t), saving = f.context.save();
  f.context.menuConfigTree.value[0].label = 'new unsaved B';
  f.pending.reject(new Error('submitted request failed')); await saving;
  assert.equal(f.context.isMenuEditorVisible.value, true);
  assert.equal(f.context.menuConfigTree.value[0].label, 'new unsaved B');
  assert.deepEqual(f.messages.map(item => item.level), ['error']);
  assert.match(f.messages[0].text, /提交时.*保存失败.*当前修改已保留/);
});
for (const outcome of ['success', 'failure']) {
  test(`old navigation ${outcome} leaves a reopened editor untouched`, async t => {
    const f = menuFixture(t), saving = f.context.save();
    f.context.isMenuEditorVisible.value = false;
    f.context.isMenuEditorVisible.value = true;
    if (outcome === 'success') f.pending.resolve(); else f.pending.reject(new Error('old menu error'));
    await saving;
    assert.equal(f.context.isMenuEditorVisible.value, true);
    assert.deepEqual(f.messages, []);
    assert.equal(f.context.isSavingMenu.value, false);
  });
}
test('old navigation response cannot report into another authenticated account', async t => {
  const f = menuFixture(t), saving = f.context.save();
  f.context.authStore.user = { id: 'admin-B' };
  f.pending.resolve(); await saving;
  assert.equal(f.context.isMenuEditorVisible.value, true);
  assert.deepEqual(f.messages, []);
});
test('unchanged navigation save closes the same editor and repeated clicks send once', async t => {
  const f = menuFixture(t), saving = f.context.save();
  await f.context.save();
  assert.equal(f.requests.length, 1);
  f.pending.resolve(); await saving;
  assert.equal(f.context.isMenuEditorVisible.value, false);
  assert.deepEqual(f.messages.map(item => item.level), ['success']);
});

function searchFixture(t) {
  const requests = [], messages = [];
  const makeContext = label => ({ label, query: ref(' query A '), submit: () => {
    const pending = deferred(); requests.push(pending); return pending.promise;
  }, clear: () => { const pending = deferred(); requests.push(pending); return pending.promise; } });
  const current = makeContext('page A');
  const context = vm.createContext({
    authStore: { isLoggedIn: true, user: { id: 'admin-A' } }, activeContextSearch: shallowRef(current),
    searchDisabled: ref(false), searchComposing: ref(false), searchOpen: ref(true),
    searchRequestSequence: 0, searchWindowSession: 0, fallbackSearch: ref(''),
    message: { error: text => messages.push(text) },
  });
  context.searchWord = { get value() { return String(context.activeContextSearch.value?.query.value ?? context.fallbackSearch.value); } };
  installWatcher(t, context, 'searchOpen');
  vm.runInContext(`${definition('updateSearch', 'function')}\n${definition('submitSearch', 'async function')}\n${definition('clearSearch', 'async function')}\nthis.submit = submitSearch; this.clear = clearSearch;`, context);
  return { context, current, requests, messages, makeContext };
}
for (const operation of ['submit', 'clear']) for (const outcome of ['success', 'failure']) {
  test(`old ${operation} ${outcome} cannot affect another page's search window`, async t => {
    const f = searchFixture(t), work = f.context[operation]();
    f.context.searchOpen.value = false;
    f.context.activeContextSearch.value = f.makeContext('page B');
    f.context.searchOpen.value = true;
    if (outcome === 'success') f.requests[0].resolve(); else f.requests[0].reject(new Error('page A error'));
    await work;
    assert.equal(f.context.searchOpen.value, true);
    assert.equal(f.context.activeContextSearch.value.label, 'page B');
    assert.deepEqual(f.messages, []);
  });
}
test('old search success cannot close the same page search after it is reopened', async t => {
  const f = searchFixture(t), work = f.context.submit();
  f.context.searchOpen.value = false;
  f.context.searchOpen.value = true;
  f.requests[0].resolve(); await work;
  assert.equal(f.context.searchOpen.value, true);
});
test('search keeps newer text entered while the submitted query is pending', async t => {
  const f = searchFixture(t), work = f.context.submit();
  f.current.query.value = 'new unsaved query';
  f.requests[0].resolve(); await work;
  assert.equal(f.context.searchOpen.value, true);
  assert.equal(f.context.searchWord.value, 'new unsaved query');
});
test('only the latest submitted search may close its unchanged window', async t => {
  const f = searchFixture(t), first = f.context.submit(), second = f.context.submit();
  f.requests[0].resolve(); await first;
  assert.equal(f.context.searchOpen.value, true);
  f.current.query.value = f.current.query.value.trim();
  f.requests[1].resolve(); await second;
  assert.equal(f.context.searchOpen.value, false);
});
test('a current search failure remains visible in its original window', async t => {
  const f = searchFixture(t), work = f.context.submit();
  f.requests[0].reject(new Error('current search error')); await work;
  assert.deepEqual(f.messages, ['current search error']);
  assert.equal(f.context.searchOpen.value, true);
});
for (const applied of [true, false]) {
  test(`${applied ? 'applied' : 'stale'} logout only navigates when the auth mutation applies`, async () => {
    const pending = deferred(), navigations = [];
    const context = vm.createContext({
      authStore: { logout: () => pending.promise },
      router: { push: route => navigations.push(route.name) },
    });
    vm.runInContext(`${definition('handleUserSelect')}\nthis.select = handleUserSelect;`, context);
    const work = context.select('logout');
    pending.resolve(applied); await work;
    assert.deepEqual(navigations, applied ? ['Login'] : []);
  });
}
