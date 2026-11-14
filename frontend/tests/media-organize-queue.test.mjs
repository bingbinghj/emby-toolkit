import assert from 'node:assert/strict';
import test from 'node:test';
import { useMediaOrganizeQueue } from '../src/composables/useMediaOrganizeQueue.js';

const item = (status = 'queued', id = 'queue-101') => ({ id, item_id: '101', mode: 'reprocess', status,
  item_json: { item_type: 'Movie', name: '队列电影' }, message: '' });
const snapshot = items => ({ items, total: items.length, page: 1, page_size: 100,
  queued_count: items.filter(row => row.status === 'queued').length, running_count: items.filter(row => row.status === 'running').length });
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const flush = () => new Promise(resolve => setImmediate(resolve));
const retryReceipt = (message, mode = 'reprocess') => ({ status: 202, data: { mode, queued_count: 1, duplicate_count: 0,
  queue_ids: ['queue-retry'], items: [{ ...item(), id: 'queue-retry', mode }], message } });

test('unconfirmed cancellation and retry receipts cannot publish a successful operation', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  for (const operation of ['cancel', 'retry']) {
    const queue = useMediaOrganizeQueue({ client: {
      get: async () => ({ data: snapshot([item(operation === 'cancel' ? 'queued' : 'failed')]) }),
      delete: async () => ({ data: { message: '操作成功' } }),
      post: async () => ({ status: 202, data: { message: '操作成功' } }),
    } });
    queue.start(); await queue.refresh();
    await queue[operation](queue.items[0]);
    assert.equal(queue.actionMessage, '');
    assert.match(queue.actionError, /尚未确认/);
    queue.stop();
  }
});

test('a confirmed cancellation aborts a stalled old poll and promptly reads its current state', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let reads = 0, cancelled = false, signal;
  const queue = useMediaOrganizeQueue({ client: {
    get: (_url, options) => {
      reads++;
      if (reads !== 2) return Promise.resolve({ data: snapshot([item(cancelled ? 'cancelled' : 'queued')]) });
      signal = options.signal;
      return new Promise((_resolve, reject) => signal.addEventListener('abort', () => reject(new Error('canceled')), { once: true }));
    },
    delete: async () => { cancelled = true; return { data: { message: '已取消', item: item('cancelled') } }; },
  } }); t.after(queue.stop);
  queue.start(); await queue.refresh();
  const reading = queue.refresh();
  const mutation = queue.cancel(queue.items[0]); await flush();
  assert.equal(signal.aborted, true);
  await reading; await mutation;
  assert.equal(reads, 3);
  assert.equal(queue.items[0].status, 'cancelled');
  assert.equal(queue.actionId, '');
  assert.equal(queue.error, '');
});

test('a confirmed cancellation reads fresh state without waiting for an abort-ignoring old poll', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const old = deferred(), current = deferred(); let reads = 0, signal;
  const queue = useMediaOrganizeQueue({ client: {
    get: (_url, options) => {
      reads++;
      if (reads === 2) { signal = options.signal; return old.promise; }
      return reads === 3 ? current.promise : Promise.resolve({ data: snapshot([item()]) });
    },
    delete: async () => ({ data: { message: '已取消', item: item('cancelled') } }),
  } }); t.after(queue.stop);
  queue.start(); await queue.refresh();
  const stale = queue.refresh();
  const mutation = queue.cancel(queue.items[0]); await flush();
  assert.equal(signal.aborted, true);
  assert.equal(reads, 3, 'the confirmed operation starts a fresh read before the old promise settles');
  assert.equal(queue.loading, true);
  current.resolve({ data: snapshot([item('cancelled')]) }); await mutation;
  assert.equal(queue.items[0].status, 'cancelled');
  assert.equal(queue.actionId, '');
  old.resolve({ data: snapshot([item()]) }); await stale;
  assert.equal(queue.items[0].status, 'cancelled');
  assert.equal(queue.error, '');
  assert.equal(reads, 3);
});

