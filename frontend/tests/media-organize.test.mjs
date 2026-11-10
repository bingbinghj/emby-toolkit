import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../src/components/MediaOrganizePage.vue', import.meta.url), 'utf8');
const movie = { Id: '101', Name: '电影一', Type: 'Movie', LibraryName: '电影' };
const result = { selected_count: 1, updated: 1, failed: 0, items: [{ item_id: '101', status: 'completed' }], message: '整理完成' };
const list = { library: { id: 'movies', name: '电影' }, items: [movie], total: 48 };
const clone = value => JSON.parse(JSON.stringify(value));
function deferred() { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
function functionSource(name) {
  const start = source.indexOf(`const ${name} = `);
  const end = source.indexOf('\n};', start) + 4;
  assert.ok(start >= 0 && end > start, name);
  return source.slice(start, end);
}
function fixture({ get = async () => ({ data: list }), post = async () => ({ status: 200, data: result }) } = {}) {
  const calls = [], messages = [];
  const context = vm.createContext({
    active: true, librarySequence: 0, itemSequence: 0, pageSize: 24,
    libraries: { value: [{ id: 'movies', name: '电影' }] }, libraryId: { value: 'movies' }, loadedLibrary: { value: { id: 'movies', name: '电影' } },
    loadingLibraries: { value: false }, librariesError: { value: '' },
    items: { value: [movie] }, search: { value: '' }, appliedSearch: { value: '' }, page: { value: 1 }, total: { value: 48 },
    selectedItems: { value: [] }, confirmationItems: { value: [] }, confirmationOpen: { value: false },
    loadingItems: { value: false }, itemsError: { value: '' }, processing: { value: false }, feedback: { value: null },
    axios: {
      get: async (url, options) => { calls.push({ method: 'GET', url, options: options && clone(options) }); return get(url, options); },
      post: async (url, body) => { calls.push({ method: 'POST', url, body: clone(body) }); return post(url, body); },
    },
    message: Object.fromEntries(['success', 'warning', 'error'].map(type => [type, text => messages.push({ type, text })])),
  });
  const select = source.match(/const canSelectItems = .*;/)[0];
  vm.runInContext(select + '\n' + ['loadLibraries', 'loadItems', 'updateSelection', 'openConfirmation', 'reorganize'].map(functionSource).join('\n') + '\nthis.load = loadItems; this.loadLibraries = loadLibraries; this.select = updateSelection; this.confirm = openConfirmation; this.run = reorganize;', context);
  return { context, calls, messages };
}

test('pagination sends primitive page values and the exact selected library', async () => {
  const f = fixture();
  f.context.page.value = 2;
  f.context.appliedSearch.value = '电影';
  await f.context.load();
  assert.equal(f.calls[0].url, '/api/media_center/libraries/movies/items');
  assert.deepEqual(f.calls[0].options.params, { page: 2, page_size: 24, start_index: 24, search: '电影' });
});

test('selection accumulates across pages and only deselects visible items', () => {
  const f = fixture();
  f.context.select(['101']);
  f.context.items.value = [{ Id: '202', Name: '电影二', Type: 'Movie' }];
  f.context.select(['202']);
  assert.deepEqual(clone(f.context.selectedItems.value).map(item => item.Id), ['101', '202']);
  f.context.select([]);
  assert.deepEqual(clone(f.context.selectedItems.value).map(item => item.Id), ['101']);
});

test('an unreadable page retains earlier selection and cannot deselect stale rows', async () => {
  const f = fixture({ get: async () => { throw { response: { data: { error: 'Emby 读取失败' } } }; } });
  f.context.select(['101']);
  await f.context.load();
  f.context.select([]);
  assert.equal(f.context.itemsError.value, 'Emby 读取失败');
  assert.equal(f.context.selectedItems.value[0].Id, '101');
  assert.equal(f.calls.filter(call => call.method === 'POST').length, 0);
});

test('late data from an earlier library cannot replace a newer library result', async () => {
  const pending = deferred();
  const f = fixture({ get: async url => url.includes('/movies/') ? pending.promise : { data: { library: { id: 'tv', name: '电视剧' }, items: [{ Id: '303', Type: 'Series', Name: '剧集' }], total: 1 } } });
  const old = f.context.load();
  f.context.libraryId.value = 'tv';
  f.context.libraries.value.push({ id: 'tv', name: '电视剧' });
  await f.context.load();
  pending.resolve({ data: list });
  await old;
  assert.equal(f.context.items.value[0].Id, '303');
  assert.equal(f.context.loadedLibrary.value.id, 'tv');
});

test('confirmation captures the exact selected IDs rather than a changed later selection', async () => {
  const f = fixture();
  f.context.select(['101']);
  f.context.confirm();
  f.context.selectedItems.value.push({ Id: '303', Name: '后选剧集', Type: 'Series', LibraryName: '电视剧' });
  await f.context.run();
  assert.deepEqual(f.calls[0], { method: 'POST', url: '/api/actions/reorganize', body: { item_ids: ['101'] } });
  assert.equal(f.context.feedback.value.type, 'success');
});

for (const [data, status, expected] of [
  [{ ...result, failed: 1, items: [{ item_id: '101', status: 'failed' }], failures: ['101：原生恢复失败'] }, 207, 'error'],
  [{ ...result, warnings: 1, warning_details: ['101：图片未完成'] }, 207, 'warning'],
  [{ ...result, warning_details: ['缓存未完成'] }, 200, 'warning'],
  [{ ...result, items: [] }, 200, 'warning'],
]) {
  test(`the actual final response retains ${expected} feedback without false completion`, async () => {
    const f = fixture({ post: async () => ({ status, data }) });
    f.context.select(['101']); f.context.confirm();
    await f.context.run();
    assert.equal(f.context.feedback.value.type, expected);
    if (!data.items.length) assert.equal(f.context.feedback.value.message, '媒体整理结果尚未确认');
    assert.equal(f.context.selectedItems.value[0].Id, '101');
  });
}

test('a failed or cancelled task retains its returned details and selection', async () => {
  const f = fixture({ post: async () => { throw { response: { data: { error: '任务已取消', failures: ['101：已执行的处理失败'], warning_details: ['202：尚未开始'] } } }; } });
  f.context.select(['101']); f.context.confirm();
  await f.context.run();
  assert.equal(f.context.feedback.value.message, '任务已取消');
  assert.deepEqual(clone(f.context.feedback.value.details), ['101：已执行的处理失败', '202：尚未开始']);
  assert.equal(f.context.selectedItems.value.length, 1);
  assert.equal(f.messages.at(-1).type, 'error');
});

test('duplicate confirmation clicks submit one job', async () => {
  const pending = deferred(), f = fixture({ post: () => pending.promise });
  f.context.select(['101']); f.context.confirm();
  const running = f.context.run();
  await f.context.run();
  assert.equal(f.calls.length, 1);
  pending.resolve({ status: 200, data: result });
  await running;
});

test('a late operation response cannot show completion after leaving the page', async () => {
  const pending = deferred(), f = fixture({ post: () => pending.promise });
  f.context.select(['101']); f.context.confirm();
  const running = f.context.run();
  f.context.active = false;
  pending.resolve({ status: 200, data: result });
  await running;
  assert.equal(f.context.feedback.value, null);
  assert.deepEqual(f.messages, []);
});
