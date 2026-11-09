import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
const start = page.indexOf('const saveMediaInfo = ');
const end = page.indexOf('\n};', start) + 4;

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
  vm.runInContext(`${page.slice(start, end)}\nthis.save = saveMediaInfo;`, context);
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
