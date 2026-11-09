import { readdir, readFile, rename, writeFile } from 'node:fs/promises'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { aliases } from 'vuetify/iconsets/mdi'
import { getIconData } from '@iconify/utils'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const names = new Set(Object.values(aliases).filter(value => typeof value === 'string'))
async function scan(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name)
    if (entry.isDirectory()) await scan(path)
    else if (/\.(vue|js|ts)$/.test(entry.name)) {
      const text = await readFile(path, 'utf8')
      for (const match of text.matchAll(/\b(?:mdi|tabler)-[a-z0-9-]+/g)) names.add(match[0])
    }
  }
}
await scan(resolve(root, 'src'))
const collections = []
for (const prefix of ['mdi', 'tabler']) {
  const source = JSON.parse(await readFile(resolve(root, 'node_modules', '@iconify-json', prefix, 'icons.json'), 'utf8'))
  const icons = {}
  for (const name of names) {
    if (!name.startsWith(`${prefix}-`)) continue
    const key = name.slice(prefix.length + 1)
    const icon = getIconData(source, key)
    if (icon) icons[key] = icon
  }
  collections.push({ prefix, width: source.width, height: source.height, icons })
}
const output = resolve(root, 'src/plugins/icons.json')
const temporary = `${output}.${process.pid}.tmp`
await writeFile(temporary, JSON.stringify(collections))
await rename(temporary, output)
console.log(`已生成 ${collections.reduce((count, collection) => count + Object.keys(collection.icons).length, 0)} 个本地图标`)
