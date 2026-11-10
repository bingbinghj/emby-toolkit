import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../src/components/MediaOrganizePage.vue', import.meta.url), 'utf8');
const movie = { id: '00000000-0000-4000-8000-000000000101', item_id: '101', name: '电影一', item_type: 'Movie', library_name: '电影', status: 'success' };
const result = { selected_count: 1, updated: 1, failed: 0, items: [{ item_id: '101', status: 'completed' }], message: '整理完成' };
const list = { items: [movie], total: 48 };
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
    active: true, itemSequence: 0, pageSize: 24,
    statusFilter: { value: 'all' }, loadedQuery: { value: '[1,"","all"]' },
    items: { value: [movie] }, search: { value: '' }, appliedSearch: { value: '' }, page: { value: 1 }, total: { value: 48 },
    selectedItems: { value: [] }, confirmationItems: { value: [] }, confirmationOpen: { value: false },
    loadingItems: { value: false }, itemsError: { value: '' }, processing: { value: false }, feedback: { value: null },
    axios: {
      get: async (url, options) => { calls.push({ method: 'GET', url, options: options && clone(options) }); return get(url, options); },
      post: async (url, body) => { calls.push({ method: 'POST', url, body: clone(body) }); return post(url, body); },
    },
    message: Object.fromEntries(['success', 'warning', 'error'].map(type => [type, text => messages.push({ type, text })])),
  });
  const select = source.match(/const queryKey = .*;/)[0] + '\n' + source.match(/const canSelectItems = .*;/)[0] + '\n' + source.match(/const isReadyRecord = .*;/)[0] + '\n' + source.match(/const canSelectRecord = .*;/)[0];
  vm.runInContext(select + '\n' + ['loadItems', 'updateSelection', 'openConfirmation', 'reorganize'].map(functionSource).join('\n') + '\nthis.load = loadItems; this.select = updateSelection; this.confirm = openConfirmation; this.run = reorganize;', context);
  return { context, calls, messages };
}

test('pagination reads ETK admission history without requesting a whole library', async () => {
  const f = fixture();
  f.context.page.value = 2;
  f.context.appliedSearch.value = '电影';
  await f.context.load();
  assert.equal(f.calls[0].url, '/api/media_organize/history');
  assert.deepEqual(f.calls[0].options.params, { page: 2, page_size: 24, search: '电影', status: 'all' });
});

test('selection accumulates across pages and only deselects visible items', () => {
  const f = fixture();
  f.context.select(['00000000-0000-4000-8000-000000000101']);
  f.context.items.value = [{ id: '00000000-0000-4000-8000-000000000202', item_id: '202', name: '电影二', item_type: 'Movie', status: 'success' }];
  f.context.select(['00000000-0000-4000-8000-000000000202']);
  assert.deepEqual(clone(f.context.selectedItems.value).map(item => item.item_id), ['101', '202']);
  f.context.select([]);
  assert.deepEqual(clone(f.context.selectedItems.value).map(item => item.item_id), ['101']);
});

test('an unreadable page retains earlier selection and cannot deselect stale rows', async () => {
  const f = fixture({ get: async () => { throw { response: { data: { error: 'Emby 读取失败' } } }; } });
  f.context.select(['00000000-0000-4000-8000-000000000101']);
  await f.context.load();
  f.context.select([]);
  assert.equal(f.context.itemsError.value, 'Emby 读取失败');
  assert.equal(f.context.selectedItems.value[0].item_id, '101');
  assert.equal(f.calls.filter(call => call.method === 'POST').length, 0);
});

test('late recent data from an earlier search cannot replace a newer result', async () => {
  const pending = deferred();
  const f = fixture({ get: async (_url, options) => options.params.search === '' ? pending.promise : { data: { items: [{ id: '00000000-0000-4000-8000-000000000303', item_id: '303', item_type: 'Episode', name: '新剧集', status: 'success' }], total: 1 } } });
  const old = f.context.load();
  f.context.appliedSearch.value = '新剧集';
  await f.context.load();
  pending.resolve({ data: list });
  await old;
  assert.equal(f.context.items.value[0].item_id, '303');
  assert.equal(f.context.loadedQuery.value, '[1,"新剧集","all"]');
});

