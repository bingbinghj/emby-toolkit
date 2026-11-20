import test from 'node:test';
import assert from 'node:assert/strict';
import { shiftLyrics, parseLyrics } from '../src/components/music/lyrics.js';
import { musicSearchTerm } from '../src/components/music/utils.js';

test('歌曲、歌手、专辑搜索默认使用当前歌曲的对应信息',()=>{
  const item={title:'Oh!',artists:['少女时代'],album:'The Second Album'};
  assert.equal(musicSearchTerm(item),'Oh! 少女时代');
  assert.equal(musicSearchTerm(item,'recording','musicbrainz'),'少女时代 - Oh!');
  assert.equal(musicSearchTerm(item,'artist'),'少女时代');
  assert.equal(musicSearchTerm(item,'album'),item.album);
  assert.equal(musicSearchTerm({title:'曲名'}),'曲名');
});

test('歌词平移同时处理重复段和逐字时间，不改变歌词正文',()=>{
  assert.equal(shiftLyrics('[00:10.50][01:10.500]<00:10.500>Oh! <00:11.50>歌词',2.25),'[00:12.750][01:12.750]<00:12.750>Oh! <00:13.750>歌词');
});
test('试听歌词保留重复时间行、逐字正文并应用 offset',()=>{
  assert.deepEqual(parseLyrics('[offset:500]\n[00:10.50][01:10.500]<00:10.500>Oh! <00:11.50>歌词'),[{time:10,text:'Oh! 歌词'},{time:70,text:'Oh! 歌词'}]);
  assert.deepEqual(parseLyrics('纯文本'),[]);
});
test('歌词偏移吸收原 offset，拒绝负时间和纯文本',()=>{
  assert.equal(shiftLyrics('[offset:200]\n[00:01.00]歌词',0.1),'[00:00.900]歌词');
  assert.throws(()=>shiftLyrics('[00:01.00]歌词',-2),/负时间/);
  assert.throws(()=>shiftLyrics('歌词',2),/没有 LRC/);
});
