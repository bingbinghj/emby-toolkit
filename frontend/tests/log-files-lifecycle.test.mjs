import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { computed, ref } from 'vue';
import { matchesLogRecord } from '../src/utils/logRecords.js';

const source = readFileSync(new URL('../src/components/LogViewer.vue', import.meta.url), 'utf8');
function definition(name, declaration = 'const') {
  const start = source.indexOf(declaration === 'const' ? `const ${name} = ` : `${declaration} ${name}(`);
  assert.ok(start >= 0, name);
  const ending = declaration === 'const' ? '\n};' : '\n}';
  return source.slice(start, source.indexOf(ending, start) + ending.length);
}
const plain = value => JSON.parse(JSON.stringify(value));
function fixture(get) {
  const requests = [];
  const context = vm.createContext({
    readSequence: 0, fileSequence: 0, pageActive: true,
    isLoadingFiles: ref(false), isLoadingContent: ref(false), isSearching: ref(false), isSearchMode: ref(false),
    logFiles: ref(['app.log']), selectedFile: ref('app.log'),
    logContent: ref([{ line: 'cached A content', file: 'app.log' }]),
    searchResults: ref([]), searchQuery: ref('query'), appliedQuery: ref(''), searchMode: ref('context'), levelFilter: ref('all'),
    pageInput: ref(2), loadedFile: ref('app.log'), logPage: ref(2), logPages: ref(3), logTotalLines: ref(1200), errorMessage: ref(''), scrollRef: ref(null),
    nextTick: async () => {}, computed, matchesLogRecord, Blob,
    axios: { get: async (path, options) => { requests.push({ path, params: plain(options?.params || {}) }); return get(path, options); } },
  });
  context.isLoading = computed(() => context.isLoadingFiles.value || context.isLoadingContent.value || context.isSearching.value);
  const displayed = source.match(/^const displayedRecords = computed\([\s\S]*?\);\n/m)[0];
  vm.runInContext(`${displayed}\n${definition('fetchLogFiles')}\n${definition('fetchLogContent')}\n${definition('executeSearch')}\n${definition('clearSearch')}\n${definition('refreshLogs', 'async function')}\n${definition('changePage')}\nthis.refresh = refreshLogs; this.load = fetchLogContent; this.search = executeSearch; this.clear = clearSearch; this.changePage = changePage;`, context);
  return { context, requests };
}

test('failed log list refresh preserves its error and existing content until a real retry', async () => {
  let unavailable = true;
  const f = fixture(async path => {
    if (path === '/api/logs/list') {
      if (unavailable) throw { response: { status: 503, data: { error: 'file list unavailable' } } };
      return { data: ['app.log', 'app.log.1'] };
    }
    return { data: { lines: ['fresh A content'], page: 1, pages: 3, total_lines: 1201 } };
  });
  await f.context.refresh();
  assert.deepEqual(f.requests.map(request => request.path), ['/api/logs/list']);
  assert.equal(f.context.errorMessage.value, 'file list unavailable');
  assert.deepEqual(plain(f.context.logContent.value), [{ line: 'cached A content', file: 'app.log' }]);
  assert.equal(f.context.selectedFile.value, 'app.log');
  assert.equal(f.context.logPage.value, 2);
  assert.equal(f.context.isLoading.value, false);
  unavailable = false;
  await f.context.refresh();
  assert.deepEqual(f.requests.map(request => request.path), ['/api/logs/list', '/api/logs/list', '/api/logs/view']);
  assert.deepEqual(f.requests[2].params, { filename: 'app.log', page: 1, format: 'json' });
  assert.equal(f.context.errorMessage.value, '');
  assert.deepEqual(plain(f.context.logContent.value), [{ line: 'fresh A content', file: 'app.log' }]);
});

test('a file list response after unmount cannot start a second log-content request', async () => {
  let release;
  const pending = new Promise(resolve => { release = resolve; });
  const f = fixture(async path => path === '/api/logs/list' ? pending : { data: { lines: [], page: 1, pages: 1, total_lines: 0 } });
  const refreshing = f.context.refresh();
  f.context.pageActive = false; ++f.context.readSequence; ++f.context.fileSequence;
  release({ data: ['app.log', 'app.log.1'] }); await refreshing;
  assert.deepEqual(f.requests.map(request => request.path), ['/api/logs/list']);
  assert.deepEqual(plain(f.context.logContent.value), [{ line: 'cached A content', file: 'app.log' }]);
  assert.equal(f.context.errorMessage.value, '');
});

test('a successful refreshed list reads its confirmed replacement file exactly once', async () => {
  const f = fixture(async path => path === '/api/logs/list' ? { data: ['app.log.1'] }
    : { data: { lines: ['replacement B content'], page: 1, pages: 1, total_lines: 1 } });
  await f.context.refresh();
  assert.equal(f.context.selectedFile.value, 'app.log.1');
  assert.deepEqual(f.requests.map(request => request.path), ['/api/logs/list', '/api/logs/view']);
  assert.deepEqual(f.requests[1].params, { filename: 'app.log.1', page: 1, format: 'json' });
  assert.deepEqual(plain(f.context.logContent.value), [{ line: 'replacement B content', file: 'app.log.1' }]);
  assert.equal(f.context.isLoading.value, false);
});

test('an empty refreshed file list clears the cached page and pagination metadata', async () => {
  const f = fixture(async () => ({ data: [] }));
  await f.context.refresh();
  assert.deepEqual(f.requests.map(request => request.path), ['/api/logs/list']);
  assert.equal(f.context.selectedFile.value, null);
  assert.deepEqual(plain(f.context.logContent.value), []);
  assert.equal(f.context.logPage.value, 1);
  assert.equal(f.context.logPages.value, 1);
  assert.equal(f.context.logTotalLines.value, 0);
});

test('page navigation reads the requested page once and leaves the last successful page on failure', async () => {
  let fail = true;
  const f = fixture(async (_path, options) => {
    if (fail) throw { response: { data: { error: 'page unavailable' } } };
    return { data: { lines: ['page three'], page: options.params.page, pages: 3, total_lines: 1200 } };
  });
  f.context.pageInput.value = 3;
  await f.context.changePage(3);
  assert.equal(f.context.logPage.value, 2);
  assert.equal(f.context.pageInput.value, 2);
  assert.equal(f.context.logContent.value[0].line, 'cached A content');
  fail = false;
  await f.context.changePage('3');
  assert.equal(f.context.logPage.value, 3);
  assert.equal(f.context.pageInput.value, 3);
  assert.equal(f.context.logContent.value[0].line, 'page three');
  assert.equal(f.requests.length, 2);
});

for (const value of [0, -1, 4, 1.5, '', 'abc', 2]) {
  test(`invalid or current page ${JSON.stringify(value)} does not request logs`, async () => {
    const f = fixture(() => { throw new Error('unexpected request'); });
    f.context.pageInput.value = value;
    await f.context.changePage(value);
    assert.equal(f.requests.length, 0);
    assert.equal(f.context.pageInput.value, 2);
  });
}

test('a failed file switch cannot paginate using the previous file total', async () => {
  const f = fixture(() => { throw new Error('unavailable'); });
  f.context.selectedFile.value = 'app.log.1';
  await f.context.load('app.log.1');
  await f.context.changePage(3);
  assert.equal(f.requests.length, 1);
  assert.equal(f.context.loadedFile.value, 'app.log');
  assert.equal(f.context.logPage.value, 2);
});
