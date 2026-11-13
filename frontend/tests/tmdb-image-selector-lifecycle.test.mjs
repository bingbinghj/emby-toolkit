import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
const start = page.indexOf('const openTmdbSelector = ');
const end = page.indexOf('\n};', start) + 4;
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const images = media => ({ posters: [{ id: media + '-poster' }], backdrops: [{ id: media + '-backdrop' }] });

function fixture() {
  const requests = [], notices = [];
  const context = vm.createContext({
    itemId: { value: 'A' }, currentEditImageType: { value: '' },
    imageTypes: [{ type: 'Primary', label: 'Poster', aspect: '2/3' }, { type: 'Backdrop', label: 'Backdrop', aspect: '16/9' }],
    currentTmdbImageLabel: { value: '' }, currentTmdbImageAspect: { value: '' },
    showTmdbSelector: { value: false }, isFetchingTmdbImages: { value: false },
    tmdbImagesCache: { value: null }, currentTmdbImages: { value: [] },
    mediaDetailsLoadSequence: 1, tmdbImagesLoadSequence: 0,
    message: { error: text => notices.push(text) },
    axios: { get: path => { const pending = deferred(); requests.push({ path, ...pending }); return pending.promise; } },
  });
  vm.runInContext(page.slice(start, end) + '\nthis.open = openTmdbSelector;', context);
  const move = media => { context.showTmdbSelector.value = false; context.itemId.value = media; context.mediaDetailsLoadSequence++; context.tmdbImagesCache.value = null; };
  return { context, requests, notices, move };
}

test('an old media poster request cannot populate the new media backdrop selector or cache', async () => {
  const f = fixture();
  const old = f.context.open('Primary', 'posters');
  f.move('B');
  const current = f.context.open('Backdrop', 'backdrops');
  f.requests[1].resolve({ data: images('B') }); await current;
  f.requests[0].resolve({ data: images('A') }); await old;
  assert.equal(f.context.currentTmdbImages.value[0].id, 'B-backdrop');
  assert.equal(f.context.tmdbImagesCache.value.posters[0].id, 'B-poster');
  assert.equal(f.context.currentEditImageType.value, 'Backdrop');
});

test('an old image type response cannot replace the current type gallery on the same media', async () => {
  const f = fixture();
  const old = f.context.open('Primary', 'posters');
  const current = f.context.open('Backdrop', 'backdrops');
  f.requests[1].resolve({ data: images('new') }); await current;
  f.requests[0].resolve({ data: images('old') }); await old;
  assert.equal(f.context.currentTmdbImages.value[0].id, 'new-backdrop');
  assert.equal(f.context.tmdbImagesCache.value.posters[0].id, 'new-poster');
});

test('an old image request does not clear loading while the current gallery is still pending', async () => {
  const f = fixture();
  const old = f.context.open('Primary', 'posters');
  f.move('B');
  const current = f.context.open('Backdrop', 'backdrops');
  f.requests[0].resolve({ data: images('A') }); await old;
  assert.equal(f.context.isFetchingTmdbImages.value, true);
  assert.equal(f.context.tmdbImagesCache.value, null);
  f.requests[1].resolve({ data: images('B') }); await current;
  assert.equal(f.context.isFetchingTmdbImages.value, false);
});

test('an obsolete image error cannot close a newer selector or show its failure', async () => {
  const f = fixture();
  const old = f.context.open('Primary', 'posters');
  f.move('B');
  const current = f.context.open('Backdrop', 'backdrops');
  f.requests[0].reject(new Error('old fixture failure')); await old;
  assert.equal(f.context.showTmdbSelector.value, true);
  assert.equal(f.context.isFetchingTmdbImages.value, true);
  assert.equal(f.notices.length, 0);
  f.requests[1].resolve({ data: images('B') }); await current;
});

test('closing and reopening the same image selector invalidates its first request', async () => {
  const f = fixture();
  const old = f.context.open('Primary', 'posters');
  f.context.showTmdbSelector.value = false;
  const current = f.context.open('Primary', 'posters');
  f.requests[1].resolve({ data: images('new') }); await current;
  f.requests[0].resolve({ data: images('old') }); await old;
  assert.equal(f.context.currentTmdbImages.value[0].id, 'new-poster');
});

test('a closed image selector does not report a delayed request error', async () => {
  const f = fixture();
  const pending = f.context.open('Primary', 'posters');
  f.context.showTmdbSelector.value = false;
  f.requests[0].reject(new Error('closed fixture failure')); await pending;
  assert.equal(f.notices.length, 0);
});

test('unmount invalidates late image results before they can enter the cache', async () => {
  const f = fixture();
  const pending = f.context.open('Primary', 'posters');
  f.context.mediaDetailsLoadSequence++;
  f.context.showTmdbSelector.value = false;
  f.requests[0].resolve({ data: images('late') }); await pending;
  assert.equal(f.context.tmdbImagesCache.value, null);
  assert.equal(f.context.currentTmdbImages.value.length, 0);
});

test('a current cached image collection is reused without another request', async () => {
  const f = fixture();
  const pending = f.context.open('Primary', 'posters');
  f.requests[0].resolve({ data: images('A') }); await pending;
  await f.context.open('Backdrop', 'backdrops');
  assert.equal(f.requests.length, 1);
  assert.equal(f.context.currentTmdbImages.value[0].id, 'A-backdrop');
  assert.equal(f.context.currentTmdbImageAspect.value, '16/9');
  assert.equal(f.context.isFetchingTmdbImages.value, false);
});

test('a current image error still closes its selector and releases loading', async () => {
  const f = fixture();
  const pending = f.context.open('Primary', 'posters');
  f.requests[0].reject(new Error('current fixture failure')); await pending;
  assert.equal(f.context.showTmdbSelector.value, false);
  assert.equal(f.context.isFetchingTmdbImages.value, false);
  assert.equal(f.notices.length, 1);
});
