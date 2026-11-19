import test from 'node:test';
import assert from 'node:assert/strict';
import { shiftLyrics } from '../src/components/music/lyrics.js';

test('歌词平移同时处理重复段和逐字时间，不改变歌词正文',()=>{
  assert.equal(shiftLyrics('[00:10.50][01:10.500]<00:10.500>Oh! <00:11.50>歌词',2.25),'[00:12.750][01:12.750]<00:12.750>Oh! <00:13.750>歌词');
});
test('歌词偏移吸收原 offset，拒绝负时间和纯文本',()=>{
  assert.equal(shiftLyrics('[offset:200]\n[00:01.00]歌词',0.1),'[00:00.900]歌词');
  assert.throws(()=>shiftLyrics('[00:01.00]歌词',-2),/负时间/);
  assert.throws(()=>shiftLyrics('歌词',2),/没有 LRC/);
});
