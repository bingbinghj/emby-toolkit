import test from 'node:test';
import assert from 'node:assert/strict';
import { shiftLyrics, parseLyrics, formatLyrics, timelineRows, serializeTimeline } from '../src/components/music/lyrics.js';
import { musicSearchTerm, musicReleaseError, metadataSources, lyricSources } from '../src/components/music/utils.js';

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

test('格式化与时间轴保留元数据、重复段、逐字时间并允许给纯文本打点',()=>{
  const text='[ar:歌手]\n[offset:100]\n[0:1.5][1:1.5]<00:01.500>歌词';
  assert.equal(formatLyrics(text),'[ar:歌手]\n[00:01.400][01:01.400]<00:01.400>歌词');
  const rows=timelineRows(text);
  rows[1].time='00:03.400';
  assert.equal(serializeTimeline(rows),'[ar:歌手]\n[00:03.400]<00:03.400>歌词\n[01:01.400]<01:01.400>歌词');
  const plain=timelineRows('第一句\n第二句');plain[1].time='00:02';
  assert.equal(serializeTimeline(plain),'第一句\n[00:02.000]第二句');
  assert.throws(()=>formatLyrics('[00:69]错误'),/秒数/);
  plain[1].time='abc';assert.throws(()=>serializeTimeline(plain),/格式/);
});
test('发行版本归属于专辑组且官方与 GD 资料源始终独立',()=>{
  const release='11111111-1111-1111-1111-111111111111';
  assert.equal(musicReleaseError({media_source:'musicbrainz',music_type:'album',media_id:release,musicbrainz_release_id:release}),'');
  assert.match(musicReleaseError({media_source:'musicbrainz',music_type:'album',musicbrainz_release_id:release}),/所属专辑/);
  assert.match(musicReleaseError({media_source:'netease',music_type:'album',musicbrainz_release_id:release}),/MusicBrainz/);
  for(const sources of [metadataSources,lyricSources]){
    assert.equal(sources.filter(row=>row.value==='netease').length,1);
    assert.equal(sources.filter(row=>row.value==='gd_netease').length,1);
    assert.equal(new Set(sources.map(row=>row.value)).size,sources.length);
  }
});
