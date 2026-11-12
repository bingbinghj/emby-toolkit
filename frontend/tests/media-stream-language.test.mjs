import assert from 'node:assert/strict';
import test from 'node:test';
import { buildMediaInfoLanguageOptions, resolveMediaInfoLanguageSelection, updateMediaInfoStreamLanguage } from '../src/utils/mediaStreams.js';

const options = buildMediaInfoLanguageOptions([
  { label: '国语', value: 'zh', aliases: ['chi', 'zho', 'zh', 'cmn'] },
  { label: '简体', value: 'chs' },
  { label: '繁体', value: 'cht' },
], true);

test('initial subtitle selection reflects the stored language and preserves its title', () => {
  for (const [language, expected] of [['chi', 'zho'], ['chs', 'chs'], ['cht', 'cht'], ['yue', 'yue']]) {
    for (const title of ['粤语', '简体', '繁体', '导评']) {
      const stream = { Type: 'Subtitle', Language: language, Title: title };
      const original = { ...stream };
      assert.equal(resolveMediaInfoLanguageSelection(options, stream), expected);
      assert.deepEqual(stream, original);
    }
  }
});

test('explicit language correction changes only the language, keeping the original title', () => {
  const stream = { Type: 'Subtitle', Language: 'chi', Title: '粤语' };
  updateMediaInfoStreamLanguage(stream, 'chs');
  assert.equal(resolveMediaInfoLanguageSelection(options, stream), 'chs');
  assert.equal(stream.Title, '粤语');
});
