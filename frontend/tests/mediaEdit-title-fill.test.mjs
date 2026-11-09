import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { resolveMediaInfoLanguageSelection } from '../src/utils/mediaStreams.js';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
const start = page.indexOf('const fillLanguagesToTitles = ');
const end = page.indexOf('// 2. 打开编辑器', start);
const clone = value => JSON.parse(JSON.stringify(value));

const drafts = () => [
  { Type: 'Audio', Index: 1, Language: 'chi', Title: '台', Codec: 'ac3', IsDefault: true },
  { Type: 'Audio', Index: 2, Language: 'ace', Title: 'DVD粤', Codec: 'aac' },
  { Type: 'Subtitle', Index: 3, Language: 'ace', Title: 'DVD粤对', IsForced: true },
  { Type: 'Subtitle', Index: 4, Language: 'chi', Title: '台对' },
  { Type: 'Subtitle', Index: 5, Language: 'eng', Title: '英文字幕' },
];
const response = original => ({ data: {
  streams: original.map((stream, index) => ({
    ...stream,
    ...(index === 0 ? { Title: '台配国语' } : {}),
    ...(index === 1 ? { Title: 'DVD粤语', Language: 'yue' } : {}),
    ...(index === 4 ? { Title: '英文' } : {}),
  })),
  report: { changed: 3, changed_languages: 1, skipped_tracks: [{ ordinal: 1 }, { ordinal: 2 }], pending: [] },
} });

function fixture(post) {
  const streams = drafts();
  const messages = [];
  const calls = [];
  const context = vm.createContext({
    isFillingMediaInfoTitles: { value: false }, isSavingMediaInfo: { value: false },
    isRepairingTruehdCore: { value: false }, isDeletingMediaVersion: { value: false },
    currentEditMediaId: { value: '61803' }, mediaInfoLoadSequence: 1,
    showMediaInfoEditor: { value: true }, mediaStreams: { value: streams },
    displayStreams: { value: streams },
    mediaInfoLanguageSelections: { value: { 'Audio:1': 'chi', 'Audio:2': 'ace', 'Subtitle:3': 'ace', 'Subtitle:4': 'chs', 'Subtitle:5': 'eng' } },
    getMediaInfoStreamSelectionKey: stream => `${stream.Type}:${stream.Index}`,
    ensureMediaInfoLanguageOption: () => {},
    resolveInitialMediaInfoLanguageSelection: stream => resolveMediaInfoLanguageSelection([
      { value: 'chi', label: '国语', _languageTokens: ['chi'] },
      { value: 'yue', label: '粤语', _languageTokens: ['yue'] },
      { value: 'en', label: '英语', _languageTokens: ['en', 'eng'] },
    ], stream),
    axios: { post: async (url, payload) => { calls.push({ url, payload: clone(payload) }); return post(payload); } },
    message: Object.fromEntries(['success', 'info', 'warning', 'error'].map(type => [type, text => messages.push({ type, text })])),
  });
  vm.runInContext(`${page.slice(start, end)}\nthis.fill = fillLanguagesToTitles;`, context);
  return { context, streams, calls, messages };
}

test('editor uses shared preview and preserves stream references and skipped subtitle languages', async () => {
  const f = fixture(payload => response(payload.streams));
  const original = clone(f.streams);
  const references = [...f.streams];
  await f.context.fill();
  assert.equal(f.calls.length, 1);
  assert.equal(f.calls[0].url, '/api/media_info/fill_titles');
  assert.deepEqual(f.calls[0].payload.streams, original);
  assert.deepEqual(f.streams, response(original).data.streams);
  f.streams.forEach((stream, index) => assert.equal(stream, references[index]));
  assert.equal(f.context.mediaInfoLanguageSelections.value['Audio:2'], 'yue');
  assert.equal(f.streams[3].Language, 'chi');
  assert.equal(f.context.mediaInfoLanguageSelections.value['Subtitle:4'], 'chs');
  assert.match(f.messages.at(-1).text, /3.*2.*中文字幕/);
  assert.equal(f.context.isFillingMediaInfoTitles.value, false);
});

test('failed preview leaves the draft and language selections untouched', async () => {
  const f = fixture(() => Promise.reject(new Error('temporary failure')));
  const original = clone(f.streams);
  const selections = clone(f.context.mediaInfoLanguageSelections.value);
  await f.context.fill();
  assert.deepEqual(f.streams, original);
  assert.deepEqual(f.context.mediaInfoLanguageSelections.value, selections);
  assert.equal(f.messages.at(-1).type, 'error');
  assert.equal(f.context.isFillingMediaInfoTitles.value, false);
});

for (const change of ['target', 'load', 'draft', 'language', 'default', 'delete', 'close']) {
  test(`preview cannot overwrite a ${change} changed while waiting`, async () => {
    let finish;
    const f = fixture(() => new Promise(resolve => { finish = resolve; }));
    const original = clone(f.streams);
    const pending = f.context.fill();
    if (change === 'target') f.context.currentEditMediaId.value = '61804';
    if (change === 'load') f.context.mediaInfoLoadSequence += 1;
    if (change === 'draft') f.streams[0].Title = '用户刚修改的标题';
    if (change === 'language') f.streams[0].Language = 'eng';
    if (change === 'default') f.streams[0].IsDefault = false;
    if (change === 'delete') f.streams.splice(0, 1);
    if (change === 'close') f.context.showMediaInfoEditor.value = false;
    const edited = clone(f.streams);
    finish(response(original));
    await pending;
    assert.deepEqual(f.streams, edited);
  });
}

test('duplicate click does not start another request', async () => {
  let finish;
  const f = fixture(() => new Promise(resolve => { finish = resolve; }));
  const first = f.context.fill();
  await f.context.fill();
  assert.equal(f.calls.length, 1);
  finish(response(drafts()));
  await first;
});

test('changed response track order is rejected before any draft field is applied', async () => {
  const f = fixture(payload => {
    const result = response(payload.streams);
    result.data.streams.reverse();
    return result;
  });
  const original = clone(f.streams);
  await f.context.fill();
  assert.deepEqual(f.streams, original);
  assert.equal(f.messages.at(-1).type, 'error');
});

test('unknown or ambiguous tracks are reported for manual confirmation', async () => {
  const f = fixture(payload => ({ data: {
    streams: payload.streams,
    report: { changed: 0, skipped_tracks: [], pending: [{ reason: '语言为空或未知' }] },
  } }));
  await f.context.fill();
  assert.equal(f.messages.at(-1).type, 'warning');
  assert.match(f.messages.at(-1).text, /1.*语言未知.*手动确认/);
});

test('corrected language selects the existing alias option while saving the canonical code', async () => {
  const f = fixture(payload => {
    const result = response(payload.streams);
    result.data.streams[1].Language = 'eng';
    result.data.streams[1].Title = '英语';
    return result;
  });
  await f.context.fill();
  assert.equal(f.streams[1].Language, 'eng');
  assert.equal(f.context.mediaInfoLanguageSelections.value['Audio:2'], 'en');
});
