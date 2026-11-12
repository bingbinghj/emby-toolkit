import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import axios, { AxiosError } from 'axios';

const source = readFileSync(new URL('../src/utils/backgroundTasks.js', import.meta.url), 'utf8')
  .replace(/^import .*;\n/gm, '').replace('export function', 'function');

function fixture(replies, submission = { status: 202, data: { task_id: 'save-task' } }) {
  const requests = [], waits = [];
  let next = 0;
  const client = axios.create({ adapter: async config => {
    requests.push({ method: config.method, url: config.url, timeout: config.timeout });
    if (config.method === 'post') return { ...submission, config, request: {} };
    const reply = replies[next++];
    assert.ok(reply, 'a task result request exceeded the supplied reply sequence');
    if (reply.error) throw new AxiosError(reply.error, reply.code || 'ERR_NETWORK', config);
    const response = { status: reply.status || 200, data: reply.data, config, request: {} };
    if (response.status >= 400) throw new AxiosError('status request failed', 'ERR_BAD_RESPONSE', config, {}, response);
    return response;
  } });
  const context = vm.createContext({ axios: client, AxiosError, refreshTaskStatus() {},
    setTimeout: (done, delay) => { waits.push(delay); done(); } });
  vm.runInContext(`${source}\ninstallBackgroundTaskInterceptor();`, context);
  return { run: () => client.post('/api/media_info/edit/101', { mediainfo: [] }), requests, waits };
}

const completed = { data: { state: 'completed', result_status: 200, result: { metadata_refresh: { state: 'verified' } } } };

test('a synchronous response is returned without task polling', async () => {
  const f = fixture([], { status: 200, data: { message: '即时结果' } });
  assert.equal((await f.run()).data.message, '即时结果');
  assert.equal(f.requests.length, 1);
});

test('temporary network and server failures retry only result GETs, preserving the submitted operation', async () => {
  const f = fixture([{ error: 'connection reset' }, { status: 503 }, completed]);
  const result = await f.run();
  assert.equal(result.data.metadata_refresh.state, 'verified');
  assert.deepEqual(f.requests.map(request => request.method), ['post', 'get', 'get', 'get']);
  assert.ok(f.requests.slice(1).every(request => request.timeout === 10000));
});

test('a valid running reply resets the consecutive result-read failure budget', async () => {
  const f = fixture([{ error: 'temporary network failure' }, { data: { state: 'running' } },
    { status: 502 }, { status: 502 }, completed]);
  assert.equal((await f.run()).data.metadata_refresh.state, 'verified');
  assert.equal(f.requests.filter(request => request.method === 'post').length, 1);
  assert.equal(f.requests.filter(request => request.method === 'get').length, 5);
});

test('three consecutive timeouts report an unconfirmed task state with its ID', async () => {
  const f = fixture(Array.from({ length: 3 }, () => ({ error: 'timeout', code: 'ECONNABORTED' })));
  await assert.rejects(f.run(), error => {
    assert.equal(error.response.data.task_id, 'save-task');
    assert.match(error.response.data.error, /状态.*未确认/);
    assert.match(error.response.data.error, /仍.*运行/);
    return true;
  });
  assert.deepEqual(f.requests.map(request => request.method), ['post', 'get', 'get', 'get']);
});

for (const status of [401, 403]) {
  test(`authorization failure ${status} is not retried`, async () => {
    const f = fixture([{ status, data: { error: '请登录或核对权限' } }]);
    await assert.rejects(f.run(), error => error.response.status === status);
    assert.equal(f.requests.length, 2);
  });
}

test('explicit request cancellation is not retried', async () => {
  const f = fixture([{ error: 'cancelled', code: 'ERR_CANCELED' }]);
  await assert.rejects(f.run(), error => error.code === 'ERR_CANCELED');
  assert.equal(f.requests.length, 2);
});

for (const state of ['failed', 'cancelled']) {
  test(`a real ${state} terminal result retains its original error and details`, async () => {
    const f = fixture([{ data: { state, result_status: 409, result: { error: '实际任务未完成', failures: ['图片恢复失败'] } } }]);
    await assert.rejects(f.run(), error => {
      assert.equal(error.response.status, 409);
      assert.equal(error.response.data.error, '实际任务未完成');
      assert.deepEqual(error.response.data.failures, ['图片恢复失败']);
      return true;
    });
    assert.equal(f.requests.length, 2);
  });
}

test('completed-with-warning responses preserve the result body and HTTP status', async () => {
  const result = { warnings: 1, warning_details: ['备份仍需清理'] };
  const f = fixture([{ data: { state: 'completed_with_warnings', result_status: 207, result } }]);
  const response = await f.run();
  assert.equal(response.status, 207);
  assert.deepEqual(response.data, result);
});
