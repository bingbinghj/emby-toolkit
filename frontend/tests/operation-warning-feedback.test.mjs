import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const files = {
  layout: '../src/MainLayout.vue', review: '../src/components/ReviewList.vue',
  cast: '../src/components/MediaEditPage.vue', fingerprint: '../src/components/settings/FingerprintAuditPanel.vue',
};
const pages = Object.fromEntries(Object.entries(files).map(([key, path]) => [key, readFileSync(new URL(path, import.meta.url), 'utf8')]));
function definition(page, name, declaration = 'const') {
  const start = pages[page].indexOf(declaration === 'const' ? `const ${name} = ` : `async function ${name}(`);
  assert.ok(start >= 0, `${name} must be an actual Vue function`);
  const terminator = declaration === 'const' ? '\n};' : '\n}';
  return pages[page].slice(start, pages[page].indexOf(terminator, start) + terminator.length);
}
async function operate(page, name, response, extra = {}, reject = false, args = []) {
  const messages = [], loads = [];
  const context = vm.createContext({
    axios: { post: async () => { if (reject) throw response; return response; } },
    message: Object.fromEntries(['info', 'success', 'warning', 'error'].map(type => [type, text => messages.push({ type, text })])),
    console: { error() {} },
    reprocessFeedback: { value: null }, selectedMediaType: { value: '' },
    loadingAction: { value: {} }, currentRowId: { value: null }, isShowingSearchResults: { value: false },
    fetchReviewItems: async () => loads.push('review'),
    progress: { value: { run_id: 'scan' } }, busy: { value: false }, runFeedback: { value: null },
    load: async () => loads.push('fingerprint'),
    editableCast: { value: [] }, itemDetails: { value: {} }, itemId: { value: '101' }, isTranslating: { value: false },
    ...extra,
  });
  const helper = page === 'review' ? definition(page, 'showReprocessResult') : '';
  vm.runInContext(`${helper}\n${definition(page, name, page === 'fingerprint' ? 'function' : 'const')}\nthis.operate = ${name};`, context);
  await context.operate(...args);
  return { context, messages, loads };
}

test('restart network interruption reports an unconfirmed result', async () => {
  const f = await operate('layout', 'triggerRestart', new Error('Network Error'), {}, true);
  assert.equal(f.messages.at(-1).type, 'warning');
  assert.match(f.messages.at(-1).text, /尚未确认/);
  assert.ok(!f.messages.some(message => message.type === 'success'));
});
test('confirmed restart succeeds and explicit rejection keeps its reason', async () => {
  const success = await operate('layout', 'triggerRestart', { status: 200, data: {} });
  assert.equal(success.messages.at(-1).type, 'success');
  const failure = await operate('layout', 'triggerRestart', { response: { data: { error: '容器未找到' } } }, {}, true);
  assert.equal(failure.messages.at(-1).type, 'error');
  assert.match(failure.messages.at(-1).text, /容器未找到/);
});

for (const name of ['handleReprocessItem', 'reprocessAllReviewItems']) {
  const args = name === 'handleReprocessItem' ? [{ item_id: '101', item_name: '电影', reason: '需要复核' }] : [];
  test(`${name} retains warnings for different media IDs`, async () => {
    const f = await operate('review', name, { status: 200, data: {
      warnings: 2, message: '处理完成，有提醒', warning_details: [{ item_id: '101', reason: '恢复未确认' }, { item_id: '102', reason: '恢复未确认' }],
    } }, {}, false, args);
    assert.equal(f.messages.at(-1).type, 'warning');
    assert.deepEqual(Array.from(f.context.reprocessFeedback.value.details), ['101：恢复未确认', '102：恢复未确认']);
    assert.match(f.messages.at(-1).text, /101.*102/);
  });
  test(`${name} treats 207 as a warning and exposes failures above warnings`, async () => {
    const warning = await operate('review', name, { status: 207, data: { message: '部分处理未确认' } }, {}, false, args);
    assert.equal(warning.messages.at(-1).type, 'warning');
    const failure = await operate('review', name, { response: { status: 500, data: { message: '处理失败', failures: [{ item_id: '101', reason: '保存失败' }], warnings: 1 } } }, {}, true, args);
    assert.equal(failure.messages.at(-1).type, 'error');
    assert.match(failure.messages.at(-1).text, /101.*保存失败/);
    assert.equal(failure.context.reprocessFeedback.value.type, 'error');
  });
  test(`${name} clears old feedback only on success`, async () => {
    const f = await operate('review', name, { status: 200, data: { message: '处理完成' } }, { reprocessFeedback: { value: { type: 'warning' } } }, false, args);
    assert.equal(f.messages.at(-1).type, 'success');
    assert.equal(f.context.reprocessFeedback.value, null);
  });
}

