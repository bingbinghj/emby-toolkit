import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { queueReceiptMatches } from '../src/composables/useMediaOrganizeQueue.js';

const source = readFileSync(new URL('../src/components/MediaOrganizePage.vue', import.meta.url), 'utf8');
const movie = { id: '00000000-0000-4000-8000-000000000101', item_id: '101', name: '电影一', item_type: 'Movie', library_name: '电影', status: 'success' };
const result = { selected_count: 1, updated: 1, failed: 0, items: [{ item_id: '101', status: 'completed' }], message: '整理完成' };
const list = { items: [movie], total: 100 };
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
    active: true, itemSequence: 0, pageSize: { value: 50 }, pageSizeOptions: [25, 50, 100, 500, 1000], queueReceiptMatches,
    computed: fn => ({ get value() { return fn(); } }),
    statusFilter: { value: 'all' }, loadedQuery: { value: '[1,50,"","all"]' },
    items: { value: [movie] }, search: { value: '' }, appliedSearch: { value: '' }, page: { value: 1 }, total: { value: 100 },
    selectedItems: { value: [] }, confirmationItems: { value: [] }, confirmationOpen: { value: false }, confirmationMode: { value: 'reprocess' }, queueOpen: { value: false },
    activeQueueItems: { value: new Set() }, queue: { refresh: async () => {} },
    loadingItems: { value: false }, itemsError: { value: '' }, processing: { value: false }, feedback: { value: null },
    axios: {
      get: async (url, options) => { calls.push({ method: 'GET', url, options: options && clone(options) }); return get(url, options); },
      post: async (url, body) => { calls.push({ method: 'POST', url, body: clone(body) }); return post(url, body); },
    },
    message: Object.fromEntries(['success', 'info', 'warning', 'error'].map(type => [type, text => messages.push({ type, text })])),
  });
  const select = source.match(/const queryKey = .*;/)[0] + '\n' + source.match(/const canSelectItems = .*;/)[0] + '\n' + source.match(/const isReadyRecord = .*;/)[0] + '\n' + source.match(/const canSelectRecord = .*;/)[0];
  vm.runInContext(select + '\n' + source.match(/const pageTip = .*;/)[0] + '\n' + source.match(/const pages = .*;/)[0] + '\n' + ['loadItems', 'changePageSize', 'updateSelection', 'openConfirmation', 'reorganize'].map(functionSource).join('\n') + '\nthis.load = loadItems; this.resizePage = changePageSize; this.select = updateSelection; this.confirm = openConfirmation; this.run = reorganize; this.pageTip = pageTip; this.pages = pages;', context);
  return { context, calls, messages };
}

test('pagination reads ETK admission history without requesting a whole library', async () => {
  const f = fixture();
  f.context.page.value = 2;
  f.context.appliedSearch.value = '电影';
  await f.context.load();
  assert.equal(f.calls[0].url, '/api/media_organize/history');
  assert.deepEqual(f.calls[0].options.params, { page: 2, page_size: 50, search: '电影', status: 'all' });
});

test('changing to MP 1000-record pagination reloads page one and preserves selected media', async () => {
  const f = fixture();
  f.context.select([movie.id]);
  f.context.page.value = 2;
  await f.context.resizePage(1000);
  assert.equal(f.context.page.value, 1);
  assert.deepEqual(f.calls[0].options.params, { page: 1, page_size: 1000, search: '', status: 'all' });
  assert.deepEqual(clone(f.context.selectedItems.value).map(item => item.item_id), ['101']);
});

test('a response from the former page size cannot replace or unlock the newly requested page', async () => {
  const pending = deferred(), resized = { ...movie, id: 'resized-record', item_id: 'resized-media' };
  const f = fixture({ get: async (_url, options) => options.params.page_size === 50 ? pending.promise : { data: { items: [resized], total: 200 } } });
  const old = f.context.load();
  await f.context.resizePage(100);
  pending.resolve({ data: list });
  await old;
  assert.equal(f.context.items.value[0].item_id, 'resized-media');
  assert.equal(f.context.loadedQuery.value, '[1,100,"","all"]');
  assert.equal(f.context.loadingItems.value, false);
  f.context.select(['resized-record']);
  assert.deepEqual(clone(f.context.selectedItems.value).map(item => item.item_id), ['resized-media']);
});

