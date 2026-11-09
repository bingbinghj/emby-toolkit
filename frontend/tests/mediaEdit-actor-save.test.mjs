import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');

function functionSource(name) {
  const start = page.indexOf(`const ${name} = `);
  assert.ok(start >= 0, `${name} must be extracted from the real page`);
  const end = page.indexOf('\n};', start);
  assert.ok(end > start);
  return page.slice(start, end + 4);
}

function callSource(marker) {
  const start = page.indexOf(marker);
  assert.ok(start >= 0, `${marker} must be wired in the real page`);
  let depth = 0, quote = null;
  for (let index = start; index < page.length; index += 1) {
    const character = page[index];
    if (quote) {
      if (character === '\\') index += 1;
      else if (character === quote) quote = null;
    } else if (['"', "'", '`'].includes(character)) quote = character;
    else if (character === '(') depth += 1;
    else if (character === ')' && --depth === 0) return `${page.slice(start, index + 1)};`;
  }
  assert.fail(`unterminated ${marker}`);
}

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function fixture(post = async () => ({ data: {} })) {
  const messages = [], calls = [], navigations = [], cleared = [], watchers = [], unmounts = [];
  const timers = new Map();
  const requestStarted = deferred();
  let timerId = 0;
  const context = vm.createContext({
    actorSaveSession: 1, actorSaveNavigationTimer: null,
    route: { name: 'MediaEditPage', params: { itemId: '101' }, fullPath: '/edit-media/101' },
    itemDetails: { value: { item_id: '101', item_name: '示例影片' } },
    editableCast: { value: [{ tmdbId: '12', name: '演员甲', role: '角色甲', emby_person_id: '8', _temp_id: 'draft-1' }] },
    isSaving: { value: false }, nextTick: () => Promise.resolve(),
    axios: { post: async (url, payload) => {
      calls.push({ url, payload: JSON.parse(JSON.stringify(payload)) }); requestStarted.resolve(); return post();
    } },
    message: Object.fromEntries(['success', 'warning', 'error'].map(type => [type, text => messages.push({ type, text })])),
    console: { error() {} }, goBack: () => navigations.push('back'),
    setTimeout: (callback, delay) => { const id = ++timerId; timers.set(id, { callback, delay }); return id; },
    clearTimeout: id => { cleared.push(id); timers.delete(id); },
    watch: (getter, callback) => watchers.push({ getter, callback }),
    onBeforeUnmount: callback => unmounts.push(callback),
    mediaDetailsLoadSequence: 1, window: { removeEventListener() {} },
    handleMediaInfoUndoShortcut() {}, closeMediaInfoEditor() {}, debouncedSearchActors: { cancel() {} },
  });
  vm.runInContext([
    functionSource('invalidateActorSaveSession'), functionSource('handleSaveChanges'),
    callSource('watch(() => route.fullPath,'), callSource('onBeforeUnmount('),
    'this.save = handleSaveChanges;',
  ].join('\n'), context);
  return {
    context, messages, calls, navigations, cleared, timers, started: requestStarted.promise,
    run: () => context.save(),
    unmount: () => unmounts.forEach(callback => callback()),
    changeRoute: () => watchers.forEach(({ getter, callback }) => callback(getter())),
    fire: id => { const timer = timers.get(id); assert.ok(timer); timers.delete(id); timer.callback(); },
    leave: () => {
      context.route.name = 'ReviewList'; context.route.params = {}; context.route.fullPath = '/review';
      watchers.forEach(({ getter, callback }) => callback(getter()));
    },
  };
}

test('actor save preserves the Rust request body and navigates only after confirmed success', async () => {
  const f = fixture();
  await f.run();
  assert.deepEqual(f.calls, [{ url: '/api/update_media_cast_sa/101', payload: {
    cast: [{ tmdbId: '12', name: '演员甲', role: '角色甲', emby_person_id: '8' }], item_name: '示例影片',
  } }]);
  assert.equal(f.messages[0].type, 'success');
  assert.equal(f.timers.size, 1);
  const id = f.context.actorSaveNavigationTimer;
  assert.equal(f.timers.get(id).delay, 1500);
  assert.deepEqual(f.navigations, []);
  f.fire(id);
  assert.deepEqual(f.navigations, ['back']);
  assert.equal(f.context.actorSaveNavigationTimer, null);
  assert.equal(f.context.isSaving.value, false);
});

