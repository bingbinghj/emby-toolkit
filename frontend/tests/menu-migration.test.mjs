import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const source = readFileSync(new URL('../src/MainLayout.vue', import.meta.url), 'utf8');
const start = source.indexOf('const syncMenuConfig = ');
const end = source.indexOf('\n};', start) + 3;
function merge(saved) {
  const groups = [
    { key: 'group-discovery', defaultLabel: '发现', children: [{ key: 'Dashboard', defaultLabel: '仪表盘' }] },
    { key: 'group-system', defaultLabel: '系统', children: [{ key: 'UserManagement', defaultLabel: '用户管理' }, { key: 'settings-general', defaultLabel: '通用设置' }] },
  ];
  const map = new Map();
  for (const group of groups) {
    map.set(group.key, { ...group, isGroup: true });
    for (const child of group.children) map.set(child.key, { ...child, isGroup: false });
  }
  const context = vm.createContext({ baseMenuOptions: { value: groups }, baseMenuMap: { value: map } });
  vm.runInContext(source.slice(start, end) + '\nthis.merge = syncMenuConfig;', context);
  return JSON.parse(JSON.stringify(context.merge(saved)));
}

test('legacy user entry is not duplicated when the missing system group is filled in', () => {
  const result = merge([{ key: 'group-discovery', label: '发现', children: [
    { key: 'MediaCenter', label: '媒体中心', visible: true },
    { key: 'UserCenter', label: '我的用户', visible: false },
  ] }]);
  const children = result.flatMap(group => group.children);
  assert.deepEqual(children.map(child => child.key), ['Dashboard', 'UserManagement', 'settings-general']);
  assert.equal(children.filter(child => child.key === 'UserManagement').length, 1);
  assert.deepEqual(children.find(child => child.key === 'UserManagement'), { key: 'UserManagement', label: '我的用户', visible: false });
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
