import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('../src/MainLayout.vue', import.meta.url), 'utf8');
function definition(name, async = false) {
  const start = source.indexOf(`${async ? 'async ' : ''}function ${name}(`);
  assert.ok(start >= 0, `${name} must be the real shell handler`);
  return source.slice(start, source.indexOf('\n}', start) + 2);
}
function fixture(context) {
  const navigations = [], errors = [], inputs = [], submissions = [], clears = [];
  const state = vm.createContext({
    activeContextSearch: { value: context && { query: { value: '' }, composing: { value: false },
      onInput: value => inputs.push(value), submit: () => submissions.push(true), clear: () => clears.push(true), ...context } },
    fallbackSearch: { value: '' }, searchComposing: { value: false }, searchDisabled: { value: false }, searchOpen: { value: true },
    router: { push: async route => navigations.push(JSON.parse(JSON.stringify(route))) },
    message: { error: error => errors.push(error) },
  });
  state.searchWord = { get value() { return state.activeContextSearch.value?.query.value ?? state.fallbackSearch.value; } };
  vm.runInContext([
    definition('updateSearch'), definition('submitSearch', true), definition('clearSearch', true),
    definition('startSearchComposition'), definition('finishSearchComposition'),
  ].join('\n') + '\nthis.input=updateSearch;this.submit=submitSearch;this.clear=clearSearch;this.startIME=startSearchComposition;this.endIME=finishSearchComposition;', state);
  return { state, navigations, errors, inputs, submissions, clears };
}

test('context input and submission use the current page without navigating to global media', async () => {
  const f = fixture({});
  f.state.input('剧集');
  assert.equal(f.state.activeContextSearch.value.query.value, '剧集');
  assert.deepEqual(f.inputs, ['剧集']);
  await f.state.submit();
  assert.equal(f.submissions.length, 1);
  assert.equal(f.navigations.length, 0);
  assert.equal(f.state.searchOpen.value, false);
  await f.state.clear();
  assert.equal(f.state.activeContextSearch.value.query.value, '');
  assert.equal(f.clears.length, 1);
});

test('composition does not submit a partial keyword and resumes the page debounce after completion', async () => {
  const f = fixture({});
  f.state.startIME();
  f.state.input('zhong');
  await f.state.submit();
  assert.deepEqual(f.inputs, []);
  assert.deepEqual(f.submissions, []);
  assert.equal(f.state.activeContextSearch.value.composing.value, true);
  f.state.input('中文');
  f.state.endIME();
  assert.equal(f.state.activeContextSearch.value.composing.value, false);
  assert.deepEqual(f.inputs, ['中文']);
  await f.state.submit();
  assert.equal(f.submissions.length, 1);
});

test('a page composition completion callback is preserved rather than scheduled twice', () => {
  let completed = 0;
  const f = fixture({ compositionEnd: () => completed++ });
  f.state.startIME();
  f.state.input('中文');
  f.state.endIME();
  assert.equal(completed, 1);
  assert.deepEqual(f.inputs, []);
});

test('global search navigates with the trimmed keyword and clearing does not navigate', async () => {
  const f = fixture(null);
  f.state.input('  最近电影  ');
  await f.state.submit();
  assert.deepEqual(f.navigations, [{ name: 'ReviewList', query: { q: '最近电影' } }]);
  await f.state.clear();
  assert.equal(f.state.fallbackSearch.value, '');
  assert.equal(f.navigations.length, 1);
});

test('disabled page search preserves the query and submits no work', async () => {
  const f = fixture({ query: { value: '保留' } });
  f.state.searchDisabled.value = true;
  f.state.input('修改');
  await f.state.submit();
  await f.state.clear();
  assert.equal(f.state.activeContextSearch.value.query.value, '保留');
  assert.deepEqual(f.submissions, []);
  assert.deepEqual(f.clears, []);
  assert.deepEqual(f.navigations, []);
});

test('page search failures stay visible and never fall back to a different scope', async () => {
  const f = fixture({ submit: async () => { throw new Error('当前库读取失败'); } });
  await f.state.submit();
  assert.deepEqual(f.errors, ['当前库读取失败']);
  assert.deepEqual(f.navigations, []);
  assert.equal(f.state.searchOpen.value, true);
});

test('unmounting a replaced page cannot clear the new page search registration', () => {
  const mounts = [], unmounts = [];
  const registry = vm.createContext({ shallowRef: value => ({ value }),
    onMounted: callback => mounts.push(callback), onBeforeUnmount: callback => unmounts.push(callback) });
  const composable = readFileSync(new URL('../src/composables/useContextSearch.js', import.meta.url), 'utf8');
  vm.runInContext(composable.replace(/^import .*;\n/, '').replaceAll('export ', '')
    + '\nthis.register=useContextSearch;this.active=activeContextSearch;', registry);
  const first = registry.register({ label: '搜索当前媒体库', query: { value: '电影' } });
  const second = registry.register({ label: '搜索用户', query: { value: '用户' } });
  mounts[0]();
  assert.equal(registry.active.value, first);
  mounts[1]();
  unmounts[0]();
  assert.equal(registry.active.value, second);
  unmounts[1]();
  assert.equal(registry.active.value, null);
});
