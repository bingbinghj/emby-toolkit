import test from 'node:test';
import assert from 'node:assert/strict';
import { shiftLyrics, parseLyrics, formatLyrics, timelineRows, serializeTimeline, mergeLyricsTranslation, lyricTranslationRows } from '../src/components/music/lyrics.js';
import { musicSearchTerm, musicReleaseError, musicReleaseOptions, musicFieldOptions, metadataSources, lyricSources } from '../src/components/music/utils.js';

test('歌曲、歌手、专辑搜索默认使用当前歌曲的对应信息',()=>{
  const item={title:'Oh!',artists:['少女时代'],album:'The Second Album'};
  assert.equal(musicSearchTerm(item),'Oh!');
  assert.equal(musicSearchTerm(item,'recording','musicbrainz'),'Oh!');
  assert.equal(musicSearchTerm(item,'artist'),'少女时代');
  assert.equal(musicSearchTerm(item,'album'),item.album);
  assert.equal(musicSearchTerm({title:'曲名'}),'曲名');
  assert.equal(musicSearchTerm({album:'GIRLS’ GENERATION II ～Girls & Peace～',year:2012},'album'),'GIRLS’ GENERATION II ～Girls & Peace～ (2012)');
  assert.equal(musicSearchTerm({album:'专辑 (2012)',year:2012},'album'),'专辑 (2012)');
  assert.equal(musicSearchTerm({album:'专辑',release_date:'2012-11-28'},'album'),'专辑 (2012)');
});

test('来源译文按原文匹配，保留本地时间轴、元数据与未匹配句子',()=>{
  const original='[ar:原歌手]\n[offset:200]\n[00:10.00][00:50.00]Hello!\n[00:20.00]Unknown line';
  const result=mergeLyricsTranslation(original,'[00:01.00]Hello\n[00:02.00]Other','[00:01.00]你好\n[00:02.00]另一句');
  assert.equal(result.text,'[ar:原歌手]\n[offset:200]\n[00:10.00][00:50.00]Hello!\n[00:10.00][00:50.00]你好\n[00:20.00]Unknown line');
  assert.equal(result.matched,1);assert.equal(result.total,2);
  assert.equal(mergeLyricsTranslation(result.text,'[00:01]Hello','[00:01]你好').text,result.text);
  assert.equal(mergeLyricsTranslation('[00:01]Different','[00:01]Hello','[00:01]你好').matched,0);
  assert.equal(mergeLyricsTranslation('Hello\nWorld','Hello\nWorld','你好\n世界').text,'Hello\n你好\nWorld\n世界');
  assert.equal(mergeLyricsTranslation('Hello\n你好\nWorld\n世界','Hello\nWorld','你好\n世界').text,'Hello\n你好\nWorld\n世界');
});

test('来源演唱者标注和跨行句段不会阻止译文合并，重复应用不重复插入',()=>{
  const original='[ar:原歌手]\n[00:33.25]어느 샌가 나는 숙녀처럼\n[00:36.63]내 입술은 사근사근 그대 이름 부르죠';
  const source='[00:33.350]（태연） 어느샌가 나는 숙녀처럼 내 입술은 사근사근 그대 이름 부르죠';
  const translation='[00:33.350]（泰妍）不知不觉我像淑女一样 我的嘴唇温柔的叫着你的名字';
  const merged=mergeLyricsTranslation(original,source,translation);
  assert.equal(merged.matched,2);assert.equal(merged.total,2);
  assert.equal(merged.text,'[ar:原歌手]\n[00:33.25]어느 샌가 나는 숙녀처럼\n[00:33.25]（泰妍）不知不觉我像淑女一样\n[00:36.63]내 입술은 사근사근 그대 이름 부르죠\n[00:36.63]我的嘴唇温柔的叫着你的名字');
  assert.equal(mergeLyricsTranslation(merged.text,source,translation).text,merged.text);
});

test('网易云整句译文按 LRCLIB 原文分行，保留原时间且可升级已合并的整句',()=>{
  const original='[offset:200]\n[00:17.66][01:46.27] 날 아직 어리다고\n[00:22.71][01:50.25] 말하던 얄미운 욕심쟁이가';
  const source='[00:17.660]날 아직 어리다고 말하던 얄미운 욕심쟁이가~';
  const translation='[00:17.660]还幼小的我 说成是可恶的贪心鬼';
  const expected='[offset:200]\n[00:17.66][01:46.27] 날 아직 어리다고\n[00:17.66][01:46.27]还幼小的我\n[00:22.71][01:50.25] 말하던 얄미운 욕심쟁이가\n[00:22.71][01:50.25]说成是可恶的贪心鬼';
  const merged=mergeLyricsTranslation(original,source,translation);
  assert.equal(merged.text,expected);
  assert.equal(mergeLyricsTranslation(merged.text,source,translation).text,merged.text);
  const old=original.replace('\n[00:22.71]', '\n[00:17.66][01:46.27]还幼小的我 说成是可恶的贪心鬼\n[00:22.71]');
  assert.equal(mergeLyricsTranslation(old,source,translation).text,expected);
  const custom=original.replace('\n[00:22.71]', '\n[00:17.66]我自己写的中文\n[00:22.71]');
  assert.match(mergeLyricsTranslation(custom,source,translation).text,/我自己写的中文/);
});