test('fingerprint scan warns about unknown items and points to actual reasons', async () => {
  const f = await operate('fingerprint', 'run', { status: 200, data: { warnings: 2, counts: { unknown: 2 }, message: '扫描完成' } });
  assert.equal(f.messages.at(-1).type, 'warning');
  assert.match(f.context.runFeedback.value.details[0], /2.*原因/);
  assert.equal(f.context.busy.value, false);
  assert.deepEqual(f.loads, ['fingerprint']);
});
test('fingerprint warning details retain different IDs and failures take priority', async () => {
  const f = await operate('fingerprint', 'run', { status: 207, data: { warning_details: [{ item_id: '101', reason: '身份未确认' }, { item_id: '102', reason: '身份未确认' }] } });
  assert.equal(f.messages.at(-1).type, 'warning');
  assert.deepEqual(Array.from(f.context.runFeedback.value.details), ['101：身份未确认', '102：身份未确认']);
  const failed = await operate('fingerprint', 'run', { status: 200, data: { warnings: 1, failures: ['101：写入失败'] } });
  assert.equal(failed.messages.at(-1).type, 'error');
  assert.match(failed.messages.at(-1).text, /101.*写入失败/);
});
test('fingerprint scan succeeds only without warnings and exposes request rejection', async () => {
  const success = await operate('fingerprint', 'run', { status: 200, data: { counts: { unknown: 0 }, message: '扫描完成' } }, { runFeedback: { value: { type: 'warning' } } });
  assert.equal(success.messages.at(-1).type, 'success');
  assert.equal(success.context.runFeedback.value, null);
  const failed = await operate('fingerprint', 'run', { response: { data: { error: '扫描中断', failures: [{ item_id: '101', reason: '网络失败' }] } } }, {}, true);
  assert.equal(failed.messages.at(-1).type, 'error');
  assert.match(failed.messages.at(-1).text, /101.*网络失败/);
  assert.equal(failed.context.busy.value, false);
});

