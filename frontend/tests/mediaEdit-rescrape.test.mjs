import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { queueReceiptMatches } from '../src/composables/useMediaOrganizeQueue.js';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
const definition = name => {
  const start = page.indexOf(`const ${name} = `);
  return page.slice(start, page.indexOf('\n};', start) + 4);
};
for (const kind of ['Movie', 'Series', 'Season', 'Episode']) {
  test(`current ${kind} rescrape submits only its id and closes confirmation without waiting for the job`, async () => {
    let resolve, confirmation;
    const pending = new Promise(done => { resolve = done; });
    const calls = [], errors = [];
    const state = vm.createContext({
      itemDetails: { value: { item_id: '201', item_type: kind } }, itemId: { value: 'wrong-parent' },
      itemTypeInChinese: { value: kind }, isSubmittingRescrape: { value: false }, rescrapeQueueOpen: { value: false },
      rescrapeQueue: { refresh() {} }, queueReceiptMatches,
      confirmMediaAction: (...args) => { confirmation = args; },
      axios: { post: async (...args) => { calls.push(args); return pending; } },
      message: { error: error => errors.push(error) },
    });
    vm.runInContext(definition('confirmCurrentRescrape') + definition('submitCurrentRescrape') + '\nthis.confirm = confirmCurrentRescrape;', state);
    state.confirm();
    assert.match(confirmation[1], /保留已有中文翻译/);
    assert.equal(confirmation[2](), undefined);
    assert.equal(calls.length, 1);
    assert.deepEqual(JSON.parse(JSON.stringify(calls[0])), ['/api/actions/reorganize', { item_ids: ['201'], mode: 'rescrape' }]);
    state.confirm();
    assert.equal(calls.length, 1);
    resolve({ status: 202, data: { mode: 'rescrape', queued_count: 1, duplicate_count: 0, queue_ids: ['q'], items: [{ id: 'q', item_id: '201', mode: 'rescrape', status: 'queued' }] } });
    await new Promise(done => setImmediate(done));
    assert.equal(state.rescrapeQueueOpen.value, true);
    assert.equal(state.isSubmittingRescrape.value, false);
    assert.deepEqual(errors, []);
  });
}
