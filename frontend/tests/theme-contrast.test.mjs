import test from 'node:test';
import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import { textColorForBackground } from '../src/utils/colorContrast.js';

const luminance = color => color.slice(1).match(/../g).map(value => parseInt(value, 16) / 255)
  .reduce((sum, value, index) => sum + (value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4)
    * [0.2126, 0.7152, 0.0722][index], 0);

test('custom gray and preset accents retain readable solid-button text', () => {
  for (const background of ['#999999', '#777777', '#FFFFFF', '#000000', '#8D51F9', '#FFB400', '#1976D2']) {
    const text = textColorForBackground(background);
    const ink = luminance(text), fill = luminance(background);
    const contrast = (Math.max(ink, fill) + 0.05) / (Math.min(ink, fill) + 0.05);
    assert.ok(contrast >= 4.5, `${background} / ${text}: ${contrast}`);
  }
  assert.equal(textColorForBackground('#999999'), '#000000');
});

test('invalid accent inputs keep the safe fallback', () => {
  for (const value of [null, undefined, '', '#123', 'red', 'not-a-color']) {
    assert.equal(textColorForBackground(value), '#FFFFFF');
  }
});

test('MP 主题信息和错误按钮的前景达到正文对比度', () => {
  const source=readFileSync(new URL('../src/utils/themePalette.ts',import.meta.url),'utf8');
  for(const key of ['info','error']){
    const pairs=[...source.matchAll(new RegExp("'"+key+"': '(#[A-Fa-f0-9]{6})',\\s*'on-"+key+"': '(#[A-Fa-f0-9]{6})'",'g'))];
    assert.equal(pairs.length,2);
    for(const [,fill,ink] of pairs){const a=luminance(fill),b=luminance(ink);assert.ok((Math.max(a,b)+.05)/(Math.min(a,b)+.05)>=4.5,`${fill} / ${ink}`);}
  }
});
