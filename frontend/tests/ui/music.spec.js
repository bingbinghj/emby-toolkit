import { test, expect } from './fixtures.js';

const track = {name:'01 - 测试歌曲.flac',type:'file',is_audio:true,path:'专辑/01 - 测试歌曲.flac',title:'测试歌曲',artists:['测试歌手'],album:'测试专辑',audio_specs:'FLAC / 24bit / 96kHz',signature:{mtime:1}};
const previewItem = {...track,source:track.path,source_storage:'local',source_item:{storage:'local',path:track.path,type:'file'},target:track.path,success:true,metadata:{title:'新曲名'},before:{title:track.title},music:{title:'新曲名',album:'测试专辑',artists:track.artists,group_id:'group',group_directory:'专辑',file_role:'audio',status:'matched',online_confirmed:true,read_status:'tags',field_sources:{title:'remote'},media_source:'musicbrainz',media_id:'song'}};

function music(api) {
  api.respond('GET','/api/music/image',{message:'没有封面'},404);
  api.respond('GET','/api/music/library',{items:[track],total:1});
  api.respond('POST','/api/music/query/inspect',{...track,lyrics:'[00:01.00]测试歌词',lyrics_revision:{}});
  api.respond('POST','/api/music/query/selection',request=>({paths:request.body.paths}));
  api.respond('POST','/api/music/jobs/preview',{task_id:'music-preview'});
  api.respond('GET','/api/tasks/result/music-preview',{state:'completed',result:{preview:{operation:'rescrape',items:[previewItem]}}});
  api.respond('POST','/api/music/jobs/execute',{task_id:'music-write'});
  api.respond('GET','/api/tasks/result/music-write',{state:'running',current_action:'音乐',message:'写入所选音乐'});
  api.respond('POST','/api/music/query/search',{items:[{media_source:'musicbrainz',media_id:'album',music_type:'album',title:'测试专辑',artists:track.artists,artist:'测试歌手',type:'音乐'}]});
  api.respond('POST','/api/music/query/detail',{media_source:'musicbrainz',media_id:'album',music_type:'album',title:'测试专辑',musicbrainz_release_id:'11111111-1111-1111-1111-111111111111',tracks:[{media_id:'song',title:'新曲名',track_number:1}],releases:[{media_id:'11111111-1111-1111-1111-111111111111',title:'测试发行'}]});
}

test('音乐只提供所选文件刮削，确认后关闭弹窗并保留日志入口', async ({page,api}) => {
  music(api);
  await page.goto('/music');
  await expect(page.getByTestId('music-library')).toContainText(track.name);
  await page.getByRole('button',{name:'选择',exact:true}).click();
  await page.getByRole('checkbox',{name:`选择 ${track.name}`}).check();
  await expect(page.getByRole('button',{name:'批量重新整理'})).toHaveCount(0);
  await page.getByRole('button',{name:'批量重新刮削'}).click();
  await page.getByRole('button',{name:'预览',exact:true}).click();
  await expect(page.getByRole('dialog').filter({hasText:'音乐刮削预览'})).toContainText('在线匹配');
  await expect(page.getByRole('dialog').filter({hasText:'音乐刮削预览'})).toContainText('新曲名');
  await page.getByRole('button',{name:'确认',exact:true}).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('link',{name:'查看日志',exact:true})).toBeVisible();
  const request=api.requests.find(request=>request.path==='/api/music/jobs/preview');
  expect(request.body.paths).toEqual([track.path]);
  expect(request.body.operation).toBe('rescrape');
});

test('MP 专辑纠正选择真实发行后重新预览原文件范围', async ({page,api}) => {
  music(api);
  await page.goto('/music');
  await page.getByRole('button',{name:'选择',exact:true}).click();
  await page.getByRole('checkbox',{name:`选择 ${track.name}`}).check();
  await page.getByRole('button',{name:'批量重新刮削'}).click();
  await page.getByRole('button',{name:'预览',exact:true}).click();
  await page.getByRole('button',{name:'纠正专辑：测试专辑'}).click();
  const dialog=page.getByRole('dialog').last();
  await expect(dialog).toContainText('核对专辑与发行版本');
  await dialog.getByText('测试专辑', {exact:true}).last().click();
  await expect(dialog).toContainText('具体发行版本');
  await dialog.getByRole('button',{name:'应用并重新预览'}).click();
  await expect.poll(()=>api.requests.filter(request=>request.path==='/api/music/jobs/preview').length).toBe(2);
  const request=api.requests.filter(request=>request.path==='/api/music/jobs/preview').at(-1);
  expect(request.body.paths).toEqual([track.path]);
  expect(request.body.selections['专辑'].musicbrainz_release_id).toBe('11111111-1111-1111-1111-111111111111');
});

