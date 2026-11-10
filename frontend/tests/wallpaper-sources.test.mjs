import test from 'node:test';
import assert from 'node:assert/strict';
import { loadMoviePilotFonts } from '../src/plugins/fonts.js';

let revision = 0;
async function fixture(read, saved = {}) {
  const storage = new Map(Object.entries(saved));
  globalThis.window = { setTimeout, clearTimeout, localStorage: {
    getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value),
  } };
  globalThis.fetch = read;
  const module = await import(`../src/composables/useWallpaper.js?wallpaper-test=${++revision}`);
  return { module, wallpaper: module.useWallpaper(), storage };
}
const response = data => ({ ok: true, json: async () => data });
const native = '/image_proxy/Items/101/Images/Backdrop/0?tag=original&quality=90';

test('MoviePilot rotation choices support one day and preserve existing custom preferences', async () => {
  const { module } = await fixture();
  const custom = module.normalizeWallpaperSettings({ mode: 'custom', customImage: 'https://example.com/old.jpg', rotationInterval: 120 });
  assert.equal(custom.rotationInterval, 120);
  assert.equal(custom.customImage, 'https://example.com/old.jpg');
  assert.equal(module.normalizeWallpaperSettings({ rotationInterval: 86400 }).rotationInterval, 86400);
  assert.deepEqual(module.wallpaperRotationIntervals.map(item => item.value), [0, 15, 60, 300, 600, 1800, 3600, 21600, 43200, 86400]);
});

test('wallpaper candidates allow native backdrops and filter invalid or duplicate addresses', async () => {
  const { module } = await fixture();
  assert.deepEqual(module.normalizeWallpaperCandidates({ images: [native, native, 'javascript:alert(1)', '/api/config', 'https://example.com/background.jpg'] }), [native, 'https://example.com/background.jpg']);
  assert.deepEqual(module.normalizeWallpaperCandidates({ images: [] }), []);
});

test('Emby source uses the protected API and receives native backdrop URLs', async () => {
  const requests = [];
  const { wallpaper } = await fixture(async (url, options) => { requests.push({ url, options }); return response({ images: [native] }); });
  wallpaper.updateSettings({ mode: 'remote', source: 'emby' });
  assert.equal(await wallpaper.refreshSource(), true);
  assert.equal(requests[0].url, '/api/wallpaper/emby');
  assert.equal(requests[0].options.credentials, 'same-origin');
  assert.deepEqual(wallpaper.images.value, [native]);
});

test('failed source reads retain the last loaded image list and visible background', async () => {
  let failing = false;
  const { wallpaper, storage } = await fixture(async () => failing
    ? { ok: false, json: async () => ({ error: '来源暂不可用' }) } : response({ images: [native] }));
  wallpaper.updateSettings({ mode: 'remote', source: 'emby' });
  await wallpaper.refreshSource();
  wallpaper.setPresentation({ activeImage: native, ready: true });
  failing = true;
  assert.equal(await wallpaper.refreshSource(), false);
  assert.deepEqual(wallpaper.images.value, [native]);
  assert.equal(wallpaper.activeImage.value, native);
  assert.equal(storage.get('etk-wallpaper-last-image'), native);
  assert.equal(wallpaper.sourceError.value, '来源暂不可用');
});

test('custom APIs receive no application cookie or authorization header', async () => {
  let request;
  const { wallpaper } = await fixture(async (url, options) => { request = { url, options }; return response(['https://example.com/first.jpg']); });
  wallpaper.updateSettings({ mode: 'remote', source: 'api', customApi: 'https://example.com/wallpapers' });
  assert.equal(await wallpaper.refreshSource(), true);
  assert.equal(request.url, 'https://example.com/wallpapers');
  assert.equal(request.options.credentials, 'omit');
  assert.equal(request.options.headers, undefined);
});

test('a late source response cannot replace a more recently selected source', async () => {
  const waiting = [];
  const { wallpaper } = await fixture((url, options) => new Promise(resolve => waiting.push({ url, options, resolve })));
  wallpaper.updateSettings({ mode: 'remote', source: 'tmdb' });
  const first = wallpaper.refreshSource();
  wallpaper.updateSettings({ source: 'emby' });
  const second = wallpaper.refreshSource();
  assert.equal(waiting[0].options.signal.aborted, true);
  waiting[1].resolve(response({ images: [native] }));
  assert.equal(await second, true);
  waiting[0].resolve(response({ images: ['https://example.com/stale.jpg'] }));
  assert.equal(await first, false);
  assert.deepEqual(wallpaper.images.value, [native]);
  assert.equal(wallpaper.sourceError.value, '');
});

test('unmount cancellation prevents even an abort-ignoring response from publishing state', async () => {
  let resolve;
  const { wallpaper } = await fixture(() => new Promise(done => { resolve = done; }));
  wallpaper.updateSettings({ mode: 'remote', source: 'bing' });
  const request = wallpaper.refreshSource();
  wallpaper.cancelSourceRequest();
  resolve(response({ images: ['https://example.com/cancelled.jpg'] }));
  assert.equal(await request, false);
  assert.deepEqual(wallpaper.images.value, []);
  assert.equal(wallpaper.sourceLoading.value, false);
});

test('a saved successful background remains available during a later source failure', async () => {
  const { wallpaper } = await fixture(async () => { throw new Error('网络不可用'); }, { 'etk-wallpaper-last-image': native });
  assert.equal(wallpaper.activeImage.value, native);
  wallpaper.updateSettings({ mode: 'remote', source: 'emby' });
  assert.equal(await wallpaper.refreshSource(), false);
  assert.equal(wallpaper.activeImage.value, native);
});

test('MP fonts load once without awaiting the external stylesheet', () => {
  const elements = new Map();
  globalThis.document = { getElementById: id => elements.get(id), createElement: () => ({}), head: { appendChild: element => elements.set(element.id, element) } };
  loadMoviePilotFonts();
  loadMoviePilotFonts();
  assert.equal(elements.size, 1);
  const stylesheet = elements.get('moviepilot-fonts');
  assert.equal(stylesheet.rel, 'stylesheet');
  assert.match(stylesheet.href, /Inter:wght@100;200;300;400;500;600;700/);
  assert.match(stylesheet.href, /Noto\+Sans\+SC:wght@400;500;700/);
  assert.match(stylesheet.href, /display=swap/);
  delete globalThis.document;
});