test('page-size identity disables stale row selection even before the replacement load starts', () => {
  const f = fixture();
  f.context.pageSize.value = 1000;
  f.context.select([movie.id]);
  assert.equal(f.context.selectedItems.value.length, 0);
});

test('queue-style history refresh retains the chosen page size and goes back to the last available page', async () => {
  const f = fixture({ get: async (_url, options) => ({ data: { items: options.params.page === 1 ? [movie] : [], total: 1 } }) });
  f.context.pageSize.value = 1000;
  f.context.page.value = 3;
  await f.context.load();
  assert.deepEqual(f.calls.map(call => call.options.params.page), [3, 1]);
  assert.equal(f.context.pageSize.value, 1000);
  assert.equal(f.context.items.value[0].item_id, '101');
  assert.equal(f.context.loadedQuery.value, '[1,1000,"","all"]');
});

test('MP range ends at the filtered total and an empty page has range zero without extra requests', async () => {
  const f = fixture({ get: async () => ({ data: { items: [], total: 0 } }) });
  f.context.page.value = 2;
  f.context.total.value = 73;
  assert.deepEqual(clone(f.context.pageTip.value), { begin: 51, end: 73 });
  f.context.page.value = 1;
  await f.context.load();
  assert.deepEqual(clone(f.context.pageTip.value), { begin: 0, end: 0 });
  assert.equal(f.context.pages.value, 1);
  assert.equal(f.calls.length, 1);
});

test('all 1000 real rows can be selected and submitted in one confirmed queue operation', async () => {
  const records = Array.from({ length: 1000 }, (_, index) => ({ ...movie, id: `00000000-0000-4000-8000-${String(index).padStart(12, '0')}`, item_id: `media-${index}` }));
  const f = fixture({ get: async () => ({ data: { items: records, total: 1000 } }), post: async (_url, body) => ({ status: 202, data: {
    mode: body.mode, queued_count: body.item_ids.length, duplicate_count: 0, queue_ids: body.item_ids.map(id => `queue-${id}`),
    items: body.item_ids.map(item_id => ({ id: `queue-${item_id}`, item_id, mode: body.mode, status: 'queued' })),
  } }) });
  await f.context.resizePage(1000);
  f.context.select(records.map(item => item.id));
  f.context.confirm();
  await f.context.run();
  const submissions = f.calls.filter(call => call.method === 'POST');
  assert.equal(submissions.length, 1);
  assert.equal(submissions[0].body.item_ids.length, 1000);
  assert.equal(submissions[0].body.history_ids.length, 1000);
  assert.equal(f.context.feedback.value.type, 'info');
  assert.equal(f.context.selectedItems.value.length, 1000);
});

test('search and state filters reset to page one while preserving the chosen page size', async () => {
  const f = fixture();
  Object.assign(f.context, { searchComposing: { value: false }, searchTimer: null, clearTimeout() {} });
  vm.runInContext(source.match(/const searchItems = .*;/)[0] + '\nthis.searchItems = searchItems;', f.context);
  f.context.pageSize.value = 500;
  f.context.page.value = 2;
  f.context.search.value = '凡人修仙传';
  f.context.searchItems();
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(f.calls[0].options.params, { page: 1, page_size: 500, search: '凡人修仙传', status: 'all' });
  f.context.watch = (_target, callback) => { f.context.filterChanged = callback; };
  vm.runInContext(source.match(/watch\(statusFilter, .*\);/)[0], f.context);
  f.context.page.value = 5;
  f.context.statusFilter.value = 'failed';
  f.context.filterChanged();
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(f.calls[1].options.params, { page: 1, page_size: 500, search: '凡人修仙传', status: 'failed' });
});

test('unsupported sizes and sizes changed during submission do not issue requests', async () => {
  const f = fixture();
  await f.context.resizePage(30);
  f.context.processing.value = true;
  await f.context.resizePage(1000);
  assert.equal(f.context.pageSize.value, 50);
  assert.equal(f.calls.length, 0);
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
  assert.equal(f.context.loadedQuery.value, '[1,50,"新剧集","all"]');
});

