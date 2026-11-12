import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import assert from 'node:assert/strict';
import yaml from 'js-yaml';

const source = readFileSync(new URL('../src/components/CoverGeneratorConfig.vue', import.meta.url), 'utf8');
const plain = value => JSON.parse(JSON.stringify(value));
const initial = { enabled: false, exclude_libraries: [], cover_style: 'single_1', title_config: '电影: [原中文, 原英文]', zh_font_size: 1, future_option: { keep: true } };
const sectionCode = source.match(/^const sectionFields = \{[\s\S]*?^};/m)[0];
const saveCode = source.match(/^const saveConfig = async \(section\) => \{[\s\S]*?^};/m)[0];

function fixture(server = { config: plain(initial) }, request) {
  const calls = [], messages = [];
  const context = vm.createContext({
    configData: { value: plain(initial) }, persistedConfig: plain(initial),
    isSaving: { value: false }, isLoading: { value: false }, isConfigReady: { value: true },
    titleConfigUnreadable: { value: false },
    convertDataToYaml: () => '',
    message: Object.fromEntries(['success', 'error', 'warning'].map(type => [type, text => messages.push({ type, text })])),
    axios: Object.fromEntries(['post', 'patch'].map(method => [method, async (path, payload) => {
      calls.push({ method, path, payload: plain(payload), reference: payload });
      if (request) return request(payload);
      server.config = method === 'post' ? plain(payload) : { ...server.config, ...plain(payload) };
      return { data: { message: '已保存' } };
    }])),
  });
  vm.runInContext(`${sectionCode}\n${saveCode}\nthis.save = saveConfig;`, context);
  return { context, calls, messages, server };
}

function loadedFixture(titleConfig) {
  const f = fixture({ config: { ...plain(initial), title_config: titleConfig } });
  Object.assign(f.context, {
    yaml, titleConfigs: { value: [] }, previewGeneration: 0, previewUpdateTimeout: null,
    isPreviewLoading: { value: false }, configLoadError: { value: '' },
    clearTimeout: () => {}, console: { error: () => {} },
  });
  f.context.axios.get = async path => {
    assert.equal(path, '/api/config/cover_generator');
    return { data: plain(f.server.config) };
  };
  const definitions = ['parseYamlToData', 'convertDataToYaml', 'addTitleConfig', 'clearInvalidTitleConfig', 'fetchConfig']
    .map(name => source.match(new RegExp(`^const ${name} = (?:async )?[^=]*=> \\{[\\s\\S]*?^};`, 'm'))?.[0] || '');
  vm.runInContext(`${definitions.join('\n')}\nthis.load = fetchConfig; this.add = addTitleConfig; this.clear = typeof clearInvalidTitleConfig === 'function' ? clearInvalidTitleConfig : null;`, f.context);
  return f;
}

test('stale cover windows saving different cards cannot restore the other window’s saved values', async () => {
  const server = { config: plain(initial) };
  const first = fixture(server), second = fixture(server);
  first.context.configData.value.enabled = true;
  await first.context.save('basic');
  second.context.configData.value.cover_style = 'multi_1';
  await second.context.save('style-tab');
  assert.equal(server.config.enabled, true);
  assert.equal(server.config.cover_style, 'multi_1');
  assert.deepEqual(server.config.future_option, { keep: true });
  assert.deepEqual(second.calls.map(({ method, payload }) => ({ method, payload })), [
    { method: 'patch', payload: { cover_style: 'multi_1' } },
  ]);
});

test('a cover card only submits its fields and leaves other card drafts unsaved', async () => {
  const f = fixture();
  f.context.configData.value.enabled = true;
  f.context.configData.value.cover_style = 'multi_1';
  f.context.configData.value.zh_font_size = 2;
  await f.context.save('basic');
  assert.deepEqual(f.calls[0].payload, { enabled: true, exclude_libraries: [] });
  assert.equal(f.server.config.cover_style, 'single_1');
  assert.equal(f.server.config.zh_font_size, 1);
  assert.equal(f.context.configData.value.cover_style, 'multi_1');
  assert.equal(f.context.configData.value.zh_font_size, 2);
});

test('a submitted cover array stays immutable and newer drafts survive the reply', async () => {
  let release;
  const f = fixture(undefined, () => new Promise(resolve => { release = resolve; }));
  const saving = f.context.save('basic');
  f.context.configData.value.exclude_libraries.push('new-draft');
  f.context.configData.value.enabled = true;
  assert.deepEqual(plain(f.calls[0].reference), { enabled: false, exclude_libraries: [] });
  await f.context.save('style-tab');
  assert.equal(f.calls.length, 1);
  release({ data: { message: '已保存' } });
  await saving;
  assert.equal(f.context.configData.value.enabled, true);
  assert.deepEqual(plain(f.context.configData.value.exclude_libraries), ['new-draft']);
  assert.equal(f.context.isSaving.value, false);
});