test('recent episodes are selected by their exact ID instead of expanding to the series', async () => {
  const episode = { id: '00000000-0000-4000-8000-000000000005', item_id: 'episode-5', item_type: 'Episode', name: '最近单集', series_id: 'whole-series', status: 'success' };
  const f = fixture({ get: async () => ({ data: { items: [episode], total: 1 } }) });
  await f.context.load();
  f.context.select(['00000000-0000-4000-8000-000000000005']);
  f.context.confirm();
  await f.context.run();
  const body = f.calls.find(call => call.method === 'POST').body;
  assert.deepEqual(body, { item_ids: ['episode-5'], history_ids: ['00000000-0000-4000-8000-000000000005'] });
  assert.equal(body.item_ids.includes('whole-series'), false);
});

for (const items of [[{ id: '00000000-0000-4000-8000-000000000404', item_id: 'series', item_type: 'Series' }], [movie, movie]]) {
  test('invalid history media types or duplicate record identities cannot become selectable', async () => {
    const f = fixture({ get: async () => ({ data: { items, total: items.length } }) });
    await f.context.load();
    assert.equal(f.context.itemsError.value, '入库处理记录身份或类型无效');
    f.context.select(['00000000-0000-4000-8000-000000000101']);
    assert.deepEqual(clone(f.context.selectedItems.value), []);
  });
}

test('confirmation captures the exact selected IDs rather than a changed later selection', async () => {
  const f = fixture();
  f.context.select(['00000000-0000-4000-8000-000000000101']);
  f.context.confirm();
  f.context.selectedItems.value.push({ id: '00000000-0000-4000-8000-000000000303', item_id: '303', name: '后选剧集', item_type: 'Episode', status: 'success' });
  await f.context.run();
  assert.deepEqual(f.calls[0], { method: 'POST', url: '/api/actions/reorganize', body: { item_ids: ['101'], history_ids: ['00000000-0000-4000-8000-000000000101'] } });
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
    f.context.select(['00000000-0000-4000-8000-000000000101']); f.context.confirm();
    await f.context.run();
    assert.equal(f.context.feedback.value.type, expected);
    if (!data.items.length) assert.equal(f.context.feedback.value.message, '媒体整理结果尚未确认');
    assert.equal(f.context.selectedItems.value[0].item_id, '101');
  });
}

test('a failed or cancelled task retains its returned details and selection', async () => {
  const f = fixture({ post: async () => { throw { response: { data: { error: '任务已取消', failures: ['101：已执行的处理失败'], warning_details: ['202：尚未开始'] } } }; } });
  f.context.select(['00000000-0000-4000-8000-000000000101']); f.context.confirm();
  await f.context.run();
  assert.equal(f.context.feedback.value.message, '任务已取消');
  assert.deepEqual(clone(f.context.feedback.value.details), ['101：已执行的处理失败', '202：尚未开始']);
  assert.equal(f.context.selectedItems.value.length, 1);
  assert.equal(f.messages.at(-1).type, 'error');
});

test('duplicate confirmation clicks submit one job', async () => {
  const pending = deferred(), f = fixture({ post: () => pending.promise });
  f.context.select(['00000000-0000-4000-8000-000000000101']); f.context.confirm();
  const running = f.context.run();
  await f.context.run();
  assert.equal(f.calls.filter(call => call.method === 'POST').length, 1);
  pending.resolve({ status: 200, data: result });
  await running;
});

test('a late operation response cannot show completion after leaving the page', async () => {
  const pending = deferred(), f = fixture({ post: () => pending.promise });
  f.context.select(['00000000-0000-4000-8000-000000000101']); f.context.confirm();
  const running = f.context.run();
  f.context.active = false;
  pending.resolve({ status: 200, data: result });
  await running;
  assert.equal(f.context.feedback.value, null);
  assert.deepEqual(f.messages, []);
});


