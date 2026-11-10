import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../src/components/ReviewList.vue', import.meta.url), 'utf8');
const copy = value => JSON.parse(JSON.stringify(value));
function definition(name) {
  const start = source.indexOf(`const ${name} = `);
  const end = source.indexOf('\n};', start) + 4;
  assert.ok(start >= 0 && end > start, name);
  return source.slice(start, end);
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function fixture(get = async () => ({ data: { items: [], total_items: 0 } })) {
  const calls = [], messages = [], timers = new Map();
  let timerId = 0;
  const context = vm.createContext({
    pageActive: true, listSequence: 0, searchTimer: null,
    currentPage: { value: 1 }, itemsPerPage: { value: 15 }, loading: { value: false }, error: { value: null },
    selectedMediaType: { value: null }, searchQuery: { value: '' }, searchComposing: { value: false }, compactList: { value: false },
    tableData: { value: [] }, totalItems: { value: 0 }, isShowingSearchResults: { value: false },
    console: { error() {} }, message: { error: value => messages.push(value) },
    setTimeout: (callback, delay) => { const id = ++timerId; timers.set(id, { callback, delay }); return id; },
    clearTimeout: id => timers.delete(id),
    axios: { get: async (url, options) => { calls.push({ url, params: copy(options.params) }); return get(url, options); } },
  });
  const functions = ['fetchReviewItems', 'searchEmbyLibrary', 'handleFetchError', 'handleSearchQueryChange', 'clearSearch', 'handleSearch', 'handleMediaTypeChange'];
  vm.runInContext(functions.map(definition).join('\n') + '\n' + source.match(/const finishSearchComposition = .*;/)[0] + '\nthis.load = fetchReviewItems; this.search = searchEmbyLibrary; this.change = handleSearchQueryChange; this.compose = finishSearchComposition; this.clear = clearSearch; this.submit = handleSearch; this.filter = handleMediaTypeChange;', context);
  return { context, calls, messages, timers };
}

test('a late review list cannot overwrite a newer Emby search or show an obsolete error', async () => {
  for (const rejected of [false, true]) {
    const pending = deferred();
    const f = fixture(async url => url === '/api/review_items' ? pending.promise : { data: { items: [{ item_id: 'new', item_name: '新电影' }], total_items: 1 } });
    const old = f.context.load();
    f.context.searchQuery.value = '新电影';
    await f.context.search();
    if (rejected) pending.reject({ response: { data: { error: '旧列表失败' } } });
    else pending.resolve({ data: { items: [{ item_id: 'old' }], total_items: 1 } });
    await old;
    assert.equal(f.context.tableData.value[0].item_id, 'new');
    assert.equal(f.context.loading.value, false);
    assert.equal(f.context.error.value, null);
    assert.deepEqual(f.messages, []);
  }
});

test('clearing a pending search restores review records and ignores the late search response', async () => {
  const pending = deferred();
  const f = fixture(async url => url === '/api/search_emby_library' ? pending.promise : { data: { items: [{ item_id: 'failed', reason: '媒体恢复失败' }], total_items: 1 } });
  f.context.searchQuery.value = '电影';
  const old = f.context.search();
  f.context.clear();
  await new Promise(resolve => setImmediate(resolve));
  pending.resolve({ data: { items: [{ item_id: 'old-search' }], total_items: 1 } });
  await old;
  assert.equal(f.context.tableData.value[0].item_id, 'failed');
  assert.equal(f.context.isShowingSearchResults.value, false);
  assert.equal(f.context.searchQuery.value, '');
  assert.deepEqual(f.calls.at(-1).params, { page: 1, per_page: 15, media_type: '' });
});

test('typing searches only after the MP delay and composing Chinese text never submits partial input', () => {
  const f = fixture();
  f.context.searchComposing.value = true;
  f.context.change('中');
  assert.equal(f.timers.size, 0);
  assert.deepEqual(f.calls, []);
  f.context.change('中国电影');
  f.context.compose();
  assert.equal(f.timers.size, 1);
  assert.equal([...f.timers.values()][0].delay, 1000);
  f.context.compactList.value = true;
  f.context.change('新的电影');
  assert.equal(f.timers.size, 1);
  assert.equal([...f.timers.values()][0].delay, 600);
  f.context.submit();
  assert.equal(f.timers.size, 0);
  assert.equal(f.calls.length, 1);
  assert.deepEqual(f.calls[0].params, { query: '新的电影', media_type: '' });
});

test('changing a type filter cancels the scheduled duplicate and uses the same source and query', () => {
  const f = fixture();
  f.context.isShowingSearchResults.value = true;
  f.context.change('电影');
  f.context.selectedMediaType.value = 'Movie';
  f.context.currentPage.value = 4;
  f.context.filter();
  assert.equal(f.timers.size, 0);
  assert.equal(f.context.currentPage.value, 1);
  assert.deepEqual(f.calls, [{ url: '/api/search_emby_library', params: { query: '电影', media_type: 'Movie' } }]);
});

test('late data cannot change the page after it is unmounted', async () => {
  const pending = deferred(), f = fixture(() => pending.promise);
  const operation = f.context.load();
  f.context.pageActive = false;
  pending.resolve({ data: { items: [{ item_id: 'late' }], total_items: 1 } });
  await operation;
  assert.equal(f.context.tableData.value.length, 0);
  assert.equal(f.context.totalItems.value, 0);
});