test('recent episodes are selected by their exact ID instead of expanding to the series', async () => {
  const episode = { id: '00000000-0000-4000-8000-000000000005', item_id: 'episode-5', item_type: 'Episode', name: '最近单集', series_id: 'whole-series', status: 'success' };
  const f = fixture({ get: async () => ({ data: { items: [episode], total: 1 } }) });
  await f.context.load();
  f.context.select(['00000000-0000-4000-8000-000000000005']);
  f.context.confirm();
  await f.context.run();
  const body = f.calls.find(call => call.method === 'POST').body;
  assert.deepEqual(body, { item_ids: ['episode-5'], history_ids: ['00000000-0000-4000-8000-000000000005'], mode: 'reprocess' });
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
  assert.deepEqual(f.calls[0], { method: 'POST', url: '/api/actions/reorganize', body: { item_ids: ['101'], history_ids: ['00000000-0000-4000-8000-000000000101'], mode: 'reprocess' } });
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
  assert.deepEqual(f.calls.find(call => call.method === 'POST').body, { item_ids: ['101'], history_ids: ['00000000-0000-4000-8000-000000000101', '00000000-0000-4000-8000-000000000111'], mode: 'reprocess' });
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
  assert.deepEqual(f.calls[0].options.params, { page: 1, page_size: 50, search: '下一部电影', status: 'all' });
});

const acceptedQueue = (itemId = '101', mode = 'reprocess') => ({ mode, queued_count: 1, duplicate_count: 0,
  queue_ids: ['queue-accepted'], items: [{ id: 'queue-accepted', item_id: itemId, mode, status: 'queued' }], message: '已加入整理队列' });

test('an accepted queue response releases the form without claiming the media finished', async () => {
  const f = fixture({ post: async () => ({ status: 202, data: acceptedQueue() }) });
  f.context.select(['00000000-0000-4000-8000-000000000101']); f.context.confirm();
  await f.context.run();
  assert.equal(f.context.processing.value, false);
  assert.equal(f.context.confirmationOpen.value, false);
  assert.equal(f.context.queueOpen.value, true);
  assert.equal(f.context.feedback.value.type, 'info');
  assert.equal(f.context.feedback.value.message, '已加入整理队列');
  assert.equal(f.calls.some(call => call.url.startsWith('/api/tasks/result/')), false);
});

test('rescrape freezes its selected episode and mode before awaiting queue acceptance', async () => {
  const pending = deferred();
  const episode = { ...movie, item_id: 'episode-5', item_type: 'Episode', series_id: 'whole-series' };
  const f = fixture({ get: async () => ({ data: { items: [episode], total: 1 } }), post: () => pending.promise });
  await f.context.load(); f.context.select([movie.id]); f.context.confirm();
  f.context.confirmationMode.value = 'rescrape';
  const work = f.context.run();
  f.context.confirmationMode.value = 'reprocess';
  pending.resolve({ status: 202, data: acceptedQueue('episode-5', 'rescrape') }); await work;
  assert.deepEqual(f.calls.find(call => call.method === 'POST').body, { item_ids: ['episode-5'], history_ids: [movie.id], mode: 'rescrape' });
  assert.equal(f.context.feedback.value.type, 'info');
});

test('an incomplete queue receipt cannot claim that every selected item was accepted', async () => {
  const f = fixture({ post: async () => ({ status: 202, data: { ...acceptedQueue(), queue_ids: [] } }) });
  f.context.select([movie.id]); f.context.confirm(); await f.context.run();
  assert.equal(f.context.feedback.value.type, 'warning');
  assert.equal(f.context.feedback.value.message, '整理队列提交结果尚未确认');
  assert.equal(f.context.selectedItems.value.length, 1);
});

test('a queue receipt for a different mode or an already finished task cannot confirm acceptance', async () => {
  for (const changes of [{ mode: 'rescrape' }, { status: 'completed' }]) {
    const receipt = acceptedQueue();
    receipt.items[0] = { ...receipt.items[0], ...changes };
    const f = fixture({ post: async () => ({ status: 202, data: receipt }) });
    f.context.select([movie.id]); f.context.confirm(); await f.context.run();
    assert.equal(f.context.feedback.value.type, 'warning');
    assert.equal(f.context.feedback.value.message, '整理队列提交结果尚未确认');
  }
});

test('queue acceptance unlocks the form even while its queue refresh is slow', async () => {
  const pending = deferred();
  const f = fixture({ post: async () => ({ status: 202, data: acceptedQueue() }) });
  f.context.queue.refresh = () => pending.promise;
  f.context.select([movie.id]); f.context.confirm(); await f.context.run();
  assert.equal(f.context.processing.value, false);
  assert.equal(f.context.feedback.value.message, '已加入整理队列');
  pending.resolve();
});
