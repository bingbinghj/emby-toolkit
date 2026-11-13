import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { buildMediaInfoLanguageOptions } from '../src/utils/mediaStreams.js';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
const definition = name => {
  const start = page.indexOf(`const ${name} = `);
  assert.ok(start >= 0, name);
  return page.slice(start, page.indexOf('\n};', start) + 4);
};
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const info = title => ({ data: { sha1: 'a'.repeat(40), versions: [], iso_language_options: [],
  mediainfo: [{ MediaSourceInfo: { MediaStreams: [
    { Type: 'Audio', Index: 1, Language: 'jpn', Title: title },
  ] } }] } });
const mapping = [{ label: '日语', value: 'jpn', aliases: ['jpn', 'ja'] }];

function fixture() {
  const requests = [], notices = [], waiters = new Map();
  const context = vm.createContext({
    mediaInfoLoadSequence: 0, mediaInfoLanguageMappingPromise: null,
    mediaInfoEditorBooted: { value: false }, showMediaInfoEditor: { value: false },
    currentEditMediaId: { value: null }, mediaInfoSaveFeedback: { value: null },
    mediaInfoContext: { value: {} }, mediaInfoVersions: { value: [] },
    mediaInfoIsoLanguageOptions: { value: [] }, mediaInfoData: { value: null },
    mediaStreams: { value: [] }, mediaInfoLanguageSelections: { value: {} },
    mediaInfoLanguageSelectRefs: new Map(), mediaInfoLanguageUndoStack: [],
    deletedMediaStreamSnapshots: { value: [] }, mediaInfoLoadedSnapshot: '',
    mediaInfoStreamSnapshotMap: new WeakMap(), audioLanguageOptions: { value: [] },
    subtitleLanguageOptions: { value: [] }, buildMediaInfoLanguageOptions,
    createMediaStreamSnapshots: streams => streams.map(stream => ({ ...stream })),
    ensureMediaInfoLanguageOption() {}, getMediaInfoStreamSelectionKey: stream => `${stream.Type}:${stream.Index}`,
    resolveInitialMediaInfoLanguageSelection: stream => stream.Language,
    loadMediaRecovery: () => {}, console: { error() {} },
    message: { loading: () => { notices.push('loading'); return { destroy: () => notices.push('destroy') }; },
      error: text => notices.push(text) },
    axios: { get: path => {
      requests.push(path);
      const pending = deferred();
      if (!waiters.has(path)) waiters.set(path, []);
      waiters.get(path).push(pending);
      return pending.promise;
    } },
  });
  vm.runInContext(`${['fetchLanguageMapping', 'closeMediaInfoEditor', 'openMediaInfoEditor'].map(definition).join('\n')}\nthis.open = openMediaInfoEditor; this.close = closeMediaInfoEditor;`, context);
  return { context, requests, notices, pending: (path, index = 0) => waiters.get(path)?.[index] };
}

test('the first open starts media and language requests together and shows feedback immediately', async () => {
  const f = fixture();
  const opening = f.context.open('101');
  assert.deepEqual(new Set(f.requests), new Set(['/api/media_info/edit/101', '/api/mappings/config/language_mapping']));
  assert.equal(f.notices[0], 'loading');
  f.pending('/api/media_info/edit/101').resolve(info('原日语'));
  await Promise.resolve();
  assert.equal(f.context.showMediaInfoEditor.value, false);
  f.pending('/api/mappings/config/language_mapping').resolve({ data: mapping });
  await opening;
  assert.equal(f.context.showMediaInfoEditor.value, true);
  assert.equal(f.context.mediaInfoEditorBooted.value, true);
  assert.equal(f.context.mediaStreams.value[0].Title, '原日语');
});

test('two rapid opens share one language request and an older media response cannot replace the current editor', async () => {
  const f = fixture();
  const old = f.context.open('101');
  const current = f.context.open('102');
  assert.equal(f.requests.filter(path => path.endsWith('language_mapping')).length, 1);
  assert.ok(f.pending('/api/media_info/edit/102'));
  f.pending('/api/media_info/edit/102').resolve(info('新媒体'));
  f.pending('/api/mappings/config/language_mapping').resolve({ data: mapping });
  await current;
  f.pending('/api/media_info/edit/101').resolve(info('旧媒体'));
  await old;
  assert.equal(f.context.currentEditMediaId.value, '102');
  assert.equal(f.context.mediaStreams.value[0].Title, '新媒体');
});

test('closing during initial parallel requests invalidates the pending open', async () => {
  const f = fixture();
  const opening = f.context.open('101');
  f.context.close();
  assert.ok(f.pending('/api/media_info/edit/101'));
  f.pending('/api/media_info/edit/101').resolve(info('迟到媒体'));
  f.pending('/api/mappings/config/language_mapping').resolve({ data: mapping });
  await opening;
  assert.equal(f.context.showMediaInfoEditor.value, false);
  assert.equal(f.context.mediaInfoEditorBooted.value, false);
  assert.equal(f.context.mediaInfoData.value, null);
});

test('closing a retained editor closes its language menu and keeps its live selector reference', () => {
  const f = fixture();
  let blurred = 0;
  const selector = { menu: true, search: '英语', blur: () => { blurred++; } };
  f.context.mediaInfoLanguageSelectRefs.set('101:Audio:1:1', selector);
  f.context.showMediaInfoEditor.value = true;
  f.context.close();
  assert.equal(selector.menu, false);
  assert.equal(blurred, 1);
  assert.equal(f.context.mediaInfoLanguageSelectRefs.get('101:Audio:1:1'), selector);
  assert.equal(f.context.showMediaInfoEditor.value, false);
});

test('reopening reads fresh media instead of keeping a cancelled draft and reuses cached languages', async () => {
  const f = fixture();
  const first = f.context.open('101');
  f.pending('/api/media_info/edit/101').resolve(info('已保存标题'));
  f.pending('/api/mappings/config/language_mapping').resolve({ data: mapping });
  await first;
  f.context.mediaStreams.value[0].Title = '取消的草稿';
  f.context.close();
  const reopen = f.context.open('101');
  f.pending('/api/media_info/edit/101', 1).resolve(info('已保存标题'));
  await reopen;
  assert.equal(f.context.mediaStreams.value[0].Title, '已保存标题');
  assert.equal(f.requests.filter(path => path.endsWith('language_mapping')).length, 1);
  assert.equal(f.context.mediaInfoEditorBooted.value, true);
});

test('a failed language request leaves media unchanged and can be retried on the next open', async () => {
  const f = fixture();
  const first = f.context.open('101');
  f.pending('/api/media_info/edit/101').resolve(info('原日语'));
  f.pending('/api/mappings/config/language_mapping').reject(new Error('language server unavailable'));
  await first;
  assert.equal(f.context.showMediaInfoEditor.value, true);
  assert.equal(f.context.mediaStreams.value[0].Language, 'jpn');
  f.context.close();
  const retry = f.context.open('101');
  assert.equal(f.requests.filter(path => path.endsWith('language_mapping')).length, 2);
  f.pending('/api/media_info/edit/101', 1).resolve(info('原日语'));
  f.pending('/api/mappings/config/language_mapping', 1).resolve({ data: mapping });
  await retry;
  assert.equal(f.context.audioLanguageOptions.value[0].label, '日语');
  assert.equal(f.context.mediaStreams.value[0].Title, '原日语');
});
