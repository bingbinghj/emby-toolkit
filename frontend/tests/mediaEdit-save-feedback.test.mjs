import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
const start = page.indexOf('const saveMediaInfo = ');
const end = page.indexOf('\n};', start) + 4;
const detailStart = page.indexOf('const mediaOperationDetails = ');
const detailEnd = page.indexOf('\n};', detailStart) + 4;
const closeStart = page.indexOf('const closeMediaInfoEditor = ');
const closeEnd = page.indexOf('\n};', closeStart) + 4;

function fixture(post) {
  const messages = [];
  const calls = [];
  const context = vm.createContext({
    isFillingMediaInfoTitles: { value: false }, isSavingMediaInfo: { value: false },
    isRepairingTruehdCore: { value: false }, isDeletingMediaVersion: { value: false },
    mediaInfoLoadSequence: 1, mediaInfoLanguageSelectRefs: new Map(),
    mediaInfoSaveFeedback: { value: null }, mediaInfoContext: { value: {} },
    mediaInfoData: { value: [{
      MediaSourceInfo: { MediaStreams: [{ Type: 'Audio', Index: 1, Title: '日语', Language: 'jpn' }] },
      Chapters: [],
    }] }, applyToSelectedEpisodes: { value: false },
    selectedEpisodeIds: { value: [] }, currentEditMediaId: { value: '101' },
    episodesList: { value: [{ emby_id: '101' }] }, showMediaInfoEditor: { value: true },
    axios: { post: async (url, payload) => {
      calls.push({ url, payload: JSON.parse(JSON.stringify(payload)) });
      return post();
    } },
    message: {
      loading: () => ({ destroy() {} }),
      ...Object.fromEntries(['success', 'warning', 'error'].map(type => [type, text => messages.push({ type, text })])),
    },
  });
  vm.runInContext(`${page.slice(detailStart, detailEnd)}\n${page.slice(closeStart, closeEnd)}\n${page.slice(start, end)}\nthis.save = saveMediaInfo; this.close = closeMediaInfoEditor;`, context);
  return { context, messages, calls };
}

async function save(result) {
  const f = fixture(async () => ({ data: result }));
  await f.context.save();
  return f;
}

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

for (const result of [
  { warnings: 1, warning_details: ['缓存同步失败'] },
  { metadata_refresh: { state: 'failed', failures: [{ reason: '刷新请求失败' }] } },
  { metadata_refresh: { state: 'unverified', failed_ids: ['101'] } },
  { metadata_refresh: { state: 'submitted' } },
  { video_thumbnails: { failed: 1, reason: '图片验收失败' } },
]) {
  test(`save keeps the editor open and exposes ${JSON.stringify(result)}`, async () => {
    const f = await save(result);
    assert.equal(f.context.showMediaInfoEditor.value, true);
    assert.equal(f.messages.at(-1).type, 'warning');
    assert.equal(f.context.mediaInfoSaveFeedback.value.type, 'warning');
    assert.ok(f.context.mediaInfoSaveFeedback.value.details.length > 0);
  });
}

test('verified save closes the editor with success', async () => {
  const f = await save({ metadata_refresh: { state: 'verified' }, warnings: 0 });
  assert.equal(f.context.showMediaInfoEditor.value, false);
  assert.equal(f.messages.at(-1).type, 'success');
});

test('skipped tracks keep their detailed episode warning', async () => {
  const f = await save({ batch_reports: [{ item_id: '101', skipped: [{ type: 'Audio', ordinal: 2, reason: '轨道范围不唯一' }] }] });
  assert.equal(f.context.showMediaInfoEditor.value, true);
  assert.match(f.context.episodesList.value[0].track_difference, /轨道范围不唯一/);
  assert.match(f.context.mediaInfoSaveFeedback.value.details[0], /101.*轨道范围不唯一/);
});

test('duplicate upstream failures have no empty identifier or repeated save prefix', async () => {
  const f = await save({
    warning_details: ['媒体信息 JSON 已保存，Emby 刷新或回读未确认：：无法访问上游服务'],
    metadata_refresh: { state: 'failed', failed_ids: ['61616'], failures: [{ reason: '无法访问上游服务' }] },
  });
  const details = Array.from(f.context.mediaInfoSaveFeedback.value.details);
  assert.equal(details.filter(detail => detail.includes('无法访问上游服务')).length, 1);
  assert.ok(details.every(detail => !detail.startsWith('：') && !detail.includes('：：')));
  assert.ok(details.includes('Emby 刷新尚未通过验收（61616）'));
});

