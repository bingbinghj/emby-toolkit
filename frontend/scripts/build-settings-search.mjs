import { readFile, writeFile, rename } from 'node:fs/promises';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'vue/compiler-sfc';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const clean = value => String(value || '').replace(/\s+/g, ' ').trim();
const attribute = (node, name) => node.props?.find(prop => prop.type === 6 && prop.name === name)?.value?.content || '';
const elements = node => [node, ...(node.children || []).filter(child => child.type === 1).flatMap(elements)];
const text = node => clean((node.children || []).map(child => child.type === 2 ? child.content : child.type === 1 ? text(child) : '').join(' '));

/** Only static template labels and hints are searchable; never model values or script expressions. */
export function settingsCatalog(source, route) {
  const { descriptor, errors } = parse(source);
  if (errors.length) throw new Error(`设置模板无法解析：${route}`);
  const found = new Map();
  function visit(node, category, card, subtitle) {
    if (node.type !== 1) return;
    if (route === 'general' && node.tag === 'VWindowItem') category = attribute(node, 'value');
    if (node.tag === 'SettingCard') {
      card = attribute(node, 'title');
      subtitle = attribute(node, 'subtitle');
    } else if (!card && node.tag === 'VCard') {
      card = text((node.children || []).find(child => child.tag === 'VCardTitle') || {});
    }
    const field = attribute(node, 'data-field');
    if (field) {
      const children = elements(node).filter(child => child === node || !attribute(child, 'data-field'));
      const label = attribute(node, 'data-settings-label') || children.map(child => attribute(child, 'label')).find(Boolean)
        || children.filter(child => attribute(child, 'class').split(/\s+/).includes('settings-field-label')).map(text).find(Boolean)
        || attribute(node, 'aria-label');
      if (!label || !card || !category) throw new Error(`设置字段缺少实际标签或卡片分类：${route}/${field}`);
      const hints = children.flatMap(child => [attribute(child, 'hint'),
        attribute(child, 'class').split(/\s+/).includes('settings-hint') ? text(child) : '']);
      const keywords = [...new Set([label, card, subtitle, ...hints, ...children.filter(child => ['VRadio', 'VBtn'].includes(child.tag)).map(text)].map(clean).filter(Boolean))];
      const entry = { field, label: clean(label), card: clean(card), category, keywords, route };
      const previous = found.get(field);
      if (previous && !(route === 'notification' && field === 'notification_scope')) throw new Error(`设置字段重复：${route}/${field}`);
      if (previous) previous.keywords = [...new Set([...previous.keywords, ...keywords])];
      else found.set(field, entry);
    }
    for (const child of node.children || []) visit(child, category, card, subtitle);
  }
  for (const child of descriptor.template?.ast?.children || []) visit(child, route === 'notification' ? 'notification' : '', '', '');
  return [...found.values()];
}

export async function buildCatalog() {
  const files = [['general', 'GeneralSettingsPage.vue'], ['notification', 'NotificationSettingsPage.vue']];
  const catalog = [];
  for (const [route, name] of files) catalog.push(...settingsCatalog(await readFile(resolve(root, 'src/components/settings', name), 'utf8'), route));
  return catalog;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const catalog = await buildCatalog();
  const output = resolve(root, 'src/assets/settings-search.json');
  const temporary = `${output}.${process.pid}.tmp`;
  await writeFile(temporary, `${JSON.stringify(catalog, null, 2)}\n`);
  await rename(temporary, output);
  console.log(`已生成 ${catalog.length} 项设置搜索目录`);
}
