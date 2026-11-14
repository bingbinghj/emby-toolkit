import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(process.env.ETK_NAV_LAYOUT_SOURCE || new URL('../src/layouts/components/DefaultLayout.vue', import.meta.url), 'utf8');
const start = source.indexOf('function navigate(');
assert.ok(start >= 0);
const definition = source.slice(start, source.indexOf('\n}', start) + 2);
function fixture() {
  const emitted = [];
  const context = vm.createContext({ overlayOpen: { value: true }, openHorizontalGroup: { value: 'group-system' },
    emit: (...args) => emitted.push(args) });
  vm.runInContext(`${definition}\nthis.click = navigate;`, context);
  return { context, emitted };
}

for (const [name, event] of [
  ['Control', { button: 0, ctrlKey: true }],
  ['Command', { button: 0, metaKey: true }],
  ['Shift', { button: 0, shiftKey: true }],
  ['Alt', { button: 0, altKey: true }],
  ['middle button', { button: 1 }],
  ['right button', { button: 2 }],
  ['blank target', { button: 0, currentTarget: { getAttribute: () => '_blank' } }],
]) test(`${name} navigation leaves the current page and open navigation intact`, () => {
  const f = fixture();
  f.context.click(event);
  assert.deepEqual(f.emitted, [], 'native link actions must not trigger the parent router.push');
  assert.equal(f.context.overlayOpen.value, true);
  assert.equal(f.context.openHorizontalGroup.value, 'group-system');
});

for (const defaultPrevented of [false, true]) test(`ordinary link activation closes navigation after ${defaultPrevented ? 'Vuetify handles' : 'before Vuetify handles'} the click`, () => {
  const f = fixture();
  f.context.click({ button: 0, defaultPrevented });
  assert.equal(f.context.overlayOpen.value, false);
  assert.equal(f.context.openHorizontalGroup.value, null);
  assert.deepEqual(f.emitted, [], 'Vuetify performs the single route navigation');
});