for (const invalidation of ['route', 'unmount']) {
  for (const outcome of ['success', 'failure']) {
    test(`actor save ${outcome} arriving after ${invalidation} has no navigation or stale feedback`, async () => {
      const pending = deferred();
      const f = fixture(() => pending.promise);
      const saving = f.run();
      await f.started;
      assert.equal(f.calls.length, 1);
      if (invalidation === 'route') f.leave();
      else f.unmount();
      if (outcome === 'success') pending.resolve({ data: {} });
      else pending.reject({ response: { data: { error: '旧会话保存失败' } } });
      await saving;
      assert.deepEqual(f.messages, []);
      assert.deepEqual(f.navigations, []);
      assert.equal(f.timers.size, 0);
      assert.equal(f.context.isSaving.value, false);
    });
  }
}

test('leaving during nextTick prevents submitting an actor request for the old route', async () => {
  const f = fixture();
  const saving = f.run();
  f.leave();
  await saving;
  assert.deepEqual(f.calls, []);
  assert.deepEqual(f.messages, []);
  assert.equal(f.timers.size, 0);
});

test('unmount cancels an existing navigation timer and guards even an already queued callback', async () => {
  const f = fixture();
  await f.run();
  const id = f.context.actorSaveNavigationTimer;
  const callback = f.timers.get(id).callback;
  f.unmount();
  assert.deepEqual(f.cleared, [id]);
  assert.equal(f.timers.size, 0);
  assert.equal(f.context.actorSaveNavigationTimer, null);
  callback();
  assert.deepEqual(f.navigations, []);
});

test('a new session for the same media cannot be affected by the old actor save', async () => {
  const pending = deferred();
  const f = fixture(() => pending.promise);
  const saving = f.run();
  await f.started;
  f.context.route.fullPath = '/edit-media/101?session=2';
  f.changeRoute();
  pending.resolve({ data: {} });
  await saving;
  assert.equal(f.context.itemDetails.value.item_id, '101');
  assert.deepEqual(f.messages, []);
  assert.equal(f.timers.size, 0);
});

test('duplicate actor saves produce one request and one navigation timer', async () => {
  const pending = deferred();
  const f = fixture(() => pending.promise);
  const saving = f.run();
  await f.started;
  await f.run();
  assert.equal(f.calls.length, 1);
  pending.resolve({ data: {} });
  await saving;
  assert.equal(f.messages.length, 1);
  assert.equal(f.timers.size, 1);
});

test('starting another save cancels prior delayed navigation while the new request is pending', async () => {
  const pending = deferred(), secondStarted = deferred();
  let requests = 0;
  const f = fixture(() => {
    if (++requests === 1) return Promise.resolve({ data: {} });
    secondStarted.resolve();
    return pending.promise;
  });
  await f.run();
  const previousId = f.context.actorSaveNavigationTimer;
  const previousCallback = f.timers.get(previousId).callback;
  const saving = f.run();
  await secondStarted.promise;
  assert.deepEqual(f.cleared, [previousId]);
  assert.equal(f.timers.size, 0);
  previousCallback();
  assert.deepEqual(f.navigations, []);
  pending.resolve({ data: {} });
  await saving;
  assert.equal(f.timers.size, 1);
  f.fire(f.context.actorSaveNavigationTimer);
  assert.deepEqual(f.navigations, ['back']);
});

test('an old queued callback cannot clear the navigation timer of a newer save', async () => {
  const f = fixture();
  await f.run();
  const previousId = f.context.actorSaveNavigationTimer;
  const previousCallback = f.timers.get(previousId).callback;
  await f.run();
  const currentId = f.context.actorSaveNavigationTimer;
  assert.notEqual(currentId, previousId);
  previousCallback();
  assert.equal(f.context.actorSaveNavigationTimer, currentId);
  assert.deepEqual(f.navigations, []);
  f.unmount();
  assert.deepEqual(f.cleared, [previousId, currentId]);
  assert.equal(f.timers.size, 0);
  assert.equal(f.context.actorSaveNavigationTimer, null);
});

test('actor edits made while saving stay in the editor with a warning', async () => {
  const pending = deferred();
  const f = fixture(() => pending.promise);
  const saving = f.run();
  await f.started;
  f.context.editableCast.value[0].name = '用户新增修改';
  pending.resolve({ data: {} });
  await saving;
  assert.equal(f.calls[0].payload.cast[0].name, '演员甲');
  assert.equal(f.context.editableCast.value[0].name, '用户新增修改');
  assert.equal(f.messages[0].type, 'warning');
  assert.match(f.messages[0].text, /保留当前草稿/);
  assert.equal(f.timers.size, 0);
});

test('actor edits made after a successful save prevent delayed navigation', async () => {
  const f = fixture();
  await f.run();
  const id = f.context.actorSaveNavigationTimer;
  f.context.editableCast.value[0].role = '尚未保存的新角色';
  f.fire(id);
  assert.deepEqual(f.navigations, []);
  assert.equal(f.context.editableCast.value[0].role, '尚未保存的新角色');
  assert.equal(f.context.actorSaveNavigationTimer, null);
});
