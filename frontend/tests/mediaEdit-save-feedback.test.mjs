import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
const start = page.indexOf('const saveMediaInfo = ');
const end = page.indexOf('\n};', start) + 4;
const detailStart = page.indexOf('const mediaOperationDetails = ');
const detailEnd = page.indexOf('\n};', detailStart) + 4;

async function save(result) {
  const messages = [];
  const context = vm.createContext({
    isFillingMediaInfoTitles: { value: false }, isSavingMediaInfo: { value: false },
    mediaInfoSaveFeedback: { value: null }, mediaInfoContext: { value: {} },
    mediaInfoData: { value: [] }, applyToSelectedEpisodes: { value: false },
    selectedEpisodeIds: { value: [] }, currentEditMediaId: { value: '101' },
    episodesList: { value: [{ emby_id: '101' }] }, showMediaInfoEditor: { value: true },
    axios: { post: async () => ({ data: result }) },
    message: {
      loading: () => ({ destroy() {} }),
      ...Object.fromEntries(['success', 'warning', 'error'].map(type => [type, text => messages.push({ type, text })])),
    },
  });
  vm.runInContext(`${page.slice(detailStart, detailEnd)}\n${page.slice(start, end)}\nthis.save = saveMediaInfo;`, context);
  await context.save();
  return { context, messages };
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