test('failure de-duplication retains different target IDs', async () => {
  const f = await save({
    warning_details: ['媒体信息 JSON 已保存，Emby 刷新或回读未确认：61616：读取失败'],
    metadata_refresh: { state: 'failed', failures: [
      { item_id: '61616', reason: '读取失败' }, { item_id: '61617', reason: '读取失败' }, { reason: '读取失败' },
    ] },
  });
  const details = Array.from(f.context.mediaInfoSaveFeedback.value.details);
  assert.deepEqual(details.filter(detail => detail.includes('读取失败')), ['61616：读取失败', '61617：读取失败']);
});

for (const change of ['target', 'load', 'close']) {
  for (const outcome of ['success', 'failure']) {
    test(`late save ${outcome} cannot affect an editor changed by ${change}`, async () => {
      const pending = deferred();
      const f = fixture(() => pending.promise);
      const saving = f.context.save();
      assert.equal(f.context.isSavingMediaInfo.value, true);
      if (change === 'target') f.context.currentEditMediaId.value = '102';
      if (change === 'load') f.context.mediaInfoLoadSequence += 1;
      if (change === 'close') f.context.close();
      const visible = f.context.showMediaInfoEditor.value;
      const draft = JSON.parse(JSON.stringify(f.context.mediaInfoData.value));
      if (outcome === 'success') pending.resolve({ data: { metadata_refresh: { state: 'verified' } } });
      else pending.reject({ response: { data: { error: '原目标刷新失败' } } });
      await saving;
      assert.equal(f.calls.length, 1);
      assert.equal(f.calls[0].url, '/api/media_info/edit/101');
      assert.equal(f.context.showMediaInfoEditor.value, visible);
      assert.equal(f.context.mediaInfoSaveFeedback.value, null);
      assert.equal(f.messages.length, 0);
      assert.deepEqual(f.context.mediaInfoData.value, draft);
      assert.equal(f.context.isSavingMediaInfo.value, false);
    });
  }
}

test('reopening the same media keeps a newer editor separate from the pending save', async () => {
  const pending = deferred();
  const f = fixture(() => pending.promise);
  const saving = f.context.save();
  f.context.close();
  f.context.showMediaInfoEditor.value = true;
  pending.resolve({ data: { metadata_refresh: { state: 'verified' } } });
  await saving;
  assert.equal(f.context.currentEditMediaId.value, '101');
  assert.equal(f.context.showMediaInfoEditor.value, true);
  assert.equal(f.context.mediaInfoSaveFeedback.value, null);
  assert.deepEqual(f.messages, []);
});

test('duplicate save does not submit a second request or replace its loading state', async () => {
  const pending = deferred();
  const f = fixture(() => pending.promise);
  const saving = f.context.save();
  await f.context.save();
  assert.equal(f.calls.length, 1);
  assert.equal(f.context.isSavingMediaInfo.value, true);
  pending.resolve({ data: { metadata_refresh: { state: 'verified' } } });
  await saving;
  assert.equal(f.context.isSavingMediaInfo.value, false);
  assert.equal(f.messages.at(-1).type, 'success');
});

test('successful save preserves a draft changed after its request was sent', async () => {
  const pending = deferred();
  const f = fixture(() => pending.promise);
  const saving = f.context.save();
  f.context.mediaInfoData.value[0].MediaSourceInfo.MediaStreams[0].Title = '用户新标题';
  pending.resolve({ data: { metadata_refresh: { state: 'verified' } } });
  await saving;
  assert.equal(f.calls[0].payload.mediainfo[0].MediaSourceInfo.MediaStreams[0].Title, '日语');
  assert.equal(f.context.mediaInfoData.value[0].MediaSourceInfo.MediaStreams[0].Title, '用户新标题');
  assert.equal(f.context.showMediaInfoEditor.value, true);
  assert.equal(f.messages.at(-1).type, 'warning');
  assert.match(f.context.mediaInfoSaveFeedback.value.details.join('；'), /草稿已修改.*已保留新草稿/);
});

test('failed save preserves the editor and exposes the actual recovery failure', async () => {
  const f = fixture(async () => { throw { response: { data: {
    error: '保存失败', failures: [{ item_id: '101', reason: '原图片恢复尚未确认' }],
  } } }; });
  const draft = JSON.parse(JSON.stringify(f.context.mediaInfoData.value));
  await f.context.save();
  assert.equal(f.context.showMediaInfoEditor.value, true);
  assert.deepEqual(f.context.mediaInfoData.value, draft);
  assert.equal(f.messages.at(-1).type, 'error');
  assert.match(f.context.mediaInfoSaveFeedback.value.details.join('；'), /101.*原图片恢复尚未确认/);
});
