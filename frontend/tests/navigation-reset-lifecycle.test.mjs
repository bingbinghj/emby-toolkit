import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { mergeNavigationConfig } from '../src/utils/navigationConfig.js';

const source = readFileSync(new URL('../src/MainLayout.vue', import.meta.url), 'utf8');
const start = source.indexOf('const resetMenuConfig = ');
const resetSource = source.slice(start, source.indexOf('\n};', start) + 3);

function fixture() {
  let release, fail;
  const requests = [], notices = [];
  const pending = new Promise((resolve, reject) => { release = resolve; fail = reject; });
  const base = [{ key: 'Dashboard', defaultLabel: '仪表盘' }];
  const context = vm.createContext({
    isSavingMenu: { value: false }, isMenuLoading: { value: false },
    menuEditorSession: 1, authStore: { isLoggedIn: true, user: { id: 'original-admin' } },
    menuConfigTree: { value: [{ key: 'Dashboard', label: '原导航草稿', visible: true }] },
    menuConfigLoaded: { value: true }, menuConfigError: { value: '' },
    syncMenuConfig: tree => mergeNavigationConfig(tree, base),
    axios: { post: path => { requests.push(path); return pending; } },
    message: Object.fromEntries(['success', 'error', 'info'].map(kind => [kind, text => notices.push({ kind, text })])),
  });
  vm.runInContext(`${resetSource}\nthis.reset = resetMenuConfig;`, context);
  return { context, requests, notices, release, fail };
}

test('default navigation reset preserves changes entered while the request is pending', async () => {
  const f = fixture(), saving = f.context.reset();
  f.context.menuConfigTree.value[0].label = '重置期间的新名称';
  f.release({ data: {} }); await saving;
  assert.equal(f.context.menuConfigTree.value[0].label, '重置期间的新名称');
  assert.match(f.notices[0].text, /当前修改尚未保存/);
  assert.equal(f.context.isSavingMenu.value, false);
  assert.deepEqual(f.requests, ['/api/system/menu_config/reset']);
});

for (const change of ['reopened editor', 'new user']) {
  test(`a late reset cannot replace the draft or notify the ${change}`, async () => {
    const f = fixture(), saving = f.context.reset();
    if (change === 'reopened editor') f.context.menuEditorSession += 2;
    else f.context.authStore.user = { id: 'new-admin' };
    f.release({ data: {} }); await saving;
    assert.equal(f.context.menuConfigTree.value[0].label, '原导航草稿');
    assert.equal(f.notices.length, 0);
    assert.equal(f.context.isSavingMenu.value, false);
  });
}

test('an unchanged editor accepts verified default navigation after resetting a failed read', async () => {
  const f = fixture();
  f.context.menuConfigLoaded.value = false;
  f.context.menuConfigError.value = '旧读取失败';
  const saving = f.context.reset();
  f.release({ data: {} }); await saving;
  assert.equal(f.context.menuConfigTree.value[0].label, '仪表盘');
  assert.equal(f.context.menuConfigLoaded.value, true);
  assert.equal(f.context.menuConfigError.value, '');
  assert.equal(f.notices[0].kind, 'success');
});

test('a late failed reset stays with its original editor', async () => {
  const f = fixture(), saving = f.context.reset();
  f.context.menuEditorSession += 2;
  f.fail({ response: { data: { error: '旧重置失败' } } }); await saving;
  assert.equal(f.notices.length, 0);
  assert.equal(f.context.menuConfigTree.value[0].label, '原导航草稿');
});
