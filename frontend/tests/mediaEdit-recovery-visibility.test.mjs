import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { computed, ref, unref } from 'vue';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
const alerts = [...page.matchAll(/<VAlert ([^>]*data-testid="media-recovery-alert")>/g)].map(match => match[1]);
function fixture(records, pageId = '101', editorId = pageId) {
  const context = vm.createContext({ computed, itemId: ref(pageId), currentEditMediaId: ref(editorId),
    mediaRecoveryState: ref({ pending: records.length > 0, records }), mediaRecoveryError: ref(''),
    isCheckingMediaRecovery: ref(false), isRecoveringMediaInfo: ref(false) });
  const names = ['mediaRecoveryCleanupOnly', 'mediaRecoveryRecordsFor', 'pageMediaRecoveryRecords',
    'editorMediaRecoveryRecords', 'isRecoveryCleanupOnly', 'mediaRecoveryBlocked'];
  for (const name of names) {
    const start = page.indexOf(`const ${name} = `);
    if (start < 0) continue;
    const end = name === 'mediaRecoveryRecordsFor' ? page.indexOf('\n};', start) + 3 : page.indexOf('\n', start);
    vm.runInContext(`${page.slice(start, end)}\nthis.${name} = ${name};`, context);
  }
  const expression = value => {
    const keys = Object.keys(context);
    return new Function(...keys, `return (${value});`)(...keys.map(key => unref(context[key])));
  };
  return { context, visible: index => Boolean(expression(alerts[index].match(/v-if="([^"]+)"/)[1])),
    title: index => expression(alerts[index].match(/:title="([^"]+)"/)[1]) };
}
const pending = (id, targets, kind = 'media-reload') => ({ id, target_ids: targets, kind });

test('unrelated recovery does not appear on the page or in its media editor and still blocks saving', () => {
  const f = fixture([pending('other', ['61688', '61690'])]);
  assert.equal(f.visible(0), false);
  assert.equal(f.visible(1), false);
  assert.equal(f.context.mediaRecoveryBlocked.value, true);
});

test('page and editor independently show recovery for their actual native item IDs', () => {
  const f = fixture([pending('selected-episode', ['102'])], 'series', '102');
  assert.equal(f.visible(0), false);
  assert.equal(f.visible(1), true);
  assert.equal(f.title(1), '媒体信息需要恢复');
});

test('switching versions hides unrelated recovery and shows matching recovery without another request', () => {
  const f = fixture([pending('version-102', ['102'])]);
  assert.equal(f.visible(1), false);
  f.context.currentEditMediaId.value = '102';
  assert.equal(f.visible(1), true);
  f.context.currentEditMediaId.value = '103';
  assert.equal(f.visible(1), false);
});

test('a cleanup title is determined by the matching records rather than another media recovery', () => {
  const f = fixture([pending('cleanup', [101], 'verified-cleanup'), pending('other', ['102'])]);
  assert.equal(f.visible(0), true);
  assert.equal(f.title(0), '恢复备份待清理');
  assert.equal(f.title(1), '恢复备份待清理');
});

test('an empty current ID does not match an empty or unavailable recovery target', () => {
  const f = fixture([pending('unknown', []), pending('empty', [''])], '', '');
  assert.equal(f.visible(0), false);
  assert.equal(f.visible(1), false);
});
