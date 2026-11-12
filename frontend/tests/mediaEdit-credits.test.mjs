import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
const definition = name => {
  const start = page.indexOf(`const ${name} = `);
  assert.ok(start >= 0);
  return page.slice(start, page.indexOf('\n};', start) + 4);
};
const credits = { Name: '片尾', MarkerType: 'CreditsStart', StartPositionTicks: 9000000000 };
const intro = { Name: '片头', MarkerType: 'IntroStart', StartPositionTicks: 30000000 };
function fixture(post = async () => ({ status: 200, data: { message: '片尾提取已完成', chapter: credits } })) {
  const calls = [], messages = [];
  const state = vm.createContext({
    itemDetails: { value: { item_type: 'Episode' } }, currentEditMediaId: { value: '101' },
    creditsExtractionBlocked: { value: false }, isExtractingCredits: { value: false }, creditsExtractionFeedback: { value: null },
    mediaDetailsLoadSequence: 1, mediaInfoLoadSequence: 1,
    mediaInfoData: { value: [{ Chapters: [intro, { ...credits, StartPositionTicks: 7000000000 }], MediaSourceInfo: { MediaStreams: [{ Type: 'Audio', Title: '保留草稿' }] } }] },
    axios: { post: async (url, payload) => { calls.push({ url, payload }); return post(); } },
    message: { loading: () => ({ destroy() {} }),
      ...Object.fromEntries(['success', 'warning', 'error'].map(type => [type, text => messages.push({ type, text })])) },
  });
  vm.runInContext(['mediaOperationDetails', 'mergeCreditsChapter', 'extractEpisodeCredits'].map(definition).join('\n')
    + '\nthis.run = extractEpisodeCredits; this.merge = mergeCreditsChapter;', state);
  return { state, calls, messages };
}
const json = value => JSON.parse(JSON.stringify(value));

test('credits extraction replaces only CreditsStart and preserves audio drafts and intro markers', async () => {
  const f = fixture();
  await f.state.run('101');
  assert.equal(f.calls[0].url, '/api/media/101/extract_credits');
  assert.deepEqual(json(f.state.mediaInfoData.value[0].Chapters), [intro, credits]);
  assert.equal(f.state.mediaInfoData.value[0].MediaSourceInfo.MediaStreams[0].Title, '保留草稿');
  assert.equal(f.state.creditsExtractionFeedback.value.type, 'success');
  assert.equal(f.state.isExtractingCredits.value, false);
});

test('object media info preserves its streams when credits are merged', () => {
  const f = fixture();
  const info = { Chapters: [intro], MediaStreams: [{ Type: 'Subtitle', Title: '字幕草稿' }] };
  f.state.merge(info, credits);
  assert.deepEqual(json(info.Chapters), [intro, credits]);
  assert.equal(info.MediaStreams[0].Title, '字幕草稿');
  f.state.merge(info, { MarkerType: 'Chapter' });
  assert.deepEqual(json(info.Chapters), [intro, credits]);
});

for (const reason of ['movie', 'pending-recovery', 'duplicate']) {
  test(`${reason} prevents submitting credits extraction`, async () => {
    const f = fixture();
    if (reason === 'movie') f.state.itemDetails.value.item_type = 'Movie';
    if (reason === 'pending-recovery') f.state.creditsExtractionBlocked.value = true;
    if (reason === 'duplicate') f.state.isExtractingCredits.value = true;
    await f.state.run('101');
    assert.equal(f.calls.length, 0);
  });
}

test('partial credits failure keeps the actual per-item reason', async () => {
  const f = fixture(async () => ({ status: 207, data: { failed: 1, message: '片尾提取有提醒', failures: [{ item_id: '101', reason: '未检测到有效黑场' }] } }));
  await f.state.run('101');
  assert.equal(f.state.creditsExtractionFeedback.value.type, 'warning');
  assert.deepEqual(json(f.state.creditsExtractionFeedback.value.details), ['101：未检测到有效黑场']);
  assert.equal(f.state.mediaInfoData.value[0].Chapters[1].StartPositionTicks, 7000000000);
});

test('credits failure preserves the draft and reports the backend error', async () => {
  const f = fixture(async () => { throw { response: { data: { error: '115下载失败', failures: [{ item_id: '101', reason: '直链不可用' }] } } }; });
  const before = json(f.state.mediaInfoData.value);
  await f.state.run('101');
  assert.equal(f.state.creditsExtractionFeedback.value.type, 'error');
  assert.equal(f.messages.at(-1).text, '115下载失败');
  assert.deepEqual(json(f.state.creditsExtractionFeedback.value.details), ['101：直链不可用']);
  assert.deepEqual(json(f.state.mediaInfoData.value), before);
});

for (const changed of ['page', 'editor']) {
  test(`a late credits result cannot mutate the newer ${changed}`, async () => {
    let finish;
    const pending = new Promise(resolve => { finish = resolve; });
    const f = fixture(() => pending);
    const running = f.state.run('101');
    if (changed === 'page') f.state.mediaDetailsLoadSequence++;
    else f.state.mediaInfoLoadSequence++;
    const before = json(f.state.mediaInfoData.value);
    finish({ status: 200, data: { message: '旧媒体片尾已提取', chapter: credits } });
    await running;
    assert.deepEqual(json(f.state.mediaInfoData.value), before);
    if (changed === 'page') {
      assert.equal(f.state.creditsExtractionFeedback.value, null);
      assert.equal(f.messages.length, 0);
    }
  });
}


test('insufficient comparable episodes stays a warning with its actual skipped error', async () => {
  const f = fixture(async () => ({ status: 200, data: { failed: 0, skipped: 1, message: '更新0集，跳过1集', items: [{ item_id: '101', status: 'skipped', error: '同季可比对分集不足2集，未猜测片尾' }] } }));
  await f.state.run('101');
  assert.equal(f.state.creditsExtractionFeedback.value.type, 'warning');
  assert.deepEqual(json(f.state.creditsExtractionFeedback.value.details), ['101：同季可比对分集不足2集，未猜测片尾']);
});

test('a verified existing credits marker is a successful skip and updates only the marker', async () => {
  const f = fixture(async () => ({ status: 200, data: { failed: 0, skipped: 1, message: '已有有效片尾标记', items: [{ item_id: '101', status: 'skipped', reason: '已有有效片尾标记', chapter: credits }] } }));
  await f.state.run('101');
  assert.equal(f.state.creditsExtractionFeedback.value.type, 'success');
  assert.deepEqual(json(f.state.mediaInfoData.value[0].Chapters), [intro, credits]);
});