test('multiple historical events for one media item submit one organize operation', async () => {
  const second = { ...movie, id: '00000000-0000-4000-8000-000000000111' };
  const f = fixture({ get: async () => ({ data: { items: [movie, second], total: 2 } }) });
  await f.context.load();
  f.context.select(['00000000-0000-4000-8000-000000000101', '00000000-0000-4000-8000-000000000111']);
  f.context.confirm();
  await f.context.run();
  assert.deepEqual(f.calls.find(call => call.method === 'POST').body, { item_ids: ['101'], history_ids: ['00000000-0000-4000-8000-000000000101', '00000000-0000-4000-8000-000000000111'] });
  assert.equal(f.context.feedback.value.type, 'success');
});

for (const record of [{ ...movie, status: 'processing' }, { ...movie, item_id: null }]) {
  test('an unfinished or unbound history record cannot submit work', async () => {
    const f = fixture({ get: async () => ({ data: { items: [record], total: 1 } }) });
    await f.context.load();
    f.context.select(['00000000-0000-4000-8000-000000000101']);
    f.context.confirm();
    await f.context.run();
    assert.deepEqual(clone(f.context.selectedItems.value), []);
    assert.equal(f.calls.some(call => call.method === 'POST'), false);
  });
}

test('empty history does not synthesize records from the retired tracking timestamp', async () => {
  const f = fixture({ get: async () => ({ data: { items: [], total: 0, tracking_since: '2026-11-10T03:00:00Z', legacy_records_available: false } }) });
  await f.context.load();
  assert.ok(!source.includes('organize-tracking'));
  assert.ok(!source.includes('此前入库的历史记录不可用'));
  assert.equal(f.context.items.value.length, 0);
  assert.equal(f.calls.filter(call => call.method === 'GET').length, 1);
});


test('verified organize results reload actual history while retaining selected records', async () => {
  const f = fixture({ get: async () => ({ data: { items: [{ ...movie, status: 'warning', message: '媒体已整理，图片待确认' }], total: 1 } }) });
  f.context.select(['00000000-0000-4000-8000-000000000101']); f.context.confirm();
  await f.context.run();
  assert.equal(f.calls.filter(call => call.method === 'GET').length, 1);
  assert.equal(f.context.selectedItems.value[0].status, 'warning');
  assert.equal(f.context.items.value[0].message, '媒体已整理，图片待确认');
});

test('unverified organize results do not claim the history was updated', async () => {
  const f = fixture({ post: async () => ({ status: 200, data: { ...result, items: [] } }) });
  f.context.select(['00000000-0000-4000-8000-000000000101']); f.context.confirm();
  await f.context.run();
  assert.equal(f.calls.filter(call => call.method === 'GET').length, 0);
  assert.equal(f.context.selectedItems.value[0].status, 'success');
  assert.equal(f.context.feedback.value.message, '媒体整理结果尚未确认');
});

test('history search ignores composing input and coalesces typed queries using MP delays', async () => {
  const f = fixture(), timers = new Map();
  let nextTimer = 0;
  Object.assign(f.context, {
    searchComposing: { value: true }, smAndDown: { value: false }, searchTimer: null,
    setTimeout: (callback, delay) => { const id = ++nextTimer; timers.set(id, { callback, delay }); return id; },
    clearTimeout: id => timers.delete(id),
  });
  vm.runInContext(source.match(/const searchItems = .*;/)[0] + '\n' + functionSource('scheduleSearch') + '\n' + source.match(/const finishSearchComposition = .*;/)[0] + '\nthis.schedule = scheduleSearch; this.submit = searchItems; this.finishComposition = finishSearchComposition;', f.context);
  f.context.search.value = '中';
  f.context.schedule();
  f.context.submit();
  assert.equal(timers.size, 0);
  assert.deepEqual(f.calls, []);
  f.context.search.value = '中国电影';
  f.context.finishComposition();
  assert.equal(timers.size, 1);
  assert.equal([...timers.values()][0].delay, 1000);
  f.context.search.value = '下一部电影';
  f.context.smAndDown.value = true;
  f.context.schedule();
  assert.equal(timers.size, 1);
  assert.equal([...timers.values()][0].delay, 600);
  f.context.submit();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(timers.size, 0);
  assert.equal(f.calls.length, 1);
  assert.deepEqual(f.calls[0].options.params, { page: 1, page_size: 24, search: '下一部电影', status: 'all' });
});
