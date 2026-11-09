import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

// Load the real Workbox strategy selected by the actual Vite PWA configuration.
globalThis.self = globalThis;
globalThis.location = new URL('https://etk.example/');
globalThis.registration = { scope: 'https://etk.example/' };
process.env.NODE_ENV = 'production';
const source = readFileSync(new URL('../vite.config.js', import.meta.url), 'utf8');
let workbox;
vm.runInNewContext(source.replace(/^import .+$/gm, '').replaceAll('import.meta.url', '__VITE_CONFIG_URL__').replace('export default', 'globalThis.config ='), {
  defineConfig: config => config, vue: () => [], version: 'test',
  vuetify: () => [], AutoImport: () => [], fileURLToPath, URL,
  __VITE_CONFIG_URL__: new URL('../vite.config.js', import.meta.url).href,
  VitePWA: options => { workbox = options.workbox; return []; },
});
const rule = workbox.runtimeCaching.find(rule => rule.urlPattern.test('https://etk.example/api/status'));
const { [rule.handler]: Strategy } = await import(`workbox-strategies/${rule.handler}.js`);

for (const path of ['status', 'tasks/result/101', 'media_for_editing/61616', 'config']) {
  test(`failed API ${path} does not return a stale cached response`, async () => {
    const strategy = new Strategy(rule.options || {});
    let cachedReads = 0;
    const handler = {
      waitUntil: promise => promise,
      fetch: async () => { throw new Error('network unavailable'); },
      fetchAndCachePut: async () => { throw new Error('network unavailable'); },
      cacheMatch: async () => { cachedReads++; return Response.json({ old_data: true }); },
    };
    await assert.rejects(strategy._handle(new Request(`https://etk.example/api/${path}`), handler));
    assert.equal(cachedReads, 0);
  });
}
test('API requests return only their current network response', async () => {
  const strategy = new Strategy(rule.options || {});
  assert.equal(strategy.fetchOptions.cache, 'no-store');
  const current = Response.json({ version: 'current' });
  const response = await strategy._handle(new Request('https://etk.example/api/status'), {
    fetch: async () => current,
    cacheMatch: async () => { throw new Error('cache must not be consulted'); },
  });
  assert.deepEqual(await response.json(), { version: 'current' });
});
test('worker activation removes the old API cache and retains static caches', async () => {
  assert.ok(workbox.importScripts.includes('cleanup-api-cache.js'));
  const script = readFileSync(new URL('../public/cleanup-api-cache.js', import.meta.url), 'utf8');
  const stored = new Set(['api-cache', 'workbox-precache-static', 'image-cache']);
  const events = new Map();
  vm.runInNewContext(script, {
    self: { addEventListener: (name, listener) => events.set(name, listener) },
    caches: { delete: async name => stored.delete(name) },
  });
  let cleanup;
  events.get('activate')({ waitUntil: promise => { cleanup = promise; } });
  await cleanup;
  assert.deepEqual([...stored], ['workbox-precache-static', 'image-cache']);
  assert.ok(workbox.globPatterns.length > 0);
  assert.equal(workbox.navigateFallback, '/index.html');
});