test('只按清楚的译文分句拆分，不按字数硬拆或拆开括号及英文单词',()=>{
  const original='[00:01]얼굴은 빨개지고\n[00:05]놀란 눈은 커다래지고';
  const source='[00:02]얼굴은 빨개지고 놀란 눈은 커다래지고';
  const split=mergeLyricsTranslation(original,source,'[00:02]脸变红了，惊慌的眼睛变大了（和声 留在这里）');
  assert.equal(split.text,'[00:01]얼굴은 빨개지고\n[00:01]脸变红了，\n[00:05]놀란 눈은 커다래지고\n[00:05]惊慌的眼睛变大了（和声 留在这里）');
  for(const value of ['脸变红了惊慌的眼睛变大了','脸变红了 惊慌的 眼睛变大了','My face is red']){
    assert.equal(mergeLyricsTranslation(original,source,'[00:02]'+value).text,`[00:01]얼굴은 빨개지고\n[00:01]${value}\n[00:05]놀란 눈은 커다래지고`);
  }
});

test('仅未确认分界的句段需要翻译参考，确认后仍使用网易云译文',()=>{
  const original='[00:01]얼굴은 빨개지고\n[00:05]놀란 눈은 커다래지고\n[00:10]어쩜 그대 내 맘을\n[00:15]흔들어 놓는지';
  const source='[00:02]얼굴은 빨개지고 놀란 눈은 커다래지고\n[00:20]어쩜 그대 내 맘을 흔들어 놓는지';
  const translation='[00:02]脸变红了 惊慌的眼睛变大了\n[00:20]不知怎么你开始动摇我的心';
  const first=mergeLyricsTranslation(original,source,translation);
  assert.deepEqual(first.pendingLineIndices,[2,3]);assert.equal(first.splits.length,1);
  const result=mergeLyricsTranslation(original,source,translation,[{index:2,text:'不知怎么你开始'},{index:3,text:'动摇我的心'}]);
  assert.deepEqual(result.pendingLineIndices,[]);assert.equal(result.splits.length,2);
  assert.match(result.text,/\[00:10\]不知怎么你开始\n\[00:15\]흔들어 놓는지\n\[00:15\]动摇我的心/);
  assert.equal(mergeLyricsTranslation(result.text,source,translation).text,result.text);
  const uncertain=mergeLyricsTranslation(original,source,translation,[{index:2,text:'不同的描述'},{index:3,text:'毫不相关的译文'}]);
  assert.equal(uncertain.text,first.text);assert.deepEqual(uncertain.pendingLineIndices,[2,3]);
});

test('翻译参考分界有空缺或原文覆盖不足时保持整句，不用机器译文覆盖来源',()=>{
  const original='[00:10]어쩜 그대 내 맘을\n[00:15]흔들어 놓는지';
  const source='[00:20]어쩜 그대 내 맘을 흔들어 놓는지';
  const translation='[00:20]不知怎么你开始慢慢动摇我的心';
  const result=mergeLyricsTranslation(original,source,translation,[{index:0,text:'不知怎么你开始'},{index:1,text:'动摇我的心'}]);
  assert.deepEqual(result.pendingLineIndices,[0,1]);assert.equal(result.splits.length,0);
  assert.match(result.text,/\[00:10\]不知怎么你开始慢慢动摇我的心/);
});

test('原文局部差异较大的句段计入待调整数量，但不发送无法可靠对照的行',()=>{
  const result=mergeLyricsTranslation('[00:10]화를 낼까 웃어버릴까\n[00:15]생각하다가 Yeah','[00:01]화를낼까 웃어버릴까 생각하다가~','[00:01]即使发火也要笑 一定要这样想(Yeah!)');
  assert.equal(result.pendingSegments.length,1);
  assert.deepEqual(result.pendingLineIndices,[]);
  assert.equal(result.splits.length,0);
});

test('中韩混合歌词默认只勾选外语行，时间标签、汉字与空行不参与自动选择',()=>{
  const rows=lyricTranslationRows('[ar:成龙 / 金喜善]\n[offset:100]\n[00:01]梦中人 熟悉的脸孔\n[00:10]<00:10>나의 <00:11>손을 잡아줘\n\n[00:15]永遠的神話');
  assert.deepEqual(rows.filter(row=>row.foreign).map(row=>row.index),[3]);
  assert.deepEqual(rows.filter(row=>row.selectable).map(row=>row.index),[2,3,5]);
});

