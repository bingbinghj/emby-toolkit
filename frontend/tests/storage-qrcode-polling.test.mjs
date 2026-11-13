import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../src/components/settings/GeneralSettingsPage.vue', import.meta.url), 'utf8');
const start = source.indexOf('const startPolling = ');
const end = source.indexOf('// ★★★ 全自动网页授权', start);
const pollingSource = source.slice(start, end);
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

function fixture() {
  const requests = [], notices = [], timers = [];
  let callback;
  const context = vm.createContext({
    openApiQrcodeRequestVersion: 1, componentIsMounted: { value: true },
    showQrcodeModal: { value: true }, qrcodeStatus: { value: 'waiting' },
    qrcodePolling: { value: null }, qrcodeUrl: { value: 'fixture-qrcode' },
    axios: { get: path => {
      const pending = deferred(); requests.push({ path, ...pending }); return pending.promise;
    } },
    message: { success: text => notices.push(text), warning: text => notices.push(text) },
    check115Status() {}, console: { error() {} },
    setInterval: fn => { callback = fn; return 1; }, clearInterval() {},
    setTimeout: fn => { timers.push(fn); return 1; },
  });
  vm.runInContext(`${pollingSource}\nthis.start = startPolling; this.close = closeQrcodeModal;`, context);
  context.start();
  return { context, requests, notices, timers, tick: () => callback() };
}

test('a slow QR status read stays single-flight instead of accumulating every two seconds', async () => {
  const f = fixture();
  const pending = f.tick();
  f.tick(); f.tick();
  assert.equal(f.requests.length, 1);
  assert.equal(f.requests[0].path, '/api/p115/qrcode/status');
  f.requests[0].resolve({ data: { status: 'waiting' } }); await pending;
  const next = f.tick();
  assert.equal(f.requests.length, 2);
  f.requests[1].resolve({ data: { status: 'success' } }); await next;
  assert.equal(f.context.qrcodeStatus.value, 'success');
  assert.equal(f.notices.length, 1);
});

for (const status of ['success', 'expired']) {
  test(`queued QR ticks cannot reread or overwrite the ${status} terminal state`, async () => {
    const f = fixture();
    const pending = f.tick();
    f.requests[0].resolve({ data: { status } }); await pending;
    f.tick();
    assert.equal(f.requests.length, 1);
    assert.equal(f.context.qrcodeStatus.value, status);
    assert.equal(f.notices.length, 1);
  });
}

test('a failed status read releases the pending slot so the same QR session can retry', async () => {
  const f = fixture();
  const pending = f.tick();
  f.requests[0].reject(new Error('temporary status failure')); await pending;
  const next = f.tick();
  assert.equal(f.requests.length, 2);
  f.requests[1].resolve({ data: { status: 'success' } }); await next;
  assert.equal(f.context.qrcodeStatus.value, 'success');
});

test('closing a QR session ignores its late success and does not schedule an automatic close', async () => {
  const f = fixture();
  const pending = f.tick();
  f.context.close();
  f.requests[0].resolve({ data: { status: 'success' } }); await pending;
  assert.equal(f.context.qrcodeStatus.value, 'idle');
  assert.equal(f.notices.length, 0);
  assert.equal(f.timers.length, 0);
  await f.tick();
  assert.equal(f.requests.length, 1);
});
