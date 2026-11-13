import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
const selection = page.slice(page.indexOf('const isEpisodeSelected = '), page.indexOf('// ★★★ 拆分音轨和字幕'));
const plain = value => JSON.parse(JSON.stringify(value));

function fixture(episodes, config) {
  const messages = [];
  const context = vm.createContext({
    configModel: { value: config }, loadingConfig: { value: false }, visibleSeasonEpisodes: { value: episodes },
    selectedEpisodeIds: { value: [] }, applyToSelectedEpisodes: { value: false },
    selectedSeasonNumber: { value: 1 }, selectedBatchVersionIndex: { value: null },
    message: { info: text => messages.push(text) },
  });
  vm.runInContext(`${selection}\nthis.selectAll = selectAllEpisodes; this.selectVersion = selectEpisodesByVersion; this.toggle = toggleEpisodeSelection; this.clear = clearSelectedEpisodes;`, context);
  return { context, messages, selected: () => plain(context.selectedEpisodeIds.value) };
}

const episodes = [
  { emby_id: '101', version_index: 1, track_status: 'normal' },
  { emby_id: '102', version_index: 1, track_status: 'inconsistent', track_difference: '缺少 1 条音轨' },
  { emby_id: '103', version_index: 1, track_status: 'inconsistent', track_difference: '额外 1 条音轨' },
  { emby_id: '201', version_index: 2, track_status: 'normal' },
  { emby_id: '202', version_index: 2, track_status: 'inconsistent' },
  { emby_id: '301', track_status: 'unavailable' },
  { emby_id: '302', track_status: 'info_failed' },
  { emby_id: ' 303 ' },
  { emby_id: '' },
];

for (const config of [null, {}, { media_edit_skip_track_differences_on_select_all: false }, { media_edit_skip_track_differences_on_select_all: 'true' }]) {
  test(`select all keeps current behavior when the option is absent or disabled: ${JSON.stringify(config)}`, () => {
    const f = fixture(episodes, config);
    f.context.selectAll();
    assert.deepEqual(f.selected(), ['101', '102', '103', '201', '202', '301', '302', '303']);
    assert.equal(f.context.applyToSelectedEpisodes.value, true);
    assert.deepEqual(f.messages, []);
  });
}

test('enabled select all skips missing and extra tracks, while unknown and unavailable information remains selectable', () => {
  const f = fixture(episodes, { media_edit_skip_track_differences_on_select_all: true });
  f.context.selectAll();
  assert.deepEqual(f.selected(), ['101', '201', '301', '302', '303']);
  assert.equal(f.context.applyToSelectedEpisodes.value, true);
  assert.equal(f.messages.length, 1);
  assert.match(f.messages[0], /3 个/);
  f.context.toggle('102', true);
  assert.deepEqual(f.selected(), ['101', '201', '301', '302', '303', '102']);
  f.context.clear();
  assert.deepEqual(f.selected(), []);
  assert.equal(f.context.applyToSelectedEpisodes.value, false);
  f.context.toggle('103', true);
  assert.deepEqual(f.selected(), ['103']);
  assert.equal(f.context.applyToSelectedEpisodes.value, true);
});

test('version selection applies the same skip rule only to the requested version', () => {
  const f = fixture(episodes, { media_edit_skip_track_differences_on_select_all: true });
  f.context.selectedBatchVersionIndex.value = 2;
  f.context.selectVersion();
  assert.deepEqual(f.selected(), ['201']);
  assert.equal(f.messages.length, 1);
  assert.match(f.messages[0], /1 个/);
  f.context.configModel.value.media_edit_skip_track_differences_on_select_all = false;
  f.context.selectVersion();
  assert.deepEqual(f.selected(), ['201', '202']);
});

test('all skipped targets disable batch apply and count each excluded media ID once', () => {
  const f = fixture([
    { emby_id: '101', track_status: 'inconsistent' },
    { emby_id: '101', track_status: 'inconsistent' },
    { emby_id: '102', track_status: 'inconsistent' },
    { emby_id: '', track_status: 'inconsistent' },
  ], { media_edit_skip_track_differences_on_select_all: true });
  f.context.selectAll();
  assert.deepEqual(f.selected(), []);
  assert.equal(f.context.applyToSelectedEpisodes.value, false);
  assert.equal(f.messages.length, 1);
  assert.match(f.messages[0], /2 个/);
});

test('the disabled option preserves existing repeated IDs and distinct version IDs', () => {
  const f = fixture([{ emby_id: '101' }, { emby_id: '101' }, { emby_id: '102', version_index: 2 }], {});
  f.context.selectAll();
  assert.deepEqual(f.selected(), ['101', '101', '102']);
});

test('bulk selection waits for the saved configuration to load without blocking manual selection', () => {
  const f = fixture(episodes, null);
  f.context.loadingConfig.value = true;
  f.context.selectAll();
  f.context.selectedBatchVersionIndex.value = 2;
  f.context.selectVersion();
  assert.deepEqual(f.selected(), []);
  f.context.toggle('102', true);
  assert.deepEqual(f.selected(), ['102']);
  f.context.configModel.value = { media_edit_skip_track_differences_on_select_all: true };
  f.context.loadingConfig.value = false;
  f.context.selectAll();
  assert.deepEqual(f.selected(), ['101', '201', '301', '302', '303']);
});

test('the setting defaults to off and is included in the basic settings save payload', async () => {
  const defaults = JSON.parse(readFileSync(new URL('../../backend/assets/config_defaults.json', import.meta.url), 'utf8'));
  assert.equal(defaults.media_edit_skip_track_differences_on_select_all, false);
  const settings = readFileSync(new URL('../src/components/settings/GeneralSettingsPage.vue', import.meta.url), 'utf8');
  const start = settings.indexOf('const cardFields = '), end = settings.indexOf('const previewRenameFormats = ', start);
  const writes = [];
  const context = vm.createContext({
    storageConfigFields: [], componentIsMounted: { value: true }, savingConfig: { value: false },
    configModel: { value: { media_edit_skip_track_differences_on_select_all: true, ai_model_name: 'another-card-draft' } },
    savingCard: { value: null }, configError: { value: null }, message: { success() {}, error() {} },
    handleSaveConfig: async payload => { writes.push(plain(payload)); return true; },
  });
  vm.runInContext(`${settings.slice(start, end)}\nthis.save = saveCard;`, context);
  await context.save('basic');
  assert.deepEqual(writes, [{ media_edit_skip_track_differences_on_select_all: true }]);
});