test('failed cover writes retain the draft and retry only on another explicit save', async () => {
  let succeeds = false;
  const f = fixture(undefined, async () => {
    if (!succeeds) throw { response: { data: { error: '封面配置写入失败' } } };
    return { data: { message: '已保存' } };
  });
  f.context.configData.value.cover_style = 'multi_1';
  await f.context.save('style-tab');
  assert.equal(f.context.configData.value.cover_style, 'multi_1');
  assert.equal(f.context.isSaving.value, false);
  assert.equal(f.calls.length, 1);
  assert.equal(f.messages.at(-1).text, '封面配置写入失败');
  succeeds = true;
  await f.context.save('style-tab');
  assert.equal(f.calls.length, 2);
  assert.equal(f.calls[1].method, 'patch');
  assert.deepEqual(f.calls[1].payload, { cover_style: 'multi_1' });
});

test('clearing title rows sends the empty title configuration without clearing other sections', async () => {
  const f = fixture();
  await f.context.save('title-tab');
  assert.deepEqual(f.calls[0].payload, { title_config: '' });
  assert.equal(f.server.config.title_config, '');
  assert.equal(f.server.config.cover_style, 'single_1');
  assert.deepEqual(f.server.config.future_option, { keep: true });
});

test('unread, busy or unknown cover cards never submit or leave saving stuck', async () => {
  const f = fixture();
  f.context.isConfigReady.value = false;
  await f.context.save('basic');
  f.context.isConfigReady.value = true;
  f.context.isSaving.value = true;
  await f.context.save('basic');
  f.context.isSaving.value = false;
  for (const section of ['unknown-tab', 'constructor', '__proto__']) await f.context.save(section);
  assert.equal(f.calls.length, 0);
  assert.equal(f.context.isSaving.value, false);
});

test('reserved but valid library names survive the real title YAML round trip', () => {
  const convertCode = source.match(/^const convertDataToYaml = \(\) => \{[\s\S]*?^};/m)[0];
  const context = vm.createContext({ yaml, titleConfigs: { value: [] }, configData: { value: { title_config: 'keep' } }, message: { error: () => assert.fail('unexpected YAML error') } });
  vm.runInContext(`${convertCode}\nthis.convert = convertDataToYaml;`, context);
  for (const library of ['__proto__', 'constructor', '普通库']) {
    context.titleConfigs.value = [{ library, zh: '中文', en: 'English' }];
    const parsed = yaml.load(context.convert());
    assert.ok(Object.hasOwn(parsed || {}, library), `lost title for ${library}`);
    assert.deepEqual(parsed[library], ['中文', 'English']);
  }
  context.titleConfigs.value = [];
  assert.equal(context.convert(), '');
});

test('malformed saved title YAML is preserved by fetch and an untouched title save', async () => {
  const original = '电影: [';
  const f = loadedFixture(original);
  await f.context.load();
  assert.equal(f.context.isConfigReady.value, true);
  assert.deepEqual(plain(f.context.titleConfigs.value), []);
  assert.equal(f.context.configData.value.title_config, original);
  await f.context.save('title-tab');
  assert.equal(f.calls.length, 0);
  assert.equal(f.server.config.title_config, original);
  assert.equal(f.context.configData.value.title_config, original);
  assert.equal(f.context.isSaving.value, false);
});

test('a malformed title does not block other cards and an empty added row cannot silently clear it', async () => {
  const original = '电影: [';
  const f = loadedFixture(original);
  await f.context.load();
  f.context.configData.value.enabled = true;
  await f.context.save('basic');
  assert.equal(f.calls.length, 1);
  assert.equal(f.server.config.enabled, true);
  assert.equal(f.server.config.title_config, original);
  f.context.add();
  await f.context.save('title-tab');
  assert.equal(f.calls.length, 1);
  assert.equal(f.context.configData.value.title_config, original);
});

test('a malformed title can be replaced by explicitly entered valid rows', async () => {
  const f = loadedFixture('电影: [');
  await f.context.load();
  f.context.add();
  Object.assign(f.context.titleConfigs.value[0], { library: '电影', zh: '已纠正', en: 'Corrected' });
  await f.context.save('title-tab');
  assert.equal(f.calls.length, 1);
  assert.deepEqual(yaml.load(f.calls[0].payload.title_config), { 电影: ['已纠正', 'Corrected'] });
  assert.deepEqual(Object.keys(f.calls[0].payload), ['title_config']);
  assert.equal(f.context.titleConfigUnreadable.value, false);
});

test('explicitly clearing an unreadable title only changes its draft until saved', async () => {
  const original = '电影: [';
  const f = loadedFixture(original);
  await f.context.load();
  assert.equal(typeof f.context.clear, 'function');
  f.context.clear();
  assert.equal(f.calls.length, 0);
  assert.equal(f.server.config.title_config, original);
  await f.context.save('title-tab');
  assert.deepEqual(f.calls.map(call => call.payload), [{ title_config: '' }]);
  assert.equal(f.server.config.title_config, '');
  assert.equal(f.server.config.cover_style, initial.cover_style);
});