for(const width of [390,1440]) test(`音乐页面和标签弹窗在 ${width}px 可读`, async({page,api}, testInfo)=>{
  await page.setViewportSize({width,height:900});
  music(api);
  await page.goto('/music');
  await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await expect(page.getByRole('dialog')).toContainText('编辑音乐标签');
  await expect(page.getByRole('button',{name:'预览修改'})).toBeVisible();
  await page.screenshot({path:testInfo.outputPath('music-editor.png')});
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
  await page.getByRole('button',{name:'取消',exact:true}).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('音乐搜索只查询本地曲库，缓存沿用 MP 表格并可删除选中项', async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/cache',{items:[{key:'opaque-key',title:'缓存曲名',artists:['歌手'],music_type:'recording',media_id:'song-id'}]});
  await page.goto('/music');
  await page.getByLabel('搜索音乐库',{exact:true}).fill('测试歌曲');
  await page.getByLabel('搜索音乐库',{exact:true}).press('Enter');
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/library').at(-1)?.query.query).toBe('测试歌曲');
  expect(api.requests.filter(row=>row.path==='/api/search_emby_library')).toEqual([]);
  await page.getByRole('tab',{name:'识别缓存'}).click();
  await expect(page.locator('.music-cache-panel')).toContainText('缓存曲名');
  await page.locator('.music-cache-panel tbody input[type=checkbox]').check();
  await page.getByRole('button',{name:'删除选中缓存'}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/cache' && row.body.keys).length).toBe(1);
  expect(api.requests.find(row=>row.path==='/api/music/query/cache' && row.body.keys).body.keys).toEqual(['opaque-key']);
});

test('音乐探索采用 MP 筛选控件，结果可打开专辑详情', async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/explore',{items:[{media_source:'musicbrainz',media_id:'album',music_type:'album',title:'测试专辑',artists:['测试歌手']}]});
  api.respond('POST','/api/music/query/related',{items:[]});
  await page.goto('/music');
  await page.getByRole('tab',{name:'探索',exact:true}).click();
  await expect(page.locator('.music-explore-filters')).toBeVisible();
  await page.locator('.music-card').click();
  await expect(page.getByRole('dialog')).toContainText('音乐详情');
  await expect(page.getByRole('dialog')).toContainText('新曲名');
});

test('音乐库按真实目录浏览歌手、专辑、歌曲，目录选择仅展开该范围', async({page,api})=>{
  music(api);
  api.respond('GET','/api/music/library',request=>({items:request.query.directory==='歌手/专辑' ? [track] : [{path:request.query.directory ? '歌手/专辑':'歌手',name:request.query.directory ? '专辑':'歌手',type:'dir'}],total:1}));
  await page.goto('/music');
  await expect(page.getByRole('tab',{name:'资料搜索',exact:true})).toHaveCount(0);
  await page.getByTestId('music-library').getByText('歌手',{exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/library').at(-1)?.query.directory).toBe('歌手');
  await page.getByTestId('music-library').getByText('专辑',{exact:true}).click();
  await expect(page.getByTestId('music-library')).toContainText(track.name);
  await page.getByRole('button',{name:'上级目录'}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/library').at(-1)?.query.directory).toBe('歌手');
});

for(const width of [390,1440]) test(`歌曲预览和歌词替换在 ${width}px 可用且应用前不写入`,async({page,api},testInfo)=>{
  music(api);
  api.respond('POST','/api/music/query/lyrics_search',{items:[{id:'lyric-live',source:'netease',title:'测试歌曲（现场版）',artists:['测试歌手'],album:'现场演唱会'}],has_more:false});
  api.respond('POST','/api/music/query/lyrics_detail',{variants:[{value:'original',title:'原文',text:'[00:10.00]新的现场歌词'},{value:'translation',title:'译文',text:'[00:10.00]翻译后的歌词'}]});
  await page.setViewportSize({width,height:900});
  await page.goto('/music');
  await page.getByRole('button',{name:`预览 ${track.name}`,exact:true}).click();
  await expect(page.getByRole('dialog')).toContainText('歌曲预览');
  await expect(page.locator('audio')).toHaveAttribute('preload','none');
  await expect(page.locator('audio')).toHaveAttribute('src',`/api/music/audio?path=${encodeURIComponent(track.path)}`);
  await page.getByRole('button',{name:'编辑歌词',exact:true}).click();
  await expect(page.getByRole('dialog')).toContainText('编辑音乐标签');
  await page.getByRole('button',{name:'搜索替换歌词'}).click();
  await expect(page.getByRole('dialog')).toHaveCount(1);
  await expect(page.getByRole('dialog')).toContainText('搜索并替换歌词');
  await page.getByText('测试歌曲（现场版）',{exact:true}).click();
  await expect(page.getByLabel('歌词内容',{exact:true})).toHaveValue('[00:10.00]新的现场歌词');
  await page.screenshot({path:testInfo.outputPath('music-lyrics-search.png')});
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
  expect(api.requests.filter(row=>row.path.startsWith('/api/music/jobs/'))).toHaveLength(0);
  await page.getByRole('button',{name:'应用歌词',exact:true}).click();
  await expect(page.getByRole('dialog')).toHaveCount(1);
  await expect(page.getByRole('dialog')).toContainText('编辑音乐标签');
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue('[00:10.00]新的现场歌词');
  expect(api.requests.filter(row=>row.path.startsWith('/api/music/jobs/'))).toHaveLength(0);
  await page.getByRole('button',{name:'预览修改',exact:true}).click();
  const request=api.requests.find(row=>row.path==='/api/music/jobs/preview');
  expect(request.body.metadata.lyrics).toBe('[00:10.00]新的现场歌词');
  expect(request.body.paths).toEqual([track.path]);
});

test('五个流媒体来源沿用探索卡片，榜单切换重置分页，详情不混用资料源',async({page,api},testInfo)=>{
  music(api);
  api.respond('POST','/api/music/query/explore',request=>({items:[{media_source:request.body.source,media_id:'123',music_type:'recording',title:'榜单歌曲',artists:['榜单歌手'],album:'榜单专辑',detail_link:'https://music.apple.com/cn/album/test/123'}],charts:[{title:'热门歌曲',value:'hot'},{title:'新歌榜',value:'new'}],chart:request.body.chart || 'hot',total:60,has_more:request.body.page<2}));
  await page.goto('/music');
  await page.getByRole('tab',{name:'探索',exact:true}).click();
  for(const [label,source] of [['网易云音乐','netease'],['QQ 音乐','qqmusic'],['Apple Music','apple'],['酷狗音乐','kugou'],['酷我音乐','kuwo']]) {
    await page.locator('.music-filter-row .v-chip').filter({hasText:label}).click();
    await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/explore').at(-1)?.body.source).toBe(source);
    await expect(page.locator('.music-card')).toContainText('榜单歌曲');
    await expect(page.locator('.music-card')).toContainText(label);
  }
  await page.getByRole('button',{name:'下一页',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/explore').at(-1)?.body.page).toBe(2);
  await page.getByRole('combobox').filter({has:page.getByLabel('音乐榜单',{exact:true})}).click();
  await page.getByRole('option',{name:'新歌榜',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/explore').at(-1)?.body).toMatchObject({page:1,chart:'new'});
  await page.setViewportSize({width:390,height:900});
  await page.screenshot({path:testInfo.outputPath('music-streaming-mobile.png')});
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
  await page.locator('.music-card').click();
  await expect(page.getByRole('dialog')).toContainText('榜单歌曲');
  expect(api.requests.filter(row=>row.path==='/api/music/query/detail')).toHaveLength(0);
});

test('取消歌词搜索不改变编辑中的歌词，来源错误可读',async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/lyrics_search',{message:'GD 音乐台暂时无法连接，请稍后重试'},400);
  await page.goto('/music');
  await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('button',{name:'搜索替换歌词'}).click();
  await expect(page.getByRole('dialog')).toHaveCount(1);
  await expect(page.getByRole('dialog')).toContainText('GD 音乐台暂时无法连接');
  await page.getByRole('button',{name:'取消',exact:true}).click();
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue('[00:01.00]测试歌词');
  expect(api.requests.filter(row=>row.path.startsWith('/api/music/jobs/'))).toHaveLength(0);
});

test('试听按需读取音频，可跳转歌词且关闭后停止',async({page,api})=>{
  music(api);
  const wave=Buffer.alloc(44+8000*2*6);
  wave.write('RIFF');wave.writeUInt32LE(wave.length-8,4);wave.write('WAVEfmt ',8);wave.writeUInt32LE(16,16);wave.writeUInt16LE(1,20);wave.writeUInt16LE(1,22);wave.writeUInt32LE(8000,24);wave.writeUInt32LE(16000,28);wave.writeUInt16LE(2,32);wave.writeUInt16LE(16,34);wave.write('data',36);wave.writeUInt32LE(wave.length-44,40);
  let reads=0;
  await page.route('**/api/music/audio?*',route=>{reads++;return route.fulfill({status:200,contentType:'audio/wav',body:wave,headers:{'Accept-Ranges':'bytes'}});});
  await page.goto('/music');
  await page.getByRole('button',{name:`预览 ${track.name}`,exact:true}).click();
  const audio=page.locator('audio');
  await expect(audio).toBeVisible();expect(reads).toBe(0);
  await audio.evaluate(async node=>{window.__testAudio=node;await node.play();});
  await expect.poll(()=>audio.evaluate(node=>node.currentTime)).toBeGreaterThan(0);
  await page.getByRole('button',{name:'跳至 1.00 秒：测试歌词'}).click();
  await expect.poll(()=>audio.evaluate(node=>node.currentTime)).toBeGreaterThanOrEqual(1);
  await page.getByRole('button',{name:'关闭',exact:true}).last().click();
  await expect.poll(()=>page.evaluate(()=>window.__testAudio.paused)).toBe(true);
});

test('切换歌词候选后忽略前一首的延迟响应',async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/lyrics_search',{items:[{id:'one',source:'netease',title:'版本一'},{id:'two',source:'netease',title:'版本二'}]});
  const pending=api.defer('POST','/api/music/query/lyrics_detail');
  await page.goto('/music');
  await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('button',{name:'搜索替换歌词'}).click();
  await expect(page.getByRole('dialog')).toHaveCount(1);
  await page.getByText('版本一',{exact:true}).click();await pending.started;
  api.respond('POST','/api/music/query/lyrics_detail',{variants:[{value:'original',title:'原文',text:'[00:02.00]版本二歌词'}]});
  await page.getByText('版本二',{exact:true}).click();
  await expect(page.getByLabel('歌词内容',{exact:true})).toHaveValue('[00:02.00]版本二歌词');
  pending.release({variants:[{value:'original',title:'原文',text:'[00:01.00]过期歌词'}]});
  await page.getByRole('button',{name:'应用歌词',exact:true}).click();
  await expect(page.getByRole('dialog')).toHaveCount(1);
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue('[00:02.00]版本二歌词');
});

test('标签编辑内搜索填入资料，歌词偏移进入同一次预览', async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/edit_metadata',{title:'填入的曲名',album:'所选专辑'});
  await page.goto('/music');
  await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('button',{name:'搜索音乐资料'}).click();
  await expect(page.getByRole('dialog')).toHaveCount(1);
  await page.locator('.media-id-selector input').fill('测试专辑');
  await page.locator('.media-id-selector input').press('Enter');
  // 单曲模式不会混入专辑结果；通过同一 MP ID 控件读取已知身份。
  await page.locator('.media-id-selector').getByRole('button',{name:'关闭',exact:true}).click();
  await page.getByLabel('MusicBrainz ID',{exact:true}).fill('11111111-1111-1111-1111-111111111111');
  await page.getByRole('button',{name:'读取资料'}).click();
  await expect(page.getByLabel('标题',{exact:true})).toHaveValue('填入的曲名');
  await page.getByLabel('整体偏移（秒）',{exact:true}).fill('2');
  await page.getByRole('button',{name:'应用到歌词'}).click();
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue('[00:03.000]测试歌词');
  await page.getByRole('button',{name:'预览修改'}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/jobs/preview').length).toBe(1);
  const body=api.requests.find(row=>row.path==='/api/music/jobs/preview').body;
  expect(body.operation).toBe('edit');
  expect(body.metadata.title).toBe('填入的曲名');
  expect(body.metadata.lyrics).toBe('[00:03.000]测试歌词');
});

test('探索上游失败可重试，筛选后数量不足仍保留下一页',async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/explore',{items:[],has_more:false,errors:{musicbrainz:'ListenBrainz 上游返回 HTTP 502'}});
  await page.goto('/music');
  await page.getByRole('tab',{name:'探索',exact:true}).click();
  await expect(page.getByRole('alert')).toContainText('HTTP 502');
  api.respond('POST','/api/music/query/explore',{items:[],has_more:true,errors:{}});
  await page.getByRole('button',{name:'重试',exact:true}).click();
  await expect(page.getByRole('button',{name:'下一页',exact:true})).toBeEnabled();
});

test('音乐资料选择器显示上游错误', async ({page,api}) => {
  music(api);
  api.respond('POST','/api/music/query/search',{items:[],errors:{musicbrainz:'音乐资料源暂时不可用'}});
  await page.goto('/music');
  await page.getByRole('button',{name:'选择',exact:true}).click();
  await page.getByRole('checkbox',{name:`选择 ${track.name}`}).check();
  await page.getByRole('button',{name:'批量重新刮削'}).click();
  await page.getByRole('button',{name:'选择音乐资料'}).click();
  await expect(page.getByRole('dialog')).toHaveCount(1);
  await page.locator('.media-id-selector input').fill('测试专辑');
  await expect(page.locator('.media-id-selector input')).toHaveValue('测试专辑');
  await page.locator('.media-id-selector input').press('Enter');
  await expect(page.locator('.media-id-selector')).toContainText('音乐资料源暂时不可用');
});

for (const width of [390,1440]) test(`音乐设置定位和独立保存 ${width}px`, async ({page,api}) => {
  await page.setViewportSize({width,height:900});
  await page.addInitScript(() => {
    const scroll = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function(...args) { if(this.id==='music-settings') window.__musicScrolled=true; return scroll.apply(this,args); };
  });
  await page.goto('/settings/general?tab=music#music-settings');
  const card=page.locator('#music-settings');
  await expect(card.getByLabel('本地音乐库目录',{exact:true})).toBeVisible();
  await expect.poll(()=>page.evaluate(()=>window.__musicScrolled)).toBeTruthy();
  await card.getByLabel('本地音乐库目录',{exact:true}).fill('/media/Media/Link/Music');
  await card.getByRole('button',{name:'保存',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.method==='POST' && row.path==='/api/config').length).toBe(1);
  const request=api.requests.find(row=>row.method==='POST' && row.path==='/api/config');
  expect(request.body.music_library_path).toBe('/media/Media/Link/Music');
  expect(request.body).not.toHaveProperty('emby_server_url');
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
});

test('移动端识别缓存长文字和删除控件可见', async ({page,api})=>{
  await page.setViewportSize({width:390,height:900});
  music(api);
  api.respond('POST','/api/music/query/cache',{items:[{key:'[音乐:v4]'+ 'abcdef0123456789'.repeat(4),title:'很长的音乐标题'.repeat(12),artists:['歌手'],music_type:'recording',media_id:'song-id'}]});
  await page.goto('/music');
  await page.getByRole('tab',{name:'识别缓存'}).click();
  await expect(page.locator('.music-cache-mobile-item')).toBeVisible();
  await expect(page.locator('.music-cache-mobile-item').getByRole('button',{name:'删除',exact:true})).toBeVisible();
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
});

test('豆瓣音乐沿用 MP 风格地区排序控件', async ({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/explore',{items:[]});
  await page.goto('/music');
  await page.getByRole('tab',{name:'探索',exact:true}).click();
  await page.getByText('豆瓣音乐',{exact:true}).click();
  const filters=page.locator('.music-douban-filters');
  await expect(filters).toBeVisible();
  await filters.getByText('爵士',{exact:true}).click();
  await filters.getByText('美国',{exact:true}).click();
  await filters.getByText('评分排序',{exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/explore').at(-1)?.body.tags).toBe('爵士,美国');
  expect(api.requests.filter(row=>row.path==='/api/music/query/explore').at(-1).body.douban_sort).toBe('S');
});

test('专辑详情复用艺术家作品与关联艺人并显示来源字段',async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/explore',{items:[{media_source:'musicbrainz',media_id:'album',music_type:'album',title:'专辑'}]});
  api.respond('POST','/api/music/query/detail',{media_source:'musicbrainz',media_id:'album',music_type:'album',title:'专辑',artists:['歌手'],artist_ids:['artist-id'],total_tracks:12,release_date:'2026-01-01',duration:4000});
  api.respond('POST','/api/music/query/related',request=>({items:request.body.kind==='artist' ? [{media_source:'musicbrainz',media_id:'related-artist',name:'关联歌手'}] : [{media_source:'musicbrainz',media_id:'another-album',music_type:'album',title:'另一张专辑'}]}));
  await page.goto('/music');
  await page.getByRole('tab',{name:'探索',exact:true}).click();
  await page.locator('.music-card').click();
  await expect(page.getByRole('dialog')).toContainText('1:06:40');
  await expect(page.getByRole('dialog')).toContainText('另一张专辑');
  await expect(page.locator('.music-artist-card')).toContainText('关联歌手');
  expect(api.requests.find(row=>row.path==='/api/music/query/related' && row.body.kind==='artist_albums').body.id).toBe('artist-id');
});
