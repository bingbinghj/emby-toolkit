import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
const start = page.indexOf('const handleMediaInfoButtonClick = ');
const end = page.indexOf('\n};', start) + 4;
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

function fixture() {
  const requests = [], notices = [];
  const context = vm.createContext({
    itemId: { value: 'A' }, itemDetails: { value: { item_type: 'Series' } },
    showEpisodeSelector: { value: false }, isFetchingEpisodes: { value: false },
    episodesList: { value: [] }, selectedSeasonNumber: { value: null },
    episodeSeasonGroups: { value: [{ season_number: 1 }] }, selectedBatchVersionIndex: { value: null },
    mediaDetailsLoadSequence: 1, episodeSelectorLoadSequence: 0,
    clearSelectedEpisodes() {}, openMediaInfoEditor() {},
    message: { error: text => notices.push(text) },
    axios: { get: path => { const pending = deferred(); requests.push({ path, ...pending }); return pending.promise; } },
  });
  vm.runInContext(page.slice(start, end) + '\nthis.open = handleMediaInfoButtonClick;', context);
  const move = target => { context.showEpisodeSelector.value = false; context.itemId.value = target; context.mediaDetailsLoadSequence++; };
  return { context, requests, notices, move };
}

test('a delayed episode list cannot replace a newer media selector', async () => {
  const f = fixture();
  const old = f.context.open();
  f.move('B');
  const current = f.context.open();
  f.requests[1].resolve({ data: [{ emby_id: 'B01' }] }); await current;
  f.requests[0].resolve({ data: [{ emby_id: 'A01' }] }); await old;
  assert.equal(f.context.itemId.value, 'B');
  assert.equal(f.context.episodesList.value[0].emby_id, 'B01');
  assert.equal(f.context.showEpisodeSelector.value, true);
});

test('an old episode response does not clear a newer selector loading state', async () => {
  const f = fixture();
  const old = f.context.open();
  f.move('B');
  const current = f.context.open();
  f.requests[0].resolve({ data: [{ emby_id: 'A01' }] }); await old;
  assert.equal(f.context.isFetchingEpisodes.value, true);
  assert.equal(f.context.episodesList.value.length, 0);
  f.requests[1].resolve({ data: [{ emby_id: 'B01' }] }); await current;
  assert.equal(f.context.isFetchingEpisodes.value, false);
});

test('an obsolete episode failure does not close the current selector or show a message', async () => {
  const f = fixture();
  const old = f.context.open();
  f.move('B');
  const current = f.context.open();
  f.requests[0].reject(new Error('old fixture failure')); await old;
  assert.equal(f.context.showEpisodeSelector.value, true);
  assert.equal(f.context.isFetchingEpisodes.value, true);
  assert.equal(f.notices.length, 0);
  f.requests[1].resolve({ data: [{ emby_id: 'B01' }] }); await current;
});

test('closing and reopening the same media selector invalidates its first read', async () => {
  const f = fixture();
  const old = f.context.open();
  f.context.showEpisodeSelector.value = false;
  const current = f.context.open();
  f.requests[1].resolve({ data: [{ emby_id: 'A-new' }] }); await current;
  f.requests[0].resolve({ data: [{ emby_id: 'A-old' }] }); await old;
  assert.equal(f.context.episodesList.value[0].emby_id, 'A-new');
});

test('a closed selector ignores its delayed error', async () => {
  const f = fixture();
  const pending = f.context.open();
  f.context.showEpisodeSelector.value = false;
  f.requests[0].reject(new Error('closed fixture failure')); await pending;
  assert.equal(f.notices.length, 0);
});

test('unmount invalidates the episode list read before its response arrives', async () => {
  const f = fixture();
  const pending = f.context.open();
  f.context.mediaDetailsLoadSequence++;
  f.context.showEpisodeSelector.value = false;
  f.requests[0].resolve({ data: [{ emby_id: 'late-A' }] }); await pending;
  assert.equal(f.context.episodesList.value.length, 0);
});

test('a current episode list failure still closes its selector and releases loading', async () => {
  const f = fixture();
  const pending = f.context.open();
  f.requests[0].reject(new Error('current fixture failure')); await pending;
  assert.equal(f.context.showEpisodeSelector.value, false);
  assert.equal(f.context.isFetchingEpisodes.value, false);
  assert.equal(f.notices.length, 1);
});