test('a completed old poll waiting for history cannot hold up a confirmed queue operation', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const history = deferred(); let reads = 0, callbacks = 0, finished = false;
  const other = status => ({ ...item(status, 'queue-202'), item_id: '202' });
  const queue = useMediaOrganizeQueue({ client: {
    get: async () => {
      reads++;
      return { data: snapshot([item(reads === 1 ? 'running' : 'completed'), other(reads < 3 ? 'queued' : 'cancelled')]) };
    },
    delete: async () => ({ data: { message: '已取消', item: other('cancelled') } }),
  }, onSettled: () => ++callbacks === 1 ? history.promise : Promise.resolve() }); t.after(queue.stop);
  queue.start(); await queue.refresh();
  const stale = queue.refresh(); await flush();
  assert.equal(callbacks, 1);
  const mutation = queue.cancel(queue.items[1]); mutation.then(() => { finished = true; });
  await flush();
  assert.equal(reads, 3, 'a slow history callback is independent of the operation receipt');
  assert.equal(finished, true);
  assert.equal(queue.items[1].status, 'cancelled');
  assert.equal(queue.actionId, '');
  history.resolve(); await stale; await mutation;
  assert.equal(reads, 3);
  assert.equal(queue.actionMessage, '已取消');
});

test('an old cancellation and its reads cannot overwrite a restarted queue session', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const old = deferred(), cancelledRead = deferred(); let reads = 0;
  const queue = useMediaOrganizeQueue({ client: {
    get: () => {
      reads++;
      if (reads === 2) return old.promise;
      if (reads === 3) return cancelledRead.promise;
      return Promise.resolve({ data: snapshot([item()]) });
    },
    delete: async () => ({ data: { message: '已取消', item: item('cancelled') } }),
  } }); t.after(queue.stop);
  queue.start(); await queue.refresh();
  const reading = queue.refresh();
  const mutation = queue.cancel(queue.items[0]); await flush();
  assert.equal(reads, 3);
  assert.equal(queue.actionId, 'queue-101');
  queue.stop(); queue.start(); await queue.refresh();
  assert.equal(reads, 4);
  assert.equal(queue.items[0].status, 'queued');
  assert.equal(queue.loading, false);
  cancelledRead.resolve({ data: snapshot([item('cancelled')]) });
  old.resolve({ data: snapshot([item()]) }); await reading; await mutation;
  assert.equal(reads, 4);
  assert.equal(queue.items[0].status, 'queued');
  assert.equal(queue.actionId, '');
  assert.equal(queue.actionMessage, '');
  assert.equal(queue.actionError, '');
});

test('queue polling reads actual states and never marks an accepted row completed', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let rows = [item()], settled = 0;
  const queue = useMediaOrganizeQueue({ client: { get: async () => ({ data: snapshot(rows) }) }, onSettled: () => { settled++; } });
  t.after(queue.stop);
  queue.start(); await queue.refresh();
  assert.equal(queue.queuedCount, 1);
  assert.equal(queue.items[0].status, 'queued');
  assert.equal(settled, 0);
  rows = [item('running')]; t.mock.timers.tick(3000); await queue.refresh();
  assert.equal(queue.runningCount, 1);
  assert.equal(settled, 0);
  rows = [item('completed')]; t.mock.timers.tick(3000); await queue.refresh();
  assert.equal(queue.runningCount, 0);
  assert.equal(queue.items[0].status, 'completed');
  assert.equal(settled, 1);
  await queue.refresh();
  assert.equal(settled, 1);
});

test('slow reads share one request and stop cannot recreate its polling timer', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const pending = deferred(); let reads = 0, signal;
  const queue = useMediaOrganizeQueue({ client: { get: (_url, options) => { reads++; signal = options.signal; return pending.promise; } } });
  queue.start();
  const first = queue.refresh();
  assert.equal(queue.refresh(), first);
  t.mock.timers.tick(12000);
  assert.equal(reads, 1);
  queue.stop();
  assert.equal(signal.aborted, true);
  pending.resolve({ data: snapshot([item('completed')]) });
  await first;
  t.mock.timers.tick(12000);
  assert.equal(reads, 1);
  assert.equal(queue.loaded, false);
  assert.deepEqual([...queue.items], []);
});