test('partial actor translations preserve obtained fields and expose incomplete reasons', async () => {
  const actors = [{ tmdbId: 1, emby_person_id: 'p1', name: '译名', role: '译文角色', imageUrl: 'image', matchStatus: '已翻译' },
    { tmdbId: 2, emby_person_id: 'p2', name: 'Raw name', role: '已译角色', matchStatus: '翻译未完成', translationWarning: 'AI请求失败' }];
  const f = await operate('cast', 'translateAllFields', { data: actors });
  assert.equal(f.messages.at(-1).type, 'warning');
  assert.match(f.messages.at(-1).text, /1.*AI请求失败/);
  assert.equal(f.context.editableCast.value[0].name, '译名');
  assert.equal(f.context.editableCast.value[0].imageUrl, 'image');
  assert.equal(f.context.editableCast.value[1].role, '已译角色');
  assert.equal(f.context.editableCast.value[1].emby_person_id, 'p2');
  assert.equal(f.context.editableCast.value[1].translationWarning, 'AI请求失败');
  assert.match(pages.cast, /v-if="actor\.matchStatus === '翻译未完成' \|\| actor\.translationWarning"/);
  assert.match(pages.cast, /\{\{ actor\.translationWarning \|\|/);
  assert.equal(f.context.isTranslating.value, false);
});
test('successful translations succeed while request failures preserve the draft', async () => {
  const success = await operate('cast', 'translateAllFields', { data: [{ name: '译名', role: '角色', matchStatus: '已翻译' }] });
  assert.equal(success.messages.at(-1).type, 'success');
  const draft = [{ name: '用户草稿', role: '角色' }];
  const failed = await operate('cast', 'translateAllFields', { response: { data: { error: '翻译服务失联' } } }, { editableCast: { value: draft } }, true);
  assert.equal(failed.messages.at(-1).type, 'error');
  assert.equal(failed.context.editableCast.value, draft);
  assert.equal(failed.context.isTranslating.value, false);
});

for (const [change, mutate] of [
  ['name', cast => { cast.value[0].name = '用户修改的姓名'; }],
  ['role', cast => { cast.value[0].role = '用户修改的角色'; }],
  ['delete', cast => { cast.value.splice(0, 1); }],
  ['order', cast => { cast.value.reverse(); }],
]) {
  test(`late actor translations preserve a draft changed by ${change}`, async () => {
    const cast = { value: [{ name: 'First', role: 'Role' }, { name: 'Second', role: 'Other' }] };
    let resolve;
    const pending = new Promise(done => { resolve = done; });
    const operation = operate('cast', 'translateAllFields', null, {
      editableCast: cast, axios: { post: async () => pending },
    });
    mutate(cast);
    const modified = JSON.stringify(cast.value);
    resolve({ data: [{ name: '旧请求返回姓名', role: '旧角色', matchStatus: '已翻译' }] });
    const f = await operation;
    assert.equal(JSON.stringify(cast.value), modified);
    assert.equal(f.messages.at(-1).type, 'warning');
    assert.match(f.messages.at(-1).text, /保留.*草稿/);
    assert.equal(f.context.isTranslating.value, false);
  });
}

for (const change of ['target', 'details']) {
  test(`late actor translations cannot overwrite a changed ${change}`, async () => {
    const cast = { value: [{ name: 'First', role: 'Role' }] };
    const details = { value: { item_name: '旧影片' } }, id = { value: '101' };
    let resolve;
    const pending = new Promise(done => { resolve = done; });
    const operation = operate('cast', 'translateAllFields', null, {
      editableCast: cast, itemDetails: details, itemId: id, axios: { post: async () => pending },
    });
    if (change === 'target') id.value = '102';
    else details.value = { item_name: '重新载入的影片' };
    resolve({ data: [{ name: '旧请求返回姓名', role: '旧角色', matchStatus: '已翻译' }] });
    const f = await operation;
    assert.equal(cast.value[0].name, 'First');
    assert.equal(f.messages.length, 0);
    assert.equal(f.context.isTranslating.value, false);
  });
}

test('duplicate actor translation does not submit another request or clear its busy state', async () => {
  let requests = 0;
  const busy = { value: true };
  const f = await operate('cast', 'translateAllFields', null, {
    isTranslating: busy, axios: { post: async () => { requests++; return { data: [] }; } },
  });
  assert.equal(requests, 0);
  assert.equal(f.messages.length, 0);
  assert.equal(busy.value, true);
});

test('disabled AI translation preserves original actors and warns instead of reporting success', async () => {
  const f = await operate('cast', 'translateAllFields', { data: [{ name: 'Original', role: 'Role', matchStatus: 'AI未启用' }] });
  assert.equal(f.messages.at(-1).type, 'warning');
  assert.match(f.messages.at(-1).text, /未启用.*保留/);
  assert.equal(f.context.editableCast.value[0].name, 'Original');
  assert.equal(f.context.editableCast.value[0].role, 'Role');
});
