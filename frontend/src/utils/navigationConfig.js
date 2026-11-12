const legacyKeys = { MediaCenter: 'Dashboard', UserCenter: 'UserManagement', 'settings-notifications': 'settings-general' };
const legacyLabels = { MediaCenter: '媒体中心', UserCenter: '用户中心', 'settings-notifications': '通知设置', 'settings-general': '通用设置' };

export function mergeNavigationConfig(savedTree, defaults) {
  const catalogue = new Map();
  defaults.forEach(group => {
    catalogue.set(group.key, group);
    group.children?.forEach(item => catalogue.set(item.key, item));
  });
  const used = new Set(), result = [];
  function readItem(saved) {
    const key = legacyKeys[saved?.key] || saved?.key, base = catalogue.get(key);
    if (!base || used.has(key)) return null;
    used.add(key);
    const item = { key, label: saved.label && saved.label !== legacyLabels[saved.key] ? saved.label : base.defaultLabel, visible: saved.visible !== false };
    if (base.children) {
      Object.assign(item, { expanded: saved.expanded !== false, user_management_order_version: saved.user_management_order_version, children: [] });
      for (const child of Array.isArray(saved.children) ? saved.children : []) {
        if (catalogue.get(legacyKeys[child?.key] || child?.key)?.children) continue;
        const entry = readItem(child);
        if (entry) item.children.push(entry);
      }
    } else if (saved.user_management_order_version !== undefined) item.user_management_order_version = saved.user_management_order_version;
    return item;
  }
  for (const saved of Array.isArray(savedTree) ? savedTree : []) {
    const entry = readItem(saved);
    if (entry) result.push(entry);
  }
  for (const base of defaults) {
    let group = result.find(item => item.key === base.key);
    if (!group) {
      group = { key: base.key, label: base.defaultLabel, visible: true, expanded: true, children: [] };
      result.push(group);
      used.add(base.key);
    }
    for (const child of base.children || []) {
      if (used.has(child.key)) continue;
      used.add(child.key);
      group.children.push({ key: child.key, label: child.defaultLabel, visible: true });
    }
  }
  // Old defaults could leave UserCenter under discovery, before cover generation.
  // Once the new order is saved, keep the user's subsequent placement.
  const owner = result.find(item => item.children?.some(child => child.key === 'UserManagement'));
  if (owner && owner.user_management_order_version !== 1) {
    const index = owner.children.findIndex(child => child.key === 'UserManagement');
    const [user] = owner.children.splice(index, 1);
    (result.find(item => item.key === 'group-system') || owner).children.push(user);
  }
  result.forEach(item => { item.user_management_order_version = 1; });
  return result;
}

export function moveNavigationItem(tree, key, groupKey = null, targetKey = null, position = 'bottom') {
  if (key === targetKey || key === groupKey) return false;
  const sourceGroup = tree.find(item => item.children?.some(child => child.key === key));
  const source = sourceGroup?.children || tree;
  const sourceIndex = source.findIndex(item => item.key === key);
  const item = source[sourceIndex];
  const targetGroup = groupKey === null ? null : tree.find(entry => entry.key === groupKey && entry.children);
  if (!item || (groupKey !== null && !targetGroup) || (targetGroup && item.children)) return false;
  const target = targetGroup?.children || tree;
  if (targetKey !== null && !target.some(entry => entry.key === targetKey)) return false;
  source.splice(sourceIndex, 1);
  const index = targetKey === null ? target.length : target.findIndex(entry => entry.key === targetKey) + (position === 'bottom' ? 1 : 0);
  target.splice(index, 0, item);
  return true;
}