test('a shrinking queue leaves an empty old page and reads its current last page', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const current = deferred(), pages = [];
  const queue = useMediaOrganizeQueue({ client: { get: (_url, options) => {
    pages.push(options.params.page);
    return pages.length === 1 ? Promise.resolve({ data: { ...snapshot([]), total: 1, page: 2 } }) : current.promise;
  } } }); t.after(queue.stop);
  queue.page = 2; queue.start(); const first = queue.refresh();
  await first; await flush();
  assert.deepEqual(pages, [2, 1]);
  assert.equal(queue.page, 1);
  const reading = queue.refresh(); current.resolve({ data: snapshot([item()]) }); await reading;
  assert.equal(queue.items[0].item_id, '101');
});

test('failed reads and invalid responses preserve the prior task snapshot', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let response = { data: snapshot([item('running')]) };
  const queue = useMediaOrganizeQueue({ client: { get: async () => { if (response instanceof Error) throw response; return response; } } });
  t.after(queue.stop);
  queue.start(); await queue.refresh();
  response = new Error('队列读取失败'); await queue.refresh();
  assert.equal(queue.error, '队列读取失败');
  assert.equal(queue.items[0].status, 'running');
  response = { data: { ...snapshot([]), items: [{ ...item(), status: 'success' }] } }; await queue.refresh();
  assert.equal(queue.error, '整理队列响应无效');
  assert.equal(queue.items[0].status, 'running');
});

test('running tasks cannot be cancelled and a cancelled waiting task is re-read', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const calls = []; let rows = [item()];
  const client = { get: async () => ({ data: snapshot(rows) }), delete: async url => { calls.push(url); rows = [item('cancelled')]; return { data: { message: '等待任务已取消', item: rows[0] } }; } };
  const queue = useMediaOrganizeQueue({ client }); t.after(queue.stop);
  queue.start(); await queue.refresh();
  await queue.cancel(item('running'));
  assert.deepEqual(calls, []);
  await queue.cancel(queue.items[0]);
  assert.deepEqual(calls, ['/api/media_organize/queue/queue-101']);
  assert.equal(queue.items[0].status, 'cancelled');
  assert.equal(queue.actionMessage, '等待任务已取消');
});

test('duplicate retry clicks send one request and failure preserves the task', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const pending = deferred(); let retries = 0;
  const queue = useMediaOrganizeQueue({ client: { get: async () => ({ data: snapshot([item('failed')]) }), post: () => { retries++; return pending.promise; } } });
  t.after(queue.stop);
  queue.start(); await queue.refresh();
  const first = queue.retry(queue.items[0]); await queue.retry(queue.items[0]);
  assert.equal(retries, 1);
  pending.reject({ response: { data: { error: '本文件已经在执行' } } }); await first;
  assert.equal(queue.actionError, '本文件已经在执行');
  assert.equal(queue.items[0].status, 'failed');
  assert.equal(queue.actionId, '');
});

test('a read started before cancellation cannot overwrite its confirmed queue state', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const old = deferred(); let reads = 0, cancelled = false;
  const queue = useMediaOrganizeQueue({ client: {
    get: () => { reads++; return reads === 2 ? old.promise : Promise.resolve({ data: snapshot([item(cancelled ? 'cancelled' : 'queued')]) }); },
    delete: async () => { cancelled = true; return { data: { message: '已取消', item: item('cancelled') } }; },
  } }); t.after(queue.stop);
  queue.start(); await queue.refresh();
  const reading = queue.refresh();
  const mutation = queue.cancel(queue.items[0]); await flush();
  assert.equal(reads, 3);
  await mutation;
  assert.equal(queue.items[0].status, 'cancelled');
  assert.equal(queue.queuedCount, 0);
  old.resolve({ data: snapshot([item()]) }); await reading; await mutation;
  assert.equal(reads, 3);
  assert.equal(queue.items[0].status, 'cancelled');
  assert.equal(queue.queuedCount, 0);
  assert.equal(queue.error, '');
});

