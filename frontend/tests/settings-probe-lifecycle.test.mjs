import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../src/components/settings/GeneralSettingsPage.vue', import.meta.url), 'utf8');
const extract = name => {
  const start = page.indexOf('const ' + name + ' = ');
  const end = page.indexOf('\n};', start) + 4;
  return page.slice(start, end);
};
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

function fixture() {
  const requests = [], notices = [];
  const context = vm.createContext({
    componentIsMounted: { value: true },
    configModel: { value: { ai_provider: 'openai', ai_api_key: 'fixture', ai_model_name: 'fixture-A', ai_base_url: 'fixture-url', ai_request_timeout: 60 } },
    isTestingAI: { value: false }, aiTestRequest: null, p115StatusRequest: null,
    p115Info: { value: { user_info: { user_name: 'saved-user' }, has_token: true } },
    loading115Info: { value: false }, p115StatusError: { value: '' }, AbortController,
    axios: Object.fromEntries(['get', 'post'].map(method => [method, (path, bodyOrOptions, options) => {
      const pending = deferred();
      requests.push({ method, path, options: method === 'get' ? bodyOrOptions : options, ...pending });
      return pending.promise;
    }])),
    message: Object.fromEntries(['success', 'warning', 'error'].map(type => [type, message => notices.push({ type, message })])),
    dialog: Object.fromEntries(['success', 'error'].map(type => [type, data => notices.push({ type, ...data })])),
  });
  vm.runInContext(extract('testAI') + '\n' + extract('check115Status') + '\nthis.testAI = testAI; this.status = check115Status;', context);
  return { context, requests, notices };
}

test('an unchanged AI test remains single-flight', async () => {
  const f = fixture();
  const first = f.context.testAI();
  const duplicate = f.context.testAI();
  assert.equal(f.requests.length, 1);
  f.requests[0].resolve({ data: { success: true, message: 'fixture result' } });
  await Promise.all([first, duplicate]);
  assert.equal(f.notices.length, 1);
  assert.equal(f.context.isTestingAI.value, false);
});

for (const change of ['owner', 'connection', 'unmount']) {
  test('an obsolete AI result is ignored after ' + change, async () => {
    const f = fixture();
    const reading = f.context.testAI();
    if (change === 'owner') f.context.configModel.value = { ...f.context.configModel.value };
    if (change === 'connection') f.context.configModel.value.ai_model_name = 'fixture-B';
    if (change === 'unmount') f.context.componentIsMounted.value = false;
    f.requests[0].resolve({ data: { success: true, message: 'old fixture result' } });
    await reading;
    assert.equal(f.notices.length, 0);
  });
}

test('a new AI connection replaces the old request without its finally clearing current loading', async () => {
  const f = fixture();
  const old = f.context.testAI();
  f.context.configModel.value.ai_model_name = 'fixture-B';
  const current = f.context.testAI();
  assert.equal(f.requests.length, 2);
  assert.equal(f.requests[0].options?.signal.aborted, true);
  f.requests[0].reject(new Error('obsolete fixture failure'));
  await old;
  assert.equal(f.notices.length, 0);
  assert.equal(f.context.isTestingAI.value, true);
  f.requests[1].resolve({ data: { success: true, message: 'new fixture result' } });
  await current;
  assert.equal(f.notices.length, 1);
  assert.equal(f.context.isTestingAI.value, false);
});

test('a fresh 115 read cancels the old read and retains the latest credentials status', async () => {
  const f = fixture();
  const old = f.context.status();
  const current = f.context.status();
  assert.equal(f.requests[0].options?.signal.aborted, true);
  f.requests[1].resolve({ data: { data: { user_info: { user_name: 'new-user' }, has_token: true } } });
  await current;
  f.requests[0].resolve({ data: { data: { user_info: { user_name: 'old-user' }, has_token: false } } });
  await old;
  assert.equal(f.context.p115Info.value.user_info.user_name, 'new-user');
  assert.equal(f.context.p115Info.value.has_token, true);
  assert.equal(f.notices.length, 1);
});

test('an obsolete 115 failure does not clear the new request loading or saved account', async () => {
  const f = fixture();
  const old = f.context.status();
  f.context.configModel.value = { ...f.context.configModel.value };
  const current = f.context.status();
  f.requests[0].reject(new Error('obsolete fixture failure'));
  await old;
  assert.equal(f.context.loading115Info.value, true);
  assert.equal(f.context.p115Info.value.user_info.user_name, 'saved-user');
  assert.equal(f.context.p115StatusError.value, '');
  assert.equal(f.notices.length, 0);
  f.requests[1].resolve({ data: { data: { has_token: true } } });
  await current;
  assert.equal(f.context.loading115Info.value, false);
});

for (const change of ['owner', 'unmount']) {
  test('an obsolete 115 success is ignored after ' + change, async () => {
    const f = fixture();
    const pending = f.context.status();
    if (change === 'owner') f.context.configModel.value = { ...f.context.configModel.value };
    else f.context.componentIsMounted.value = false;
    f.requests[0].resolve({ data: { data: { has_token: false } } });
    await pending;
    assert.equal(f.context.p115Info.value.user_info.user_name, 'saved-user');
    assert.equal(f.notices.length, 0);
  });
}

test('a current invalid 115 response still reports the original error state', async () => {
  const f = fixture();
  const pending = f.context.status();
  f.requests[0].resolve({ data: {} });
  await pending;
  assert.equal(f.context.p115Info.value, null);
  assert.equal(f.context.p115StatusError.value, '状态响应无效');
  assert.equal(f.context.loading115Info.value, false);
});