test('分段翻译通用于英日韩俄阿拉伯泰等语言，纯汉字歌词可手动选择',()=>{
  const foreign=['Hold my hand','私の手を握って','나의 손을 잡아줘','Возьми меня за руку','أمسك بيدي','จับมือฉัน','Toma mi mano','Prends ma main'];
  const text=['[ar:歌手]','[00:01]请握住我的手',...foreign.map((value,i)=>`[00:${10+i}]${value}`),'[00:20]夢'].join('\n');
  const rows=lyricTranslationRows(text);
  assert.deepEqual(rows.filter(row=>row.foreign).map(row=>row.index),foreign.map((_,i)=>i+2));
  assert(rows.at(-1).selectable);assert(!rows.at(-1).foreign);
  for(const [first,second] of [['Hold my hand','Never let me go'],['私の手を握って','もう離さないで'],['Возьми меня за руку','Никогда не отпускай'],['أمسك بيدي','لا تتركني أبدا']]){
    const merged=mergeLyricsTranslation(`[00:01]${first}\n[00:05]${second}`,`[00:02]${first} ${second}`,'[00:02]握住我的手 不要放开我');
    assert.equal(merged.text,`[00:01]${first}\n[00:01]握住我的手\n[00:05]${second}\n[00:05]不要放开我`);
  }
});

test('重复副歌按出现顺序保留各次译文，不因译文不同而整句丢弃',()=>{
  const original='[00:10]어리다고 놀리지 말아요\n[00:20]어리다고 놀리지 말아요\n[00:30]어리다고 놀리지 말아요';
  const source='[00:01]어리다고 놀리지말아요\n[00:02]어리다고 놀리지말아요\n[00:03]어리다고 놀리지말아요';
  const translation='[00:01]不要捉弄幼小的我\n[00:02]不要捉弄幼小的我(不要捉弄)\n[00:03]不要捉弄幼小的我(我不知道)';
  const merged=mergeLyricsTranslation(original,source,translation);
  assert.equal(merged.matched,3);
  assert.equal(merged.text,'[00:10]어리다고 놀리지 말아요\n[00:10]不要捉弄幼小的我\n[00:20]어리다고 놀리지 말아요\n[00:20]不要捉弄幼小的我(不要捉弄)\n[00:30]어리다고 놀리지 말아요\n[00:30]不要捉弄幼小的我(我不知道)');
  const chorus=mergeLyricsTranslation(original.replace('[00:20]','[00:15]놀리지 말아요\n[00:20]'),source,translation);
  assert.match(chorus.text,/\[00:10\]不要捉弄幼小的我\n\[00:15\]놀리지 말아요/);
  assert.match(chorus.text,/\[00:20\]不要捉弄幼小的我\(不要捉弄\)/);
});

test('分行位置交错及轻微拼写差异按句段对齐，原文逐字时间保持不变',()=>{
  const original='[00:10]<00:10.00>오늘은 웬일인지\n[00:15]사랑해 하며 키스해 주었네\n[00:20]너만 들리게 말해줄 게';
  const source='[00:01]오늘은 왠일인지 사랑해하며\n[00:02]키스해주었네 너만 들리게 말해줄게';
  const translation='[00:01]今天是怎么回事 说着爱你\n[00:02]亲吻你 只告诉你';
  const result=mergeLyricsTranslation(original,source,translation);
  assert.equal(result.matched,3);
  assert.equal(result.text,'[00:10]<00:10.00>오늘은 웬일인지\n[00:10]今天是怎么回事 说着爱你\n[00:15]사랑해 하며 키스해 주었네\n[00:15]亲吻你 只告诉你\n[00:20]너만 들리게 말해줄 게');
  assert.equal(mergeLyricsTranslation('[00:10]Nothing matches here','[00:01]Completely different words','[00:01]另一版歌词').matched,0);
});

test('发行选项包含版本信息，标签候选只采用接口已返回的字段',()=>{
  const album={title:'专辑',musicbrainz_release_id:'edition',releases:[{media_id:'edition',title:'日版',country:'JP',formats:['CD'],track_count:12}],total_tracks:12,tracks:[{title:'歌曲一',track_number:1},{title:'歌曲二',track_number:2}]};
  assert.deepEqual(musicReleaseOptions(album),[{value:'edition',title:'日版 · JP · CD · 12 首曲目'}]);
  const metadata={artists:['歌手'],isrc:'USABC2400001',raw_data:{isrcs:['USABC2400001','USABC2400002']},performers:{guitar:['演奏者']}};
  assert.deepEqual(musicFieldOptions('isrc',metadata,album),['USABC2400001','USABC2400002']);
  assert.deepEqual(musicFieldOptions('title',metadata,album),[]);
  assert.deepEqual(musicFieldOptions('title',{title:'当前曲名'},album),['当前曲名']);
  assert.deepEqual(musicFieldOptions('track_number',{track_number:3},album),[3]);
  assert.deepEqual(musicFieldOptions('total_tracks',metadata,album),[12]);
  assert.deepEqual(musicFieldOptions('performer_role',metadata),['guitar']);
  assert.deepEqual(musicFieldOptions('original_year',metadata,album),[]);
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