test('a confirmed retry starts a fresh read without waiting for an abort-ignoring old poll', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const old = deferred(); let reads = 0, signal;
  const queue = useMediaOrganizeQueue({ client: {
    get: (_url, options) => {
      reads++;
      if (reads === 2) { signal = options.signal; return old.promise; }
      return Promise.resolve({ data: snapshot([item(reads === 1 ? 'failed' : 'queued', reads === 1 ? 'queue-101' : 'queue-retry')]) });
    },
    post: async () => retryReceipt('已重新入队'),
  } }); t.after(queue.stop);
  queue.start(); await queue.refresh();
  const stale = queue.refresh();
  await queue.retry(queue.items[0]);
  assert.equal(signal.aborted, true);
  assert.equal(reads, 3);
  assert.equal(queue.items[0].id, 'queue-retry');
  assert.equal(queue.items[0].status, 'queued');
  assert.equal(queue.queuedCount, 1);
  assert.equal(queue.actionMessage, '已重新入队');
  old.resolve({ data: snapshot([item('failed')]) }); await stale;
  assert.equal(queue.items[0].id, 'queue-retry');
  assert.equal(queue.queuedCount, 1);
  assert.equal(queue.actionError, '');
});

test('stale errors and finalizers cannot clear a fresh read or its loading indicator', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const old = deferred(), current = deferred(); let reads = 0;
  const queue = useMediaOrganizeQueue({ client: {
    get: () => {
      reads++;
      if (reads === 2) return old.promise;
      if (reads === 3) return current.promise;
      return Promise.resolve({ data: snapshot([item(reads === 1 ? 'queued' : 'cancelled')]) });
    },
    delete: async () => ({ data: { item: item('cancelled') } }),
  } }); t.after(queue.stop);
  queue.start(); await queue.refresh();
  const stale = queue.refresh();
  const mutation = queue.cancel(queue.items[0]); await flush();
  const fresh = queue.refresh();
  old.reject(new Error('旧轮询读取失败')); await stale;
  assert.equal(queue.loading, true);
  assert.equal(queue.error, '');
  assert.equal(queue.refresh(), fresh);
  t.mock.timers.tick(6000);
  assert.equal(reads, 3);
  current.resolve({ data: snapshot([item('cancelled')]) }); await mutation;
  assert.equal(queue.loading, false);
  t.mock.timers.tick(2999);
  assert.equal(reads, 3);
  t.mock.timers.tick(1); await queue.refresh();
  assert.equal(reads, 4);
});

test('an old poll finalizer cannot postpone the current polling timer', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const old = deferred(); let reads = 0;
  const queue = useMediaOrganizeQueue({ client: {
    get: () => ++reads === 2 ? old.promise : Promise.resolve({ data: snapshot([item(reads === 1 ? 'queued' : 'cancelled')]) }),
    delete: async () => ({ data: { item: item('cancelled') } }),
  } }); t.after(queue.stop);
  queue.start(); await queue.refresh();
  const stale = queue.refresh();
  await queue.cancel(queue.items[0]);
  assert.equal(reads, 3);
  t.mock.timers.tick(1000);
  old.resolve({ data: snapshot([item()]) }); await stale;
  t.mock.timers.tick(2000); await queue.refresh();
  assert.equal(reads, 4);
  assert.equal(queue.items[0].status, 'cancelled');
});

test('changing page size starts its own request and still coalesces identical current queries', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const old = deferred(), current = deferred(), queries = [];
  const queue = useMediaOrganizeQueue({ client: { get: (_url, options) => {
    queries.push({ ...options.params });
    return queries.length === 1 ? old.promise : current.promise;
  } } }); t.after(queue.stop);
  queue.start(); const stale = queue.refresh();
  queue.pageSize = 25;
  const fresh = queue.refresh();
  assert.equal(queue.refresh(), fresh);
  assert.deepEqual(queries, [{ page: 1, page_size: 100 }, { page: 1, page_size: 25 }]);
  old.resolve({ data: snapshot([item()]) }); await stale;
  assert.equal(queue.loaded, false);
  assert.equal(queue.loading, true);
  current.resolve({ data: { ...snapshot([item('failed')]), total: 50, page_size: 25 } }); await fresh;
  assert.equal(queue.items[0].status, 'failed');
  assert.equal(queue.total, 50);
  assert.equal(queue.pageSize, 25);
});

