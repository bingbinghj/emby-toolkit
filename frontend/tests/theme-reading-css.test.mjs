import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { parse, compileStyle } from '@vue/compiler-sfc';

test('compiled wallpaper filters and visibility remain on the background layers', () => {
  const { descriptor } = parse(readFileSync(new URL('../src/components/WallpaperBackground.vue', import.meta.url), 'utf8'));
  const result = compileStyle({ source: descriptor.styles[0].content, id: 'data-v-reading', scoped: true });
  assert.deepEqual(result.errors, []);
  const rules = [...result.code.matchAll(/([^{}]+)\{([^{}]+)\}/g)]
    .map(([, selector, properties]) => ({ selector: selector.trim(), properties }))
    .filter(rule => rule.selector.includes('html['));
  assert.equal(rules.length, 10);
  for (const rule of rules) assert.match(rule.selector, /\.wallpaper-layer/, rule.selector);
  assert.ok(rules.some(rule => rule.selector.includes("data-glass-color-scheme='light'") && rule.properties.includes('visibility: hidden')));
});
