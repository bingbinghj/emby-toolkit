import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('../src/components/LogViewer.vue', import.meta.url), 'utf8');
const handler = name => source.match(new RegExp(`^const ${name} = (?:async )?\\(\\) => \\{[\\s\\S]*?^};`, 'm'))?.[0] || assert.fail(`Missing ${name}`);
const plain = value => JSON.parse(JSON.stringify(value));
const tick = () => new Promise(resolve => setImmediate(resolve));
function fixture() {
  const requests = [];
  let modeChanged, watchOptions;
  const context = vm.createContext({
    readSequence: 0, pageActive: true,
    isLoadingFiles: { value: false }, isLoadingContent: { value: false }, isSearching: { value: false },
    isSearchMode: { value: false }, searchMode: { value: 'context' }, searchQuery: { value: 'JSON' },
    searchResults: { value: [] }, appliedQuery: { value: '' }, errorMessage: { value: '' }, scrollRef: { value: null },
    nextTick: async () => {},
    axios: { get: (path, params) => new Promise((resolve, reject) => requests.push({ path, params: plain(params), resolve, reject })) },
    watch: (_ref, callback, options) => { modeChanged = callback; watchOptions = options; },
  });
  vm.runInContext(`${handler('executeSearch')}\n${handler('clearSearch')}\n${source.match(/^watch\(searchMode,.*$/m)[0]}\nthis.search = executeSearch; this.clear = clearSearch;`, context);
  return { context, requests, get watchOptions() { return watchOptions; }, setMode(value) { context.searchMode.value = value; modeChanged(); } };
}

test('changing the first deferred context search to match invalidates it and starts the selected mode immediately', async () => {
  const f = fixture();
  const first = f.context.search();
  assert.equal(f.requests[0].path, '/api/logs/search_context');
  assert.equal(f.context.isSearchMode.value, false);
  assert.equal(f.context.isSearching.value, true);
  f.setMode('filter');
  assert.equal(f.watchOptions.flush, 'sync');
  assert.equal(f.requests[1].path, '/api/logs/search');
  assert.deepEqual(f.requests[1].params, { params: { q: 'JSON' } });
  f.requests[0].resolve({ data: [{ file: 'old-context.log', lines: ['旧上下文 JSON'] }] });
  await first;
  assert.deepEqual(plain(f.context.searchResults.value), []);
  assert.equal(f.context.isSearching.value, true);
  f.requests[1].resolve({ data: [{ file: 'new-match.log', content: '新匹配 JSON 完整结尾' }] });
  await tick();
  assert.deepEqual(plain(f.context.searchResults.value), [{ file: 'new-match.log', line: '新匹配 JSON 完整结尾' }]);
  assert.equal(f.context.isSearching.value, false);
  assert.equal(f.context.appliedQuery.value, 'JSON');
});

test('switching modes again while a previous replacement is pending accepts only the latest response and error', async () => {
  const f = fixture();
  const initial = f.context.search();
  f.requests[0].resolve({ data: [{ file: 'original.log', lines: ['原上下文 JSON'] }] });
  await initial;
  f.setMode('filter');
  assert.equal(f.requests[1].path, '/api/logs/search');
  f.setMode('context');
  assert.equal(f.requests[2].path, '/api/logs/search_context');
  f.requests[1].reject({ response: { data: { error: '旧匹配失败，不能覆盖新查询' } } });
  await tick();
  assert.equal(f.context.errorMessage.value, '');
  assert.equal(f.context.isSearching.value, true);
  assert.deepEqual(plain(f.context.searchResults.value), [{ file: 'original.log', line: '原上下文 JSON' }]);
  f.requests[2].resolve({ data: [{ file: 'latest.log', lines: ['最新上下文 JSON 完整结尾'] }] });
  await tick();
  assert.deepEqual(plain(f.context.searchResults.value), [{ file: 'latest.log', line: '最新上下文 JSON 完整结尾' }]);
  assert.equal(f.context.isSearching.value, false);
});