test('changing pages while history is pending rejects its late error and keeps the new read active', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const history = deferred(), current = deferred(), queries = [];
  const queue = useMediaOrganizeQueue({ client: { get: (_url, options) => {
    queries.push(options.params.page);
    if (queries.length === 3) return current.promise;
    return Promise.resolve({ data: { ...snapshot([item(queries.length === 1 ? 'running' : 'completed')]), total: 200 } });
  } }, onSettled: () => history.promise }); t.after(queue.stop);
  queue.start(); await queue.refresh();
  const stale = queue.refresh(); await flush();
  queue.page = 2;
  const fresh = queue.refresh();
  assert.deepEqual(queries, [1, 1, 2]);
  history.reject(new Error('旧历史记录读取失败')); await stale;
  assert.equal(queue.loading, true);
  assert.equal(queue.error, '');
  assert.equal(queue.refresh(), fresh);
  current.resolve({ data: { ...snapshot([item('completed', 'queue-202')]), total: 200 } }); await fresh;
  assert.equal(queue.page, 2);
  assert.equal(queue.items[0].id, 'queue-202');
  assert.equal(queue.error, '');
});

test('changing pages ignores a pending old snapshot and immediately reads the requested page', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const first = deferred(), second = deferred(), calls = [];
  const queue = useMediaOrganizeQueue({ client: { get: (_url, options) => { calls.push(options.params.page); return calls.length === 1 ? first.promise : second.promise; } } }); t.after(queue.stop);
  queue.start(); const old = queue.refresh();
  queue.page = 2;
  const current = queue.refresh(); await flush();
  assert.deepEqual(calls, [1, 2], 'page two starts before page one responds');
  assert.equal(queue.refresh(), current);
  first.resolve({ data: snapshot([item()]) }); await old; await flush();
  assert.deepEqual(calls, [1, 2]);
  assert.equal(queue.items.length, 0);
  assert.equal(queue.loading, true);
  second.resolve({ data: { ...snapshot([item('failed', 'queue-202')]), total: 200, page: 2 } }); await current;
  assert.equal(queue.items[0].id, 'queue-202');
  assert.equal(queue.page, 2);
});

test('a mutation finishing after the page closes cannot publish messages or start new reads', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const pending = deferred(); let reads = 0;
  const queue = useMediaOrganizeQueue({ client: { get: async () => { reads++; return { data: snapshot([item()]) }; }, delete: () => pending.promise } });
  queue.start(); await queue.refresh();
  const mutation = queue.cancel(queue.items[0]); queue.stop();
  pending.resolve({ data: { message: '已取消', item: item('cancelled') } }); await mutation;
  assert.equal(reads, 1);
  assert.equal(queue.actionMessage, '');
});

test('restarting during an old retry releases its action lock without overwriting a new action', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const old = deferred(), current = deferred(); let retries = 0;
  const queue = useMediaOrganizeQueue({ client: { get: async () => ({ data: snapshot([item('failed')]) }), post: () => ++retries === 1 ? old.promise : current.promise } });
  t.after(queue.stop);
  queue.start(); await queue.refresh();
  const first = queue.retry(queue.items[0]);
  queue.stop(); queue.start(); await queue.refresh();
  assert.equal(queue.actionId, '');
  const second = queue.retry(queue.items[0]);
  old.resolve(retryReceipt('旧重试')); await first;
  assert.equal(queue.actionId, 'queue-101');
  assert.equal(queue.actionMessage, '');
  current.resolve(retryReceipt('新重试已入队')); await second;
  assert.equal(retries, 2);
  assert.equal(queue.actionId, '');
  assert.equal(queue.actionMessage, '新重试已入队');
});
