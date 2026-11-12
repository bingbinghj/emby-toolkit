import test from 'node:test';
import assert from 'node:assert/strict';
import { mergeNavigationConfig, moveNavigationItem } from '../src/utils/navigationConfig.js';
function merge(saved) {
  const groups = [
    { key: 'group-discovery', defaultLabel: '发现', children: [{ key: 'Dashboard', defaultLabel: '仪表盘' }] },
    { key: 'group-system', defaultLabel: '系统', children: [{ key: 'UserManagement', defaultLabel: '用户管理' }, { key: 'settings-general', defaultLabel: '设置' }] },
  ];
  return mergeNavigationConfig(saved, groups);
}

test('legacy user entry is not duplicated when the missing system group is filled in', () => {
  const result = merge([{ key: 'group-discovery', label: '发现', children: [
    { key: 'MediaCenter', label: '媒体中心', visible: true },
    { key: 'UserCenter', label: '我的用户', visible: false },
  ] }]);
  const children = result.flatMap(group => group.children);
  assert.deepEqual(children.map(child => child.key), ['Dashboard', 'settings-general', 'UserManagement']);
  assert.equal(children.filter(child => child.key === 'UserManagement').length, 1);
  assert.deepEqual(children.find(child => child.key === 'UserManagement'), { key: 'UserManagement', label: '我的用户', visible: false });
});

test('notification and general legacy menus migrate to one MP settings entry', () => {
  const result = merge([{ key: 'group-system', children: [
    { key: 'settings-notifications', label: '通知设置' },
    { key: 'settings-general', label: '通用设置' },
  ] }]);
  const settings = result.flatMap(group => group.children).filter(child => child.key === 'settings-general');
  assert.equal(settings.length, 1);
  assert.equal(settings[0].label, '设置');
});

test('canonical and legacy entries still produce one key across existing groups', () => {
  const result = merge([
    { key: 'group-discovery', label: '发现', children: [{ key: 'UserCenter', label: '用户中心' }] },
    { key: 'group-system', label: '系统', children: [{ key: 'UserManagement', label: '用户管理' }] },
  ]);
  const children = result.flatMap(group => group.children);
  assert.equal(children.filter(child => child.key === 'UserManagement').length, 1);
  assert.equal(children.find(child => child.key === 'UserManagement').label, '用户管理');
  assert.equal(new Set(children.map(child => child.key)).size, children.length);
});

test('existing user management moves last without changing other menu order', () => {
  const result = merge([{ key: 'group-system', children: [
    { key: 'UserManagement', label: '用户管理' }, { key: 'settings-general', label: '我的设置' },
  ] }]);
  const system = result.find(group => group.key === 'group-system');
  assert.deepEqual(system.children.map(child => child.key), ['settings-general', 'UserManagement']);
  assert.equal(system.children[0].label, '我的设置');
  assert.equal(system.user_management_order_version, 1);
});

test('an explicit order saved after migration is preserved on future reads', () => {
  const result = merge([{ key: 'group-system', user_management_order_version: 1, children: [
    { key: 'UserManagement' }, { key: 'settings-general' },
  ] }]);
  assert.deepEqual(result.find(group => group.key === 'group-system').children.map(child => child.key), ['UserManagement', 'settings-general']);
});

test('standalone navigation retains its label, visibility and placement after reload', () => {
  const saved = [{ key: 'UserManagement', label: '我的用户', visible: false, user_management_order_version: 1 },
    { key: 'Dashboard', label: '首页', visible: true }];
  const result = merge(saved);
  assert.deepEqual(result.slice(0, 2).map(item => [item.key, item.label, item.visible]), [['UserManagement', '我的用户', false], ['Dashboard', '首页', true]]);
  assert.equal(result.flatMap(item => item.children || [item]).filter(item => item.key === 'Dashboard').length, 1);
  assert.deepEqual(merge(result), result);
});

test('unknown, duplicate and nested groups do not create unauthorized or duplicate entries', () => {
  const result = merge([{ key: 'unknown' }, { key: 'Dashboard' }, { key: 'Dashboard' },
    { key: 'group-discovery', children: [{ key: 'group-system' }, { key: 'Dashboard' }, { key: 'unknown' }] }]);
  const leaves = result.flatMap(item => item.children || [item]);
  assert.deepEqual(leaves.map(item => item.key), ['Dashboard', 'settings-general', 'UserManagement']);
  assert.equal(result.filter(item => item.key === 'group-discovery').length, 1);
});

test('group expansion defaults survive normalization, including false', () => {
  const result = merge([{ key: 'group-system', expanded: false }]);
  assert.equal(result.find(item => item.key === 'group-system').expanded, false);
  assert.equal(result.find(item => item.key === 'group-discovery').expanded, true);
});

test('a leaf moves out of a group before another root and can move back without losing edits', () => {
  const tree = merge([{ key: 'group-system', children: [{ key: 'settings-general', label: '系统设置', visible: false }] }]);
  assert.equal(moveNavigationItem(tree, 'settings-general', null, 'group-discovery', 'top'), true);
  assert.equal(tree[1].key, 'settings-general');
  assert.equal(tree[1].label, '系统设置');
  assert.equal(tree[1].visible, false);
  assert.equal(moveNavigationItem(tree, 'settings-general', 'group-discovery'), true);
  assert.equal(tree.find(item => item.key === 'group-discovery').children.at(-1).key, 'settings-general');
  assert.deepEqual(merge(tree), tree);
});

test('root groups can be reordered but cannot become children', () => {
  const tree = merge([]);
  assert.equal(moveNavigationItem(tree, 'group-system', null, 'group-discovery', 'top'), true);
  assert.equal(tree[0].key, 'group-system');
  const before = structuredClone(tree);
  assert.equal(moveNavigationItem(tree, 'group-system', 'group-discovery'), false);
  assert.deepEqual(tree, before);
});

test('same-list downward movement places a leaf after its target and invalid drops keep the tree', () => {
  const tree = merge([]);
  assert.equal(moveNavigationItem(tree, 'settings-general', 'group-system', 'UserManagement', 'bottom'), true);
  assert.deepEqual(tree.find(item => item.key === 'group-system').children.map(item => item.key), ['UserManagement', 'settings-general']);
  const before = structuredClone(tree);
  assert.equal(moveNavigationItem(tree, 'settings-general', null, 'unknown'), false);
  assert.equal(moveNavigationItem(tree, 'settings-general', 'unknown'), false);
  assert.deepEqual(tree, before);
});
