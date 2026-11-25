import { test, expect } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';
import { readFileSync } from 'node:fs';

const silence = readFileSync(new URL('../../../backend/music_engine/tests/fixtures/audio/silence.flac', import.meta.url));

const track = {name:'01 - 测试歌曲.flac',type:'file',is_audio:true,path:'专辑/01 - 测试歌曲.flac',title:'测试歌曲',artists:['测试歌手'],album:'测试专辑',audio_specs:'FLAC / 24bit / 96kHz',signature:{mtime:1}};
const previewItem = {...track,source:track.path,source_storage:'local',source_item:{storage:'local',path:track.path,type:'file'},target:track.path,success:true,metadata:{title:'新曲名'},before:{title:track.title},music:{title:'新曲名',album:'测试专辑',artists:track.artists,group_id:'group',group_directory:'专辑',file_role:'audio',status:'matched',online_confirmed:true,read_status:'tags',field_sources:{title:'remote'},media_source:'musicbrainz',media_id:'song'}};

for(const theme of ['light','dark'])for(const width of [390,1440])test(`同步音乐名称预览可读且保存后进入新目录 ${theme} ${width}px`,async({page,api},testInfo)=>{
  music(api);await page.setViewportSize({width,height:1000});
  await page.addInitScript(theme=>localStorage.setItem('moviepilot-theme-customizer',JSON.stringify({theme,glassEnabled:false})),theme);
  const target='少女时代/GIRLS’ GENERATION II ～Girls & Peace～ (2012)/Disc 2/01 - FLOWER POWER.flac';
  api.respond('GET','/api/tasks/result/music-preview',{state:'completed',result:{preview:{operation:'edit',items:[{...previewItem,target}]}}});
  api.respond('POST','/api/music/jobs/execute',{task_id:'music-save'});
  api.respond('GET','/api/tasks/result/music-save',{state:'completed',result:{items:[{path:track.path,target,success:true}],message:'音乐处理完成'}});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByLabel('标题',{exact:true}).and(page.locator('input')).fill('FLOWER POWER');
  await page.getByRole('button',{name:'预览修改',exact:true}).click();
  const region=page.getByRole('region',{name:'同步文件名称'});
  await expect(region).toContainText(target);await expect(region).toContainText('同名歌词同步更名');
  const path=region.locator('p').first();await path.scrollIntoViewIfNeeded();
  for(const reading of await measureReadingContrast(page,[{role:'新路径',locator:path}]))expect(reading.contrast).toBeGreaterThanOrEqual(4.5);
  const dimensions=await path.evaluate(node=>({width:node.clientWidth,scrollWidth:node.scrollWidth}));
  expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.width+1);
  expect(api.requests.filter(row=>row.path==='/api/music/jobs/execute')).toHaveLength(0);
  await page.screenshot({path:testInfo.outputPath('music-name-preview.png')});
  await page.getByRole('button',{name:'保存修改',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/library').at(-1)?.query.directory).toBe(target.split('/').slice(0,-1).join('/'));
});

for (const setting of [undefined, 'album', 'recording']) test(`资料搜索遵循默认类型并隔离录音和专辑身份 ${setting || '旧配置'}`, async ({page,api}) => {
  music(api);
  const recording='11111111-1111-1111-1111-111111111111', album='949fe498-457d-401d-a271-6dce887283d2';
  const kind=setting || 'album';
  api.respond('POST','/api/music/query/inspect',{...track,album:'Oh!',year:2010,media_source:'musicbrainz',media_id:recording,music_type:'recording',music_search_type:setting});
  api.respond('POST','/api/music/query/search',{items:[{media_source:'musicbrainz',media_id:kind==='album'?album:recording,music_type:kind,title:'Oh!',year:2010,type:'音乐'}]});
  api.respond('POST','/api/music/query/detail',{media_source:'musicbrainz',media_id:album,musicbrainz_release_id:recording,title:'Oh!',releases:[{media_id:recording,title:'韩国首发版'}]});
  api.respond('POST','/api/music/query/edit_metadata',{title:'Oh!'});
  await page.goto('/music');
  await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  const type=page.getByRole('combobox').filter({has:page.getByLabel('音乐类型',{exact:true})});
  await expect(type).toContainText(kind==='album'?'专辑':'单曲');
  await expect(page.getByLabel('搜索音乐资料',{exact:true}).and(page.locator('input'))).toHaveValue(kind==='album'?'Oh! (2010)':track.title);
  await expect(page.getByLabel('MusicBrainz ID',{exact:true})).toHaveValue(kind==='album'?'':recording);
  expect(api.requests.filter(row=>row.path==='/api/music/query/detail')).toHaveLength(0);
  const id=kind==='album'?album:recording;
  await page.getByLabel('搜索音乐资料',{exact:true}).and(page.locator('input')).fill(id);
  await page.getByRole('button',{name:'搜索资料',exact:true}).click();
  await expect(page.locator('.media-id-selector')).toContainText('Oh!（2010）');
  expect(api.requests.filter(row=>row.path==='/api/music/query/search').at(-1).body).toMatchObject({query:id,kind,sources:['musicbrainz']});
  await page.locator('.media-id-selector').getByText('Oh!（2010）',{exact:true}).click();
  await expect(page.getByLabel('MusicBrainz ID',{exact:true})).toHaveValue(id);
  if(kind==='album')await expect(page.getByRole('combobox').filter({has:page.getByLabel('具体发行版本',{exact:true})})).toContainText('韩国首发版');
  await type.click();await page.getByRole('option',{name:kind==='album'?'单曲':'专辑',exact:true}).click();
  await expect(page.getByLabel('MusicBrainz ID',{exact:true})).toHaveValue('');
  expect(api.requests.filter(row=>row.path==='/api/music/jobs/execute')).toHaveLength(0);
});

test('默认专辑资料使用歌曲已有专辑组 ID',async({page,api})=>{
  music(api);
  const group='949fe498-457d-401d-a271-6dce887283d2', recording='11111111-1111-1111-1111-111111111111';
  api.respond('POST','/api/music/query/inspect',{...track,media_source:'musicbrainz',media_id:recording,music_type:'recording',musicbrainz_release_group_id:group});
  api.respond('POST','/api/music/query/detail',{media_source:'musicbrainz',media_id:group,musicbrainz_release_id:recording,releases:[{media_id:recording,title:'专辑发行版'}]});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await expect(page.getByLabel('MusicBrainz ID',{exact:true})).toHaveValue(group);
  await expect(page.getByRole('combobox').filter({has:page.getByLabel('具体发行版本',{exact:true})})).toContainText('专辑发行版');
  expect(api.requests.filter(row=>row.path==='/api/music/query/detail').map(row=>row.body.id)).toEqual([group]);
});

for(const width of [390,1440]) test(`音乐任务提示完整显示且不被路径卡片遮挡 ${width}px`,async({page,api},testInfo)=>{
  music(api);await page.setViewportSize({width,height:800});
  api.respond('GET','/api/music/library',{items:[track],tracks:Array.from({length:50},(_,i)=>({...track,path:`album/${i}.flac`,title:`歌曲 ${i}`})),total:1000});
  api.respond('POST','/api/music/jobs/scan',{task_id:'music-layout'});
  let status={state:'running',current_action:'处理所选音乐',message:'Girls’ Generation/Girls’ Generation (2007)/01 - Girls’ Generation.flac',progress_known:true,progress:50};
  api.respond('GET','/api/tasks/result/music-layout',()=>status);
  await page.goto('/music');await page.getByRole('button',{name:'扫描音乐库',exact:true}).click();
  await expect(page.getByText(status.current_action+' · '+status.message,{exact:true})).toBeVisible();
  await page.clock.install();await page.clock.pauseAt(new Date(Date.now()+100));
  for(const state of ['running','completed']){
    if(state==='completed'){
      status={state:'completed',result:{message:'音乐任务完成，共 1 项，失败 0 项',items:[{path:track.path,success:true}]}};
      await page.clock.runFor(1500);await expect(page.getByText(status.result.message,{exact:true})).toBeVisible();
    }
    const alert=page.locator('.music-page-shell > .v-alert');
    const dimensions=await alert.evaluate(node=>{const bounds=node.getBoundingClientRect(),content=node.querySelector('.v-alert__content');return {top:bounds.top,bottom:bounds.bottom,height:node.clientHeight,scrollHeight:node.scrollHeight,contentBottom:content.getBoundingClientRect().bottom,contentHeight:content.clientHeight,contentScrollHeight:content.scrollHeight,contentWidth:content.clientWidth,contentScrollWidth:content.scrollWidth};});
    expect(dimensions.scrollHeight).toBeLessThanOrEqual(dimensions.height+1);
    expect(dimensions.contentBottom).toBeLessThanOrEqual(dimensions.bottom);
    expect(dimensions.contentScrollHeight).toBeLessThanOrEqual(dimensions.contentHeight+1);
    expect(dimensions.contentScrollWidth).toBeLessThanOrEqual(dimensions.contentWidth+1);
    const path=await page.getByTestId('music-library-path').boundingBox();
    expect(path.y).toBeGreaterThanOrEqual(dimensions.bottom);
    const button=alert.getByRole('button',{name:state==='running'?'查看日志':'查看详情',exact:true});
    await button.click({trial:true});
    await page.screenshot({path:testInfo.outputPath(`music-status-${state}.png`)});
    const footer=await page.getByRole('contentinfo',{name:'音乐库分页'}).boundingBox();
    expect(footer.y+footer.height).toBeLessThanOrEqual(801);
  }
  await page.clock.runFor(5000);await expect(page.getByText(status.result.message,{exact:true})).toHaveCount(0);
});

for(const theme of ['light','dark'])for(const width of [390,1440]) test(`备注描述编辑预览及清空保持可读 ${theme} ${width}px`,async({page,api},testInfo)=>{
  music(api); await page.setViewportSize({width,height:1000});
  await page.addInitScript(theme=>localStorage.setItem('moviepilot-theme-customizer',JSON.stringify({theme,glassEnabled:true,glassAppearance:'frosted',glassQuality:'css'})),theme);
  const comment='Ripped by test', description='现场演唱会简介\n保留原有时间与版本信息';
  api.respond('POST','/api/music/query/inspect',{...track,comment,description});
  api.respond('GET','/api/tasks/result/music-preview',{task_id:'music-preview',is_running:false,state:'completed',result_status:200,result:{preview:{operation:'edit',items:[{...previewItem,before:{comment,description},metadata:{comment:'修改后的备注\n第二行',description:''},tag_updates:{comment:['修改后的备注\n第二行'],description:[]}}]}}});
  await page.goto('/music'); await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await expect(page.getByLabel('备注',{exact:true})).toHaveValue(comment);
  await expect(page.getByLabel('简介 / 描述',{exact:true})).toHaveValue(description);
  await page.getByLabel('备注',{exact:true}).fill('修改后的备注\n第二行');
  await page.getByLabel('简介 / 描述',{exact:true}).fill('');
  await page.screenshot({path:testInfo.outputPath('comment-editor.png')});
  await page.getByRole('button',{name:'预览修改',exact:true}).click();
  await expect(page.getByRole('button',{name:'保存修改',exact:true})).toBeEnabled();
  const dialog=page.locator('.v-dialog.v-overlay--active').filter({hasText:'音乐修改预览'});
  await expect(dialog).toContainText(comment);await expect(dialog).toContainText('修改后的备注');await expect(dialog).toContainText('已清空');
  await expect(dialog.locator('article.v-card')).toHaveCount(1);
  const changed=dialog.getByRole('region',{name:'备注',exact:true});
  await expect(changed).toContainText('当前值');await expect(changed).toContainText('修改后');
  const before=changed.locator('.music-field-value').first().locator('p'),after=changed.locator('.music-field-value--after p');
  await expect(before).toHaveText(comment);await expect(after).toHaveText('修改后的备注\n第二行');
  for(const probe of [{role:'当前备注',locator:before},{role:'修改后备注',locator:after}]){
    await probe.locator.scrollIntoViewIfNeeded();
    for(const reading of await measureReadingContrast(page,[probe]))expect(reading.contrast).toBeGreaterThanOrEqual(4.5);
    const size=await probe.locator.evaluate(node=>({width:node.clientWidth,scrollWidth:node.scrollWidth}));expect(size.scrollWidth).toBeLessThanOrEqual(size.width+1);
  }
  const request=api.requests.find(row=>row.path==='/api/music/jobs/preview');
  expect(request.body.metadata.comment).toBe('修改后的备注\n第二行');expect(request.body.metadata.description).toBe('');
  expect(api.requests.filter(row=>row.path==='/api/music/jobs/execute')).toHaveLength(0);
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
  await page.screenshot({path:testInfo.outputPath('comment-preview.png')});
  await page.getByRole('button',{name:'返回编辑',exact:true}).click();
  await expect(page.getByLabel('备注',{exact:true}).and(page.locator('textarea'))).toHaveValue('修改后的备注\n第二行');
  await expect(page.getByLabel('简介 / 描述',{exact:true}).and(page.locator('textarea'))).toHaveValue('');
});

test('设置默认单曲后资料与封面使用曲名，关闭后重置关键词',async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/inspect',{...track,title:'天地行',artists:['戴荃'],music_search_type:'recording'});
  api.respond('POST','/api/music/query/lyrics_search',{items:[],has_more:false});
  await page.goto('/music');
  await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await expect(page.getByLabel('搜索音乐资料',{exact:true}).and(page.locator('input'))).toHaveValue('天地行');
  await page.getByRole('button',{name:'搜索资料',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/search').at(-1)?.body.query).toBe('天地行');
  await page.locator('.media-id-selector').getByRole('button',{name:'关闭',exact:true}).click();
  await page.getByLabel('搜索音乐资料',{exact:true}).and(page.locator('input')).fill('手工关键词');
  await page.getByRole('button',{name:'搜索资料',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/search').at(-1)?.body.query).toBe('手工关键词');
  await page.locator('.media-id-selector').getByRole('button',{name:'关闭',exact:true}).click();
  await page.getByRole('button',{name:'从资料源替换封面',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/search').at(-1)?.body.query).toBe('天地行');
  await page.locator('.media-id-selector').getByRole('button',{name:'关闭',exact:true}).click();
  await page.getByRole('button',{name:'搜索替换歌词',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/lyrics_search').at(-1)?.body.query).toBe('天地行');
});

for(const theme of ['light','dark']) for(const width of [390,1440]) test(`歌曲长标签与分页可读且不裁切 ${theme} ${width}px`,async({page,api},testInfo)=>{
  music(api);
  const title='少女时代 Girls’ Generation — Into The New World 现场演唱会特别版';
  const album='The First Asia Tour Concert — Into The New World (Live Album)';
  api.respond('GET','/api/music/library',{items:[{...track,name:`01 - ${title}.flac`}],tracks:[{...track,title,album}],total:1});
  await page.addInitScript(theme=>localStorage.setItem('moviepilot-theme-customizer',JSON.stringify({theme,glassEnabled:false})),theme);
  await page.setViewportSize({width,height:1100});
  await page.goto('/music');
  const songs=page.getByTestId('music-folder-tracks');
  const titleButton=songs.getByRole('button',{name:`编辑标题：${title}`,exact:true});
  await titleButton.scrollIntoViewIfNeeded();
  for(const probe of [{role:'歌曲标题',locator:titleButton},{role:'专辑名',locator:songs.getByRole('button',{name:`编辑专辑：${title}`,exact:true})}]){
    await probe.locator.evaluate(node=>node.scrollIntoView({block:'center',behavior:'instant'}));
    for(const reading of await measureReadingContrast(page,[probe]))expect(reading.contrast).toBeGreaterThanOrEqual(4.5);
  }
  for(const element of [titleButton,songs.getByRole('button',{name:`编辑专辑：${title}`,exact:true})]){
    const box=await element.evaluate(node=>({width:node.clientWidth,scrollWidth:node.scrollWidth,height:node.clientHeight,scrollHeight:node.scrollHeight}));
    expect(box.scrollWidth).toBeLessThanOrEqual(box.width+1);expect(box.scrollHeight).toBeLessThanOrEqual(box.height+1);
  }
  const current=page.locator('.music-library-footer .v-pagination__item--is-active .v-btn__content');
  await current.scrollIntoViewIfNeeded();
  const pageReadings=await measureReadingContrast(page,[{role:'当前页',locator:current}]);
  expect(pageReadings[0].contrast).toBeGreaterThanOrEqual(4.5);
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
  await page.screenshot({path:testInfo.outputPath(`music-readable-${theme}-${width}.png`),fullPage:true});
  api.respond('POST','/api/music/query/explore',{items:[],charts:[{title:'流行歌曲排行榜',value:'hot'},{title:'新歌榜',value:'new'}],chart:'hot',has_more:false});
  await page.getByRole('tab',{name:'探索',exact:true}).click();
  await page.locator('.music-source-chips .v-chip').filter({hasText:'网易云音乐'}).click();
  const sourceBounds=await page.locator('.music-source-chips .v-chip').evaluateAll(nodes=>nodes.map(node=>{const box=node.getBoundingClientRect(),parent=node.closest('.v-slide-group__container').getBoundingClientRect();return box.left>=parent.left-1 && box.right<=parent.right+1;}));
  expect(sourceBounds.every(Boolean)).toBeTruthy();
  const selectedChart=page.locator('.streaming-chart-chips .v-chip--selected .v-chip__content');
  await selectedChart.scrollIntoViewIfNeeded();
  const filterReadings=await measureReadingContrast(page,[{role:'已选榜单',locator:selectedChart}]);
  expect(filterReadings[0].contrast).toBeGreaterThanOrEqual(4.5);
  await page.screenshot({path:testInfo.outputPath(`music-filters-${theme}-${width}.png`)});
});

for(const width of [390,1440]) test(`进入目录直接显示全部歌曲并定位标签编辑 ${width}px`,async({page,api},testInfo)=>{
  music(api);
  const second={...track,path:'专辑/02.flac',name:'02.flac',title:'第二首歌曲'};
  api.respond('GET','/api/music/library',{items:[track],tracks:[track,second],total:2});
  api.respond('POST','/api/music/query/inspect',request=>({...second,path:request.body.path,lyrics:'[00:01.00]歌词'}));
  await page.setViewportSize({width,height:1000});
  await page.goto('/music');
  const songs=page.getByTestId('music-folder-tracks');
  await expect(songs).toContainText('2 首歌曲');
  await expect(songs).toContainText('第二首歌曲');
  expect(api.requests.filter(row=>row.path==='/api/music/query/inspect')).toHaveLength(0);
  await page.screenshot({path:testInfo.outputPath(`music-folder-${width}.png`)});
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
  await songs.getByRole('button',{name:'编辑艺术家：第二首歌曲',exact:true}).click();
  await expect(page.getByRole('dialog')).toContainText('编辑音乐标签');
  await expect(page.locator('[data-music-field="artists"] input')).toBeFocused();
  expect(api.requests.find(row=>row.path==='/api/music/query/inspect').body.path).toBe(second.path);
});

test('标签支持名称与 MusicBrainz ID，流媒体专辑逐首选择，单独替换封面保留其他标签',async({page,api})=>{
  music(api);
  const mbid='11111111-1111-1111-1111-111111111111';
  api.respond('POST','/api/music/query/edit_metadata',request=>request.body.selection.media_source==='musicbrainz' ? {title:'MB 曲名',media_source:'musicbrainz',media_id:mbid,music_type:'recording'} : {title:'不应覆盖的曲名',media_source:'netease',media_id:'song123',music_type:'recording',cover_url:'https://example.com/cover.jpg'});
  await page.goto('/music');
  await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByLabel('MusicBrainz ID',{exact:true}).fill(mbid);
  await page.getByRole('button',{name:'读取资料',exact:true}).click();
  await expect(page.getByLabel('标题',{exact:true})).toHaveValue('MB 曲名');
  await page.getByRole('combobox').filter({has:page.getByLabel('资料来源',{exact:true})}).click();
  await page.getByRole('option',{name:'网易云音乐',exact:true}).click();
  await page.getByRole('combobox').filter({has:page.getByLabel('音乐类型',{exact:true})}).click();
  await page.getByRole('option',{name:'专辑',exact:true}).click();
  await expect(page.getByLabel('搜索音乐资料',{exact:true}).and(page.locator('input'))).toHaveValue(track.album);
  await page.getByLabel('搜索音乐资料',{exact:true}).and(page.locator('input')).fill('手工输入的专辑名');
  api.respond('POST','/api/music/query/search',{items:[{media_source:'netease',media_id:'album123',music_type:'album',title:'专辑候选',type:'音乐'}]});
  api.respond('POST','/api/music/query/detail',{title:'专辑候选',tracks:[{media_id:'song123',title:'封面曲目',music_type:'recording'}]});
  await page.getByRole('button',{name:'从资料源替换封面',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/search').at(-1)?.body.query).toBe('手工输入的专辑名');
  await page.locator('.media-id-selector').getByRole('button',{name:'关闭',exact:true}).click();
  await expect(page.getByLabel('搜索音乐资料',{exact:true}).and(page.locator('input'))).toHaveValue(track.album);
  await page.getByRole('button',{name:'从资料源替换封面',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/search').at(-1)?.body.query).toBe(track.album);
  await page.getByText('专辑候选',{exact:true}).click();
  await expect(page.getByRole('button',{name:'返回专辑',exact:true})).toBeVisible();
  await page.getByText('封面曲目',{exact:true}).click();
  await expect(page.getByRole('combobox').filter({has:page.getByLabel('替换封面',{exact:true})})).toContainText('资料源封面');
  await expect(page.getByLabel('标题',{exact:true})).toHaveValue('MB 曲名');
  await page.getByRole('button',{name:'预览修改',exact:true}).click();
  const request=api.requests.find(row=>row.path==='/api/music/jobs/preview');
  expect(request.body.metadata.media_id).toBe(mbid);
  expect(request.body.metadata.cover_url).toBe('https://example.com/cover.jpg');
  expect(request.body.metadata.title).toBe('MB 曲名');
});

test('歌词可搜索专辑并浏览曲目，应用前不写入',async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/lyrics_search',request=>({items:request.body.album_id ? [{source:'netease',id:'song2',title:'专辑第二首',music_type:'recording'}] : request.body.kind==='album' ? [{source:'netease',id:'album1',title:'现场专辑',music_type:'album'}] : [],has_more:false}));
  api.respond('POST','/api/music/query/lyrics_detail',{variants:[{value:'original',title:'原文',text:'[00:02]专辑歌词'}]});
  await page.goto('/music');
  await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('button',{name:'搜索替换歌词'}).click();
  await page.locator('.v-chip').filter({hasText:'搜索专辑'}).click();
  await expect(page.getByLabel('搜索专辑或歌手',{exact:true})).toHaveValue(track.album);
  await page.getByLabel('搜索专辑或歌手',{exact:true}).fill('修改后的专辑名');
  await page.getByLabel('搜索专辑或歌手',{exact:true}).press('Enter');
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/lyrics_search').at(-1)?.body.query).toBe('修改后的专辑名');
  await page.getByText('现场专辑',{exact:true}).click();
  await page.getByText('专辑第二首',{exact:true}).click();
  await expect(page.getByLabel('歌词内容',{exact:true})).toHaveValue('[00:02]专辑歌词');
  expect(api.requests.find(row=>row.path==='/api/music/query/lyrics_detail').body.id).toBe('song2');
  await page.getByRole('button',{name:'应用歌词',exact:true}).click();
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue('[00:02]专辑歌词');
  expect(api.requests.filter(row=>row.path.startsWith('/api/music/jobs/'))).toHaveLength(0);
});

for(const width of [390,1440]) test(`歌词通过专辑 ID 和链接浏览分页并预览 ${width}px`,async({page,api},testInfo)=>{
  music(api);await page.setViewportSize({width,height:1000});
  api.respond('POST','/api/music/query/lyrics_search',request=>{
    if(!request.body.album_id)return {items:[],has_more:false};
    const qq=request.body.album_id.includes('002Fp2lJ1sw4bF'),source=qq?'qqmusic':'netease';
    return {source,album:{id:qq?'002Fp2lJ1sw4bF':'2087510',title:'Girls’ Generation — The First Asia Tour Concert Into The New World (Live Album)'},items:[{source,id:`song-${request.body.page}`,title:`专辑曲目 ${request.body.page}`}],has_more:request.body.page===1};
  });
  api.respond('POST','/api/music/query/lyrics_detail',{variants:[{value:'original',title:'原文',text:'[00:02]所选专辑歌词'}]});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('button',{name:'搜索替换歌词',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/lyrics_search').length).toBe(1);
  await page.getByText('专辑 ID / 链接',{exact:true}).click();
  const input=page.getByLabel('专辑 ID 或链接',{exact:true});
  await expect(input).toHaveValue('');
  await page.getByRole('button',{name:'搜索歌词',exact:true}).click();
  expect(api.requests.filter(row=>row.path==='/api/music/query/lyrics_search')).toHaveLength(1);
  for(const reference of ['2087510','https://music.163.com/#/album?id=2087510','https://y.qq.com/n/ryqq_v2/albumDetail/002Fp2lJ1sw4bF']){
    await input.fill(reference);await input.press('Enter');
    await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/lyrics_search').at(-1)?.body.album_id).toBe(reference);
    await expect(page.getByText('专辑曲目 1',{exact:true})).toBeVisible();
  }
  await expect(page.getByRole('combobox').filter({has:page.getByLabel('歌词来源',{exact:true})})).toContainText('QQ 音乐');
  expect(api.requests.filter(row=>row.path==='/api/music/query/lyrics_search')).toHaveLength(4);
  await page.getByRole('button',{name:'歌词下一页',exact:true}).click();
  await expect(page.getByText('专辑曲目 2',{exact:true})).toBeVisible();
  expect(api.requests.filter(row=>row.path==='/api/music/query/lyrics_search').at(-1).body).toMatchObject({source:'qqmusic',album_id:'002Fp2lJ1sw4bF',page:2});
  await page.getByText('专辑曲目 2',{exact:true}).click();
  await expect(page.getByLabel('歌词内容',{exact:true})).toHaveValue('[00:02]所选专辑歌词');
  expect(api.requests.find(row=>row.path==='/api/music/query/lyrics_detail').body).toEqual({source:'qqmusic',id:'song-2'});
  const dialog=page.locator('.v-dialog.v-overlay--active').filter({hasText:'搜索并替换歌词'});
  expect(await dialog.locator('.v-card-text').evaluate(node=>node.scrollWidth<=node.clientWidth+1)).toBeTruthy();
  await page.screenshot({path:testInfo.outputPath('lyrics-album-id.png')});
  await page.getByRole('button',{name:'应用歌词',exact:true}).click();
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue('[00:02]所选专辑歌词');
  expect(api.requests.filter(row=>row.path.startsWith('/api/music/jobs/'))).toHaveLength(0);
  await page.getByRole('button',{name:'搜索替换歌词',exact:true}).click();
  await expect(page.getByLabel('搜索歌曲或歌手',{exact:true})).toHaveValue(track.title);
});

test('专辑 ID 查询切换来源清理旧结果，GD 保留名称搜索',async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/lyrics_search',request=>({items:request.body.album_id ? [{id:'song',source:request.body.source,title:'旧专辑曲目'}] : [],has_more:false}));
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('button',{name:'搜索替换歌词',exact:true}).click();await page.getByText('专辑 ID / 链接',{exact:true}).click();
  await page.getByLabel('专辑 ID 或链接',{exact:true}).fill('2087510');await page.getByLabel('专辑 ID 或链接',{exact:true}).press('Enter');
  await expect(page.getByText('旧专辑曲目',{exact:true})).toBeVisible();
  await page.getByRole('combobox').filter({has:page.getByLabel('歌词来源',{exact:true})}).click();
  await page.getByRole('option',{name:'QQ 音乐',exact:true}).click();
  await expect(page.getByLabel('专辑 ID 或链接',{exact:true})).toHaveValue('');
  await expect(page.getByText('旧专辑曲目',{exact:true})).toHaveCount(0);
  await page.getByRole('combobox').filter({has:page.getByLabel('歌词来源',{exact:true})}).click();
  await page.getByRole('option',{name:'网易云音乐 · GD 音乐台',exact:true}).click();
  await expect(page.getByText('专辑 ID / 链接',{exact:true})).toHaveCount(0);
  await expect(page.getByLabel('搜索专辑或歌手',{exact:true})).toHaveValue(track.album);
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/lyrics_search').at(-1)?.body).toMatchObject({source:'gd_netease',kind:'album',query:track.album});
});

test('资料选择器再次打开可按新的专辑 ID 浏览歌曲',async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/detail',request=>({title:`专辑 ${request.body.id}`,tracks:[{media_id:'track1',title:`歌曲 ${request.body.id}`}]}));
  await page.goto('/music');
  await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('combobox').filter({has:page.getByLabel('资料来源',{exact:true})}).click();
  await page.getByRole('option',{name:'QQ 音乐',exact:true}).click();
  await page.getByRole('combobox').filter({has:page.getByLabel('音乐类型',{exact:true})}).click();
  await page.getByRole('option',{name:'专辑',exact:true}).click();
  for(const identity of ['albumA','albumB']){
    await page.getByLabel('QQ 音乐 ID',{exact:true}).fill(identity);
    await page.getByRole('button',{name:'读取资料',exact:true}).click();
    await expect(page.locator('.media-id-selector')).toContainText(`歌曲 ${identity}`);
    await page.locator('.media-id-selector').getByRole('button',{name:'关闭',exact:true}).click();
  }
  expect(api.requests.filter(row=>row.path==='/api/music/query/detail').map(row=>row.body.id)).toEqual(['albumA','albumB']);
});

for(const width of [390,1440]) test(`发行版本与标签优先选择接口候选 ${width}px`,async({page,api},testInfo)=>{
  music(api);await page.setViewportSize({width,height:1000});
  const group='11111111-1111-1111-1111-111111111111',release='22222222-2222-2222-2222-222222222222';
  api.respond('POST','/api/music/query/search',{items:[{media_source:'musicbrainz',media_id:group,music_type:'album',title:'所选专辑',type:'音乐'}]});
  api.respond('POST','/api/music/query/detail',{media_source:'musicbrainz',media_id:group,title:'所选专辑',musicbrainz_release_id:release,releases:[{media_id:release,title:'日本首发特别版',date:'2024-01-02',country:'JP',formats:['CD'],track_count:12}],total_tracks:12,total_discs:1,tracks:[{media_source:'musicbrainz',music_type:'recording',media_id:'33333333-3333-3333-3333-333333333333',title:'候选曲名',artists:['候选歌手'],album:'所选专辑',album_artist:'候选歌手',isrc:'USABC2400001',composers:['作曲家'],performers:{guitar:['演奏者']},track_number:2,disc_number:1,release_year:2024,musicbrainz_release_id:release,musicbrainz_release_group_id:group}]});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('combobox').filter({has:page.getByLabel('音乐类型',{exact:true})}).click();await page.getByRole('option',{name:'专辑',exact:true}).click();
  await page.getByRole('button',{name:'搜索资料',exact:true}).click();await page.locator('.media-id-selector').getByText('所选专辑',{exact:true}).click();
  const edition=page.getByRole('combobox').filter({has:page.getByLabel('具体发行版本',{exact:true})});
  await expect(edition).toContainText('日本首发特别版');
  await page.getByRole('combobox').filter({has:page.getByLabel('专辑曲目',{exact:true})}).click();await page.getByRole('option',{name:'1.2 · 候选曲名 · 候选歌手',exact:true}).click();
  for(const label of ['标题','专辑','专辑艺术家','ISRC','艺术家','作曲','曲序','总曲数','碟号','总碟数','发行年份','乐器 / 声部','演奏者姓名'])await expect(page.getByRole('combobox').filter({has:page.getByLabel(label,{exact:true})})).toBeVisible();
  await expect(page.getByLabel('原始年份',{exact:true})).toHaveAttribute('type','number');
  await page.getByLabel('原始年份',{exact:true}).fill('2000');
  await page.getByRole('button',{name:'预览修改',exact:true}).click();
  const metadata=api.requests.find(row=>row.path==='/api/music/jobs/preview').body.metadata;
  expect(metadata.title).toBe('候选曲名');expect(metadata.isrc).toBe('USABC2400001');expect(metadata.original_year).toBe(2000);expect(metadata.musicbrainz_release_id).toBe(release);
  expect(api.requests.filter(row=>row.path==='/api/music/jobs/execute')).toHaveLength(0);
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
  await page.screenshot({path:testInfo.outputPath('selected-release-tags.png')});
});

test('切换专辑后丢弃旧发行列表，读取失败可以重试',async({page,api})=>{
  music(api);
  const first='11111111-1111-1111-1111-111111111111',second='22222222-2222-2222-2222-222222222222';
  let releaseOld,oldRequested=false;
  api.respond('POST','/api/music/query/detail',request=>request.body.id===first ? new Promise(resolve=>{oldRequested=true;releaseOld=resolve;}) : {media_source:'musicbrainz',media_id:second,title:'新专辑',musicbrainz_release_id:second,releases:[{media_id:second,title:'新发行'}]});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('combobox').filter({has:page.getByLabel('音乐类型',{exact:true})}).click();await page.getByRole('option',{name:'专辑',exact:true}).click();
  await page.getByLabel('MusicBrainz ID',{exact:true}).fill(first);await expect.poll(()=>oldRequested).toBe(true);
  await page.getByLabel('MusicBrainz ID',{exact:true}).fill(second);
  const edition=page.getByRole('combobox').filter({has:page.getByLabel('具体发行版本',{exact:true})});
  await expect(edition).toContainText('新发行');
  releaseOld({media_source:'musicbrainz',media_id:first,musicbrainz_release_id:first,releases:[{media_id:first,title:'旧发行'}]});
  await expect(edition).not.toContainText('旧发行');
  api.respond('POST','/api/music/query/detail',{error:'发行版本暂时不可用'},503);
  await page.getByLabel('MusicBrainz ID',{exact:true}).fill(first);
  await expect(page.getByRole('button',{name:'重新读取发行版本',exact:true})).toBeVisible();
  await expect(page.getByRole('button',{name:'读取资料',exact:true})).toBeDisabled();
  api.respond('POST','/api/music/query/detail',{media_source:'musicbrainz',media_id:first,musicbrainz_release_id:first,releases:[{media_id:first,title:'恢复后的发行'}]});
  await page.getByRole('button',{name:'重新读取发行版本',exact:true}).click();await expect(edition).toContainText('恢复后的发行');
  await expect(page.getByRole('button',{name:'读取资料',exact:true})).toBeEnabled();
});

test('批量刮削选择发行版本后将所选身份传入预览',async({page,api})=>{
  music(api);
  const group='11111111-1111-1111-1111-111111111111',release='22222222-2222-2222-2222-222222222222';
  api.respond('POST','/api/music/query/search',{items:[{media_source:'musicbrainz',media_id:group,music_type:'album',title:'批量专辑',type:'音乐'}]});
  api.respond('POST','/api/music/query/detail',{media_source:'musicbrainz',media_id:group,musicbrainz_release_id:release,releases:[{media_id:release,title:'所选发行',country:'JP',formats:['CD']}]});
  await page.goto('/music');await page.getByRole('button',{name:'选择',exact:true}).click();await page.getByRole('checkbox',{name:`选择 ${track.name}`}).check();await page.getByRole('button',{name:'批量重新刮削',exact:true}).click();
  await page.getByRole('button',{name:'选择音乐资料',exact:true}).click();
  await page.locator('.media-id-selector input').fill('批量专辑');await page.locator('.media-id-selector input').press('Enter');await page.locator('.media-id-selector').getByText('批量专辑',{exact:true}).click();
  await expect(page.getByRole('combobox').filter({has:page.getByLabel('具体发行版本',{exact:true})})).toContainText('所选发行');
  await page.getByRole('button',{name:'预览',exact:true}).click();
  expect(api.requests.find(row=>row.path==='/api/music/jobs/preview').body.selection).toEqual({media_source:'musicbrainz',media_id:group,music_type:'album',musicbrainz_release_id:release});
});

function music(api) {
  api.respond('GET','/api/music/audio',()=>silence);
  api.respond('GET','/api/music/image',{message:'没有封面'},404);
  api.respond('GET','/api/music/library',{items:[track],total:1});
  api.respond('POST','/api/music/query/inspect',{...track,lyrics:'[00:01.00]测试歌词',lyrics_revision:{},music_search_type:'recording'});
  api.respond('POST','/api/music/query/selection',request=>({paths:request.body.paths}));
  api.respond('POST','/api/music/jobs/preview',{task_id:'music-preview'},202);
  api.respond('GET','/api/tasks/result/music-preview',{state:'completed',result:{preview:{operation:'rescrape',items:[previewItem]}}});
  api.respond('POST','/api/music/jobs/execute',{task_id:'music-write'},202);
  api.respond('GET','/api/tasks/result/music-write',{state:'running',current_action:'音乐',message:'写入所选音乐'});
  api.respond('POST','/api/music/query/search',{items:[{media_source:'musicbrainz',media_id:'album',music_type:'album',title:'测试专辑',artists:track.artists,artist:'测试歌手',type:'音乐'}]});
  api.respond('POST','/api/music/query/detail',{media_source:'musicbrainz',media_id:'album',music_type:'album',title:'测试专辑',musicbrainz_release_id:'11111111-1111-1111-1111-111111111111',tracks:[{media_id:'song',title:'新曲名',track_number:1}],releases:[{media_id:'11111111-1111-1111-1111-111111111111',title:'测试发行'}]});
}

test('歌词修改保留草稿，等待预览后才允许确认保存',async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/inspect',{...track,lyrics:'',lyrics_revision:{'.lrc':null,'.txt':null,'.lyricsfile.yaml':null}});
  api.respond('GET','/api/tasks/result/music-preview',{state:'running'});
  const lyrics='[00:03.50]核对后才能保存的歌词';
  const completed={state:'completed',result:{preview:{operation:'edit',items:[{...previewItem,lyrics_edit:{text:lyrics},music:{...previewItem.music,status:'manual'}}]}}};
  await page.goto('/music');
  await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true}).fill(lyrics);
  await page.getByRole('button',{name:'预览修改',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/tasks/result/music-preview').length).toBeGreaterThan(0);
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue(lyrics);
  await expect(page.getByRole('button',{name:'预览修改',exact:true})).toBeDisabled();
  expect(api.requests.filter(row=>row.path==='/api/music/jobs/execute')).toHaveLength(0);
  api.respond('GET','/api/tasks/result/music-preview',completed);
  await expect(page.locator('.v-dialog.v-overlay--active').filter({hasText:'音乐修改预览'})).toBeVisible();
  await expect(page.locator('.lyrics-preview')).toBeVisible();
  await expect(page.locator('.lyrics-preview')).toHaveText(lyrics);
  await expect.poll(()=>page.evaluate(()=>sessionStorage.getItem('etk.music.task'))).toBeNull();
  await page.getByRole('button',{name:'返回编辑',exact:true}).click();
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue(lyrics);
  await page.getByRole('button',{name:'预览修改',exact:true}).click();
  await expect(page.getByRole('button',{name:'保存修改',exact:true})).toBeEnabled();
  expect(api.requests.filter(row=>row.path==='/api/music/jobs/execute')).toHaveLength(0);
  await page.getByRole('button',{name:'保存修改',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/jobs/execute').length).toBe(1);
  expect(api.requests.find(row=>row.path==='/api/music/jobs/execute').body).toEqual({preview_task_id:'music-preview'});
  expect(api.requests.some(row=>row.path.endsWith('/undefined'))).toBe(false);
});

test('预览校验失败可返回编辑且不允许保存',async({page,api})=>{
  music(api);
  api.respond('GET','/api/tasks/result/music-preview',{state:'completed',result:{preview:{operation:'edit',items:[{...previewItem,success:false,message:'音乐文件在打开编辑器后已变化，请重新打开'}]}}});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true}).fill('[00:01]保留草稿');
  await page.getByRole('button',{name:'预览修改',exact:true}).click();
  await expect(page.locator('.v-dialog.v-overlay--active').filter({hasText:'音乐修改预览'})).toContainText('音乐文件在打开编辑器后已变化');
  await expect(page.getByRole('button',{name:'保存修改',exact:true})).toBeDisabled();
  await page.getByRole('button',{name:'返回编辑',exact:true}).click();
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue('[00:01]保留草稿');
  expect(api.requests.filter(row=>row.path==='/api/music/jobs/execute')).toHaveLength(0);
});

for(const width of [390,1440]) test(`来源译文制作双语歌词保留当前原文时间轴 ${width}px`,async({page,api},testInfo)=>{
  music(api);await page.setViewportSize({width,height:1000});
  api.respond('POST','/api/music/query/inspect',{...track,lyrics:'[00:10.00]Hello\n[00:20.00]Unmatched local line'});
  api.respond('POST','/api/music/query/lyrics_search',{items:[{id:'candidate',source:'netease',title:'来源歌词'}]});
  api.respond('POST','/api/music/query/lyrics_detail',{variants:[{value:'original',title:'原文',text:'[00:01]Hello\n[00:02]Other'},{value:'translation',title:'译文',text:'[00:01]你好\n[00:02]另一句'}]});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await expect(page.getByRole('button',{name:'翻译',exact:true})).toHaveCount(0);
  await page.getByRole('button',{name:'搜索替换歌词',exact:true}).click();await page.getByText('来源歌词',{exact:true}).click();
  await page.getByRole('checkbox',{name:'使用译文制作双语歌词',exact:true}).check();
  const expected='[00:10.00]Hello\n[00:10.00]你好\n[00:20.00]Unmatched local line';
  await expect(page.getByLabel('歌词内容',{exact:true})).toHaveValue(expected);
  await expect(page.getByText('保留当前原文和时间轴，已匹配 1/2 句；未匹配的句子保留原样。',{exact:true})).toBeVisible();
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
  await page.screenshot({path:testInfo.outputPath('source-bilingual-preview.png')});
  await page.getByRole('button',{name:'应用双语歌词',exact:true}).click();
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue(expected);
  expect(api.requests.filter(row=>row.path.startsWith('/api/music/jobs/'))).toHaveLength(0);
});

for(const theme of ['light','dark'])for(const width of [390,1440]) test(`酷狗译文和音译可选且短译文合并后可读 ${theme} ${width}px`,async({page,api},testInfo)=>{
  music(api);await page.setViewportSize({width,height:1000});
  await page.addInitScript(theme=>localStorage.setItem('moviepilot-theme-customizer',JSON.stringify({theme,glassEnabled:true,glassAppearance:'frosted',glassQuality:'css'})),theme);
  const original='[00:00.37]겁이 나서 시작조차 안 해 봤다면\n[00:17.78]Bring the boys out';
  api.respond('POST','/api/music/query/inspect',{...track,lyrics:original});
  api.respond('POST','/api/music/query/lyrics_search',request=>({items:[{id:'krc-song',source:request.body.source,title:'The Boys'}]}));
  api.respond('POST','/api/music/query/lyrics_detail',{variants:[
    {value:'original',title:'原文',text:'[00:00.30]겁이 나서\n[00:01.64]시작조차\n[00:02.70]안 해 봤다면'},
    {value:'translation',title:'译文',text:'[00:00.30]如果因为\n[00:01.64]害怕\n[00:02.70]就什么都还没有开始的话'},
    {value:'romanized',title:'音译',text:'[00:00.30]勾比 那搜'},
  ]});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('button',{name:'搜索替换歌词',exact:true}).click();
  await page.getByRole('combobox').filter({has:page.getByLabel('歌词来源',{exact:true})}).click();
  await page.getByRole('option',{name:'酷狗音乐',exact:true}).click();
  await page.getByText('The Boys',{exact:true}).click();
  await page.getByText('音译',{exact:true}).click();
  await expect(page.getByLabel('歌词内容',{exact:true})).toHaveValue('[00:00.30]勾比 那搜');
  await page.getByRole('checkbox',{name:'使用译文制作双语歌词',exact:true}).check();
  const expected=original.replace('\n[00:17.78]','\n[00:00.37]如果因为 害怕 就什么都还没有开始的话\n[00:17.78]');
  await expect(page.getByLabel('歌词内容',{exact:true})).toHaveValue(expected);
  const line=page.locator('.lyric-line').filter({hasText:'如果因为'});
  await line.scrollIntoViewIfNeeded();
  const bounds=await line.evaluate(node=>({width:node.clientWidth,scrollWidth:node.scrollWidth,height:node.clientHeight,scrollHeight:node.scrollHeight}));
  expect(bounds.scrollWidth).toBeLessThanOrEqual(bounds.width+1);expect(bounds.scrollHeight).toBeLessThanOrEqual(bounds.height+1);
  for(const reading of await measureReadingContrast(page,[{role:'合并译文',locator:line}]))expect(reading.contrast).toBeGreaterThanOrEqual(4.5);
  await page.screenshot({path:testInfo.outputPath('kugou-bilingual.png')});
  await page.getByRole('button',{name:'应用双语歌词',exact:true}).click();
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue(expected);
  expect(api.requests.filter(row=>row.path.startsWith('/api/music/jobs/'))).toHaveLength(0);
  expect(api.requests.filter(row=>row.path.endsWith('/lyrics_translate'))).toHaveLength(0);
});

for(const width of [390,1440]) test(`双语歌词支持跨行句段和演唱者标注 ${width}px`,async({page,api},testInfo)=>{
  music(api);await page.setViewportSize({width,height:1000});
  const original='[00:33.25]어느 샌가 나는 숙녀처럼\n[00:36.63]내 입술은 사근사근 그대 이름 부르죠';
  const translated='（泰妍）不知不觉我像淑女一样 我的嘴唇温柔的叫着你的名字';
  api.respond('POST','/api/music/query/inspect',{...track,lyrics:original});
  api.respond('POST','/api/music/query/lyrics_search',{items:[{id:'22735075',source:'netease',title:'Kissing You'}]});
  api.respond('POST','/api/music/query/lyrics_detail',{variants:[{value:'original',title:'原文',text:'[00:33.350]（태연） 어느샌가 나는 숙녀처럼 내 입술은 사근사근 그대 이름 부르죠'},{value:'translation',title:'译文',text:'[00:33.350]'+translated}]});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('button',{name:'搜索替换歌词',exact:true}).click();await page.getByText('Kissing You',{exact:true}).click();
  await page.getByRole('checkbox',{name:'使用译文制作双语歌词',exact:true}).check();
  await expect(page.getByLabel('歌词内容',{exact:true})).toHaveValue(`[00:33.25]어느 샌가 나는 숙녀처럼\n[00:33.25]${translated.split(' ')[0]}\n[00:36.63]내 입술은 사근사근 그대 이름 부르죠\n[00:36.63]${translated.split(' ')[1]}`);
  await expect(page.getByText('保留当前原文和时间轴，已匹配 2/2 句；未匹配的句子保留原样。',{exact:true})).toBeVisible();
  await page.screenshot({path:testInfo.outputPath('bilingual-segments.png')});
  await page.getByRole('button',{name:'应用双语歌词',exact:true}).click();
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue(`[00:33.25]어느 샌가 나는 숙녀처럼\n[00:33.25]${translated.split(' ')[0]}\n[00:36.63]내 입술은 사근사근 그대 이름 부르죠\n[00:36.63]${translated.split(' ')[1]}`);
  expect(api.requests.filter(row=>row.path.startsWith('/api/music/jobs/'))).toHaveLength(0);
});

for(const width of [390,1440]) test(`双语辅助分行仅翻译未确认段落并保留来源译文 ${width}px`,async({page,api},testInfo)=>{
  music(api);await page.setViewportSize({width,height:1000});
  const original='[00:01]얼굴은 빨개지고\n[00:05]놀란 눈은 커다래지고\n[00:10]어쩜 그대 내 맘을\n[00:15]흔들어 놓는지';
  api.respond('POST','/api/music/query/inspect',{...track,lyrics:original});
  api.respond('POST','/api/music/query/lyrics_search',{items:[{id:'22735072',source:'netease',title:'Girls’ Generation'}]});
  api.respond('POST','/api/music/query/lyrics_detail',{variants:[{value:'original',title:'原文',text:'[00:02]얼굴은 빨개지고 놀란 눈은 커다래지고\n[00:20]어쩜 그대 내 맘을 흔들어 놓는지'},{value:'translation',title:'译文',text:'[00:02]脸变红了 惊慌的眼睛变大了\n[00:20]不知怎么你开始动摇我的心'}]});
  api.respond('POST','/api/music/query/lyrics_translate',{line_translations:[{index:2,text:'不知怎么你开始'},{index:3,text:'动摇我的心'}],provider:'youdao'});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('button',{name:'搜索替换歌词',exact:true}).click();await page.getByText('Girls’ Generation',{exact:true}).click();
  await page.getByRole('checkbox',{name:'使用译文制作双语歌词',exact:true}).check();
  expect(api.requests.filter(row=>row.path.endsWith('/lyrics_translate'))).toHaveLength(0);
  await expect(page.getByText('已自动分行 1 个句段，仍有 1 个句段保留整句。',{exact:true})).toBeVisible();
  const align=page.getByRole('button',{name:'翻译未确认段落并对照',exact:true});
  await align.scrollIntoViewIfNeeded();await align.click();
  await expect(page.getByText('已自动分行 2 个句段，仍有 0 个句段保留整句。',{exact:true})).toBeVisible();
  const request=api.requests.find(row=>row.path.endsWith('/lyrics_translate'));
  expect(request.body.line_indices).toEqual([2,3]);
  await expect(page.getByLabel('歌词内容',{exact:true})).toHaveValue('[00:01]얼굴은 빨개지고\n[00:01]脸变红了\n[00:05]놀란 눈은 커다래지고\n[00:05]惊慌的眼睛变大了\n[00:10]어쩜 그대 내 맘을\n[00:10]不知怎么你开始\n[00:15]흔들어 놓는지\n[00:15]动摇我的心');
  await page.getByText('查看机器翻译参考',{exact:true}).click();
  const reference=page.locator('.alignment-reference-row').first();
  await reference.scrollIntoViewIfNeeded();
  const bounds=await reference.evaluate(node=>({width:node.clientWidth,scrollWidth:node.scrollWidth}));expect(bounds.scrollWidth).toBeLessThanOrEqual(bounds.width+1);
  for(const reading of await measureReadingContrast(page,[{role:'机器翻译参考',locator:reference.locator('p').last()}]))expect(reading.contrast).toBeGreaterThanOrEqual(4.5);
  await page.screenshot({path:testInfo.outputPath('bilingual-reference.png')});
  expect(api.requests.filter(row=>row.path.startsWith('/api/music/jobs/'))).toHaveLength(0);
});

for(const source of ['酷狗音乐','网易云音乐 · GD 音乐台','JOOX · GD 音乐台','Bilibili · GD 音乐台','LRCLIB']) test(`歌词来源可搜索专辑并选择曲目 ${source}`,async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/lyrics_search',request=>({items:request.body.album_id ? [{id:'song',source:request.body.source,title:'专辑曲目'}] : request.body.kind==='album' ? [{id:'album',source:request.body.source,title:'来源专辑',music_type:'album'}] : [],has_more:false}));
  api.respond('POST','/api/music/query/lyrics_detail',{variants:[{value:'original',title:'原文',text:'[00:01]选中歌词'}]});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();await page.getByRole('button',{name:'搜索替换歌词',exact:true}).click();
  await page.getByRole('combobox').filter({has:page.getByLabel('歌词来源',{exact:true})}).click();
  await page.getByRole('option',{name:source,exact:true}).click();
  await page.getByText('搜索专辑',{exact:true}).click();await page.getByText('来源专辑',{exact:true}).click();await page.getByText('专辑曲目',{exact:true}).click();
  await expect(page.getByLabel('歌词内容',{exact:true})).toHaveValue('[00:01]选中歌词');
  const albumRequest=api.requests.find(row=>row.path==='/api/music/query/lyrics_search' && row.body.album_id);
  expect(albumRequest.body.query).toBe(track.album);
  expect(api.requests.find(row=>row.path==='/api/music/query/lyrics_detail').body.source).toBe(albumRequest.body.source);
});

for(const id of ['undefined','expired-preview']) test(`刷新清理无效任务编号 ${id}`,async({page,api})=>{
  music(api);
  api.respond('GET','/api/tasks/result/expired-preview',{error:'任务结果不存在'},404);
  await page.addInitScript(id=>{if(!sessionStorage.getItem('seeded')){sessionStorage.setItem('etk.music.task',id);sessionStorage.setItem('seeded','1');}},id);
  await page.goto('/music');
  await expect.poll(()=>page.evaluate(()=>sessionStorage.getItem('etk.music.task'))).toBeNull();
  await expect(page.getByTestId('music-library')).toBeVisible();
  await expect(page.getByText('任务结果不存在',{exact:true})).toHaveCount(0);
  await page.reload();
  await expect(page.getByTestId('music-library')).toBeVisible();
  expect(api.requests.filter(row=>row.path==='/api/tasks/result/expired-preview')).toHaveLength(id==='expired-preview'?1:0);
  expect(api.requests.some(row=>row.path.endsWith('/undefined'))).toBe(false);
});

test('刷新恢复未完成预览，完成后展示结果并清理待恢复编号',async({page,api})=>{
  music(api);
  api.respond('GET','/api/tasks/result/music-preview',{state:'running',current_action:'音乐刮削预览'});
  await page.addInitScript(()=>{if(!sessionStorage.getItem('seeded')){sessionStorage.setItem('etk.music.task','music-preview');sessionStorage.setItem('seeded','1');}});
  await page.goto('/music');
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/tasks/result/music-preview').length).toBeGreaterThan(0);
  await expect.poll(()=>page.evaluate(()=>sessionStorage.getItem('etk.music.task'))).toBe('music-preview');
  await page.reload();
  api.respond('GET','/api/tasks/result/music-preview',{state:'completed',result:{preview:{operation:'edit',items:[previewItem]}}});
  await expect(page.locator('.v-dialog.v-overlay--active').filter({hasText:'音乐修改预览'})).toBeVisible();
  await expect.poll(()=>page.evaluate(()=>sessionStorage.getItem('etk.music.task'))).toBeNull();
  expect(api.requests.filter(row=>row.path==='/api/music/jobs/execute')).toHaveLength(0);
});

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
  await expect(page.getByRole('button',{name:'查看日志',exact:true})).toBeVisible();
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
  await expect(page.getByTestId('music-folder-tracks')).toContainText('搜索结果');
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

test('移动端 MP 音乐筛选可展开收起并保留已选条件', async({page,api})=>{
  music(api);
  await page.setViewportSize({width:390,height:900});
  api.respond('POST','/api/music/query/explore',{items:[]});
  await page.goto('/music');
  await page.getByRole('tab',{name:'探索',exact:true}).click();
  const filters=page.locator('.music-explore-filters');
  const toggle=filters.locator('.app-filter-toggle-pill');
  await expect(toggle).toHaveAttribute('aria-expanded','false');
  await expect.poll(()=>filters.locator('.app-filter-collapsible').evaluate(node=>node.getBoundingClientRect().height)).toBe(0);
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-expanded','true');
  const choice=filters.locator('.app-filter-chips .v-chip').filter({hasText:/^热门单曲$/});
  await choice.click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/explore').at(-1)?.body.entity).toBe('recording');
  const requests=api.requests.filter(row=>row.path==='/api/music/query/explore').length;
  await toggle.click();
  await expect(toggle.locator('.app-filter-toggle-pill__count')).toHaveText('1');
  await toggle.click();
  await expect(choice).toHaveClass(/v-chip--selected/);
  expect(api.requests.filter(row=>row.path==='/api/music/query/explore').length).toBe(requests);
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
});

for(const width of [390,1440]) test(`音乐库目录导航紧凑排列且可以点击 ${width}px`, async({page,api},testInfo)=>{
  await page.setViewportSize({width,height:1000});
  music(api);
  api.respond('GET','/api/music/library',request=>({items:request.query.directory==='歌手/专辑' ? [track] : [{path:request.query.directory ? '歌手/专辑':'歌手',name:request.query.directory ? '专辑':'歌手',type:'dir'}],total:1}));
  await page.goto('/music');
  await expect(page.getByRole('tab',{name:'资料搜索',exact:true})).toHaveCount(0);
  await page.getByTestId('music-library').getByText('歌手',{exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/library').at(-1)?.query.directory).toBe('歌手');
  await page.getByTestId('music-library').getByText('专辑',{exact:true}).click();
  await expect(page.getByTestId('music-library')).toContainText(track.name);
  const crumbs=page.getByRole('navigation',{name:'音乐目录'});
  const labels=await crumbs.locator('.v-btn__content').allTextContents();
  expect(labels.map(value=>value.trim())).toEqual(['音乐库','歌手','专辑']);
  for(const gap of await crumbs.locator('.v-btn').evaluateAll(nodes=>nodes.map(node=>{const button=node.getBoundingClientRect(),label=node.querySelector('.v-btn__content').getBoundingClientRect();return {left:label.left-button.left,right:button.right-label.right,height:button.height};}))){
    expect(gap.right).toBeLessThanOrEqual(8);expect(gap.height).toBeGreaterThanOrEqual(36);
  }
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
  await page.screenshot({path:testInfo.outputPath('compact-breadcrumbs.png')});
  await crumbs.getByRole('button',{name:'歌手',exact:true}).click();
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
  await expect(page.locator('audio')).toHaveAttribute('preload','metadata');
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
    await page.locator('.music-source-chips .v-chip').filter({hasText:label}).click();
    await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/explore').at(-1)?.body.source).toBe(source);
    await expect(page.locator('.music-card')).toContainText('榜单歌曲');
    await expect(page.locator('.music-card')).toContainText(label);
  }
  await page.getByRole('button',{name:'下一页',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/query/explore').at(-1)?.body.page).toBe(2);
  await page.locator('.streaming-chart-chips .v-chip').filter({hasText:'新歌榜'}).click();
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

for (const entry of ['preview', 'editor']) test(`播放前已显示实际时长，可跳转歌词且关闭后停止 ${entry}`,async({page,api},testInfo)=>{
  music(api);
  const wave=Buffer.alloc(44+8000*2*6);
  wave.write('RIFF');wave.writeUInt32LE(wave.length-8,4);wave.write('WAVEfmt ',8);wave.writeUInt32LE(16,16);wave.writeUInt16LE(1,20);wave.writeUInt16LE(1,22);wave.writeUInt32LE(8000,24);wave.writeUInt32LE(16000,28);wave.writeUInt16LE(2,32);wave.writeUInt16LE(16,34);wave.write('data',36);wave.writeUInt32LE(wave.length-44,40);
  let reads=0;
  await page.route('**/api/music/audio?*',route=>{reads++;return route.fulfill({status:200,contentType:'audio/wav',body:wave,headers:{'Accept-Ranges':'bytes'}});});
  await page.goto('/music');
  if (entry === 'preview') await page.getByRole('button',{name:`预览 ${track.name}`,exact:true}).click();
  else await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  const audio=page.locator('audio');
  await expect(audio).toBeVisible();
  await expect.poll(()=>audio.evaluate(node=>node.duration)).toBe(6);
  expect(reads).toBeGreaterThan(0);
  expect(await audio.evaluate(node=>({paused:node.paused,time:node.currentTime}))).toEqual({paused:true,time:0});
  await audio.screenshot({path:testInfo.outputPath('duration-before-play.png')});
  await audio.evaluate(async node=>{window.__testAudio=node;await node.play();});
  await expect.poll(()=>audio.evaluate(node=>node.currentTime)).toBeGreaterThan(0);
  await page.getByRole('button',{name:'跳至 1.00 秒：测试歌词'}).click();
  await expect.poll(()=>audio.evaluate(node=>node.currentTime)).toBeGreaterThanOrEqual(1);
  await page.getByRole('button',{name:entry==='preview'?'关闭':'取消',exact:true}).last().click();
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

test('官方与 GD 资料和歌词入口独立，切换渠道不沿用结果',async({page,api})=>{
  test.setTimeout(60000);
  music(api);
  api.respond('POST','/api/music/query/search',request=>({items:[{media_source:request.body.sources[0],media_id:'song',music_type:'recording',type:'音乐',title:request.body.sources[0]+' 曲名'}]}));
  api.respond('POST','/api/music/query/lyrics_search',request=>({items:[{source:request.body.source,id:'song',title:request.body.source+' 歌词'}]}));
  await page.goto('/music');
  await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  for(const [name,id] of [['网易云音乐','netease'],['网易云音乐 · GD 音乐台','gd_netease']]){
    await page.getByRole('combobox').filter({has:page.getByLabel('资料来源',{exact:true})}).click();
    await page.getByRole('option',{name,exact:true}).click();
    await page.getByRole('button',{name:'搜索资料',exact:true}).click();
    await expect(page.locator('.media-id-selector')).toContainText(id+' 曲名');
    await page.locator('.media-id-selector').getByRole('button',{name:'关闭',exact:true}).click();
  }
  await page.getByRole('button',{name:'搜索替换歌词'}).click();
  await expect(page.locator('.lyrics-results')).toContainText('netease 歌词');
  await page.getByRole('combobox').filter({has:page.getByLabel('歌词来源',{exact:true})}).click();
  await page.getByRole('option',{name:'网易云音乐 · GD 音乐台',exact:true}).click();
  await expect(page.locator('.lyrics-results')).toContainText('gd_netease 歌词');
  await expect(page.locator('.v-chip').filter({hasText:'搜索专辑'})).not.toHaveClass(/v-chip--disabled/);
});

test('本地封面上传先预览，提交保留其他标签',async({page,api})=>{
  music(api);await page.goto('/music');
  const png=await page.evaluate(()=>{const c=document.createElement('canvas');c.width=20;c.height=20;return c.toDataURL('image/png').split(',')[1];});
  await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByLabel('上传本地封面',{exact:true}).setInputFiles({name:'cover.png',mimeType:'image/png',buffer:Buffer.from(png,'base64')});
  await expect(page.getByRole('button',{name:'撤销上传'})).toBeVisible();
  expect(api.requests.filter(row=>row.path.startsWith('/api/music/jobs/'))).toHaveLength(0);
  await page.getByRole('button',{name:'预览修改',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/jobs/preview').length).toBe(1);
  const value=api.requests.find(row=>row.path==='/api/music/jobs/preview').body.metadata;
  expect(value.cover_upload).toMatch(/^data:image\/jpeg;base64,/);
  expect(value.cover_url).toBe('');expect(value.title).toBe(track.title);
});

test('歌词翻译上下显示，取消不更改草稿，双语应用不立即写入',async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/inspect',{...track,lyrics:'[00:01]Hello'});
  api.respond('POST','/api/music/query/lyrics_translate',{translation:'[00:01]你好',bilingual:'[00:01]Hello\n[00:01]你好',provider:'youdao'});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('button',{name:'机器翻译',exact:true}).click();
  await expect(page.getByText('百度翻译',{exact:true})).toHaveCount(0);
  await page.getByRole('button',{name:'开始翻译',exact:true}).click();
  await expect(page.getByLabel('歌词译文',{exact:true})).toContainText('你好');
  const original=await page.getByLabel('歌词原文',{exact:true}).boundingBox(),translated=await page.getByLabel('歌词译文',{exact:true}).boundingBox();
  expect(translated.y).toBeGreaterThan(original.y+original.height);
  await page.getByRole('button',{name:'取消',exact:true}).click();
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue('[00:01]Hello');
  await page.getByRole('button',{name:'机器翻译',exact:true}).click();await page.getByRole('button',{name:'开始翻译',exact:true}).click();
  await page.getByRole('button',{name:'应用双语歌词',exact:true}).click();
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue('[00:01]Hello\n[00:01]你好');
  expect(api.requests.filter(row=>row.path.startsWith('/api/music/jobs/'))).toHaveLength(0);
});

for(const theme of ['light','dark']) for(const width of [390,1440]) test(`中韩歌词只翻译选中段落且选择行完整可读 ${theme} ${width}px`,async({page,api},testInfo)=>{
  music(api);await page.setViewportSize({width,height:1000});
  await page.addInitScript(theme=>localStorage.setItem('moviepilot-theme-customizer',JSON.stringify({theme,glassEnabled:false})),theme);
  const original='[ar:成龙 / 金喜善]\n[00:01]梦中人 熟悉的脸孔\n[00:10]나의 손을 잡아줘\n[00:20]나의 손을 잡아줘\n[00:30]万世沧桑 唯有爱是永远的神话';
  api.respond('POST','/api/music/query/inspect',{...track,lyrics:original});
  api.respond('POST','/api/music/query/lyrics_translate',{translation:original.replace('[00:10]나의 손을 잡아줘','[00:10]请握住我的手'),bilingual:original.replace('\n[00:20]','\n[00:10]请握住我的手\n[00:20]'),provider:'youdao'});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('button',{name:'机器翻译',exact:true}).click();
  const first=page.getByRole('checkbox',{name:'翻译第 3 行：[00:10]나의 손을 잡아줘',exact:true});
  const second=page.getByRole('checkbox',{name:'翻译第 4 行：[00:20]나의 손을 잡아줘',exact:true});
  await expect(first).toBeChecked();await expect(second).toBeChecked();
  await expect(page.getByRole('checkbox',{name:'翻译第 2 行：[00:01]梦中人 熟悉的脸孔',exact:true})).not.toBeChecked();
  await second.uncheck();await page.getByRole('button',{name:'开始翻译',exact:true}).click();
  await expect(page.getByLabel('歌词译文',{exact:true})).toContainText('请握住我的手');
  expect(api.requests.find(row=>row.path.endsWith('/lyrics_translate')).body.line_indices).toEqual([2]);
  const label=page.locator('.translation-original label').last();await label.scrollIntoViewIfNeeded();
  const bounds=await label.evaluate(node=>({width:node.clientWidth,scrollWidth:node.scrollWidth,height:node.clientHeight,scrollHeight:node.scrollHeight}));
  expect(bounds.scrollWidth).toBeLessThanOrEqual(bounds.width+1);expect(bounds.scrollHeight).toBeLessThanOrEqual(bounds.height+1);
  for(const reading of await measureReadingContrast(page,[{role:'歌词选择标签',locator:label}]))expect(reading.contrast).toBeGreaterThanOrEqual(4.5);
  await page.screenshot({path:testInfo.outputPath('translation-selection.png')});
  await second.check();await expect(page.getByRole('button',{name:'应用双语歌词',exact:true})).toBeDisabled();
  await second.uncheck();await page.getByRole('button',{name:'开始翻译',exact:true}).click();
  await page.getByRole('button',{name:'应用双语歌词',exact:true}).click();
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue(original.replace('\n[00:20]','\n[00:10]请握住我的手\n[00:20]'));
  expect(api.requests.filter(row=>row.path.startsWith('/api/music/jobs/'))).toHaveLength(0);
});

test('时间轴逐行编辑与格式化保留内容，具体发行版传入读取资料',async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/detail',request=>({media_source:'musicbrainz',media_id:request.body.id,title:'专辑',musicbrainz_release_id:request.body.release || '11111111-1111-1111-1111-111111111111',tracks:[],releases:[{media_id:'11111111-1111-1111-1111-111111111111',title:'测试发行'},{media_id:'22222222-2222-2222-2222-222222222222',title:'特别版',date:'2024-01-02',country:'JP',formats:['CD'],track_count:12}]}));api.respond('POST','/api/music/query/inspect',{...track,lyrics:'[0:1.5]第一句\n[00:02]第二句'});
  api.respond('POST','/api/music/query/edit_metadata',{title:track.title});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('button',{name:'格式化时间',exact:true}).click();
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue('[00:01.500]第一句\n[00:02.000]第二句');
  await page.getByRole('button',{name:'时间轴编辑器',exact:true}).click();
  await page.getByLabel('第 2 行时间',{exact:true}).fill('00:03.500');
  await page.getByRole('button',{name:'应用时间轴',exact:true}).click();
  await expect(page.getByLabel('旁挂歌词（LRC / 纯文本）',{exact:true})).toHaveValue('[00:01.500]第一句\n[00:03.500]第二句');
  await page.getByRole('combobox').filter({has:page.getByLabel('音乐类型',{exact:true})}).click();await page.getByRole('option',{name:'专辑',exact:true}).click();
  await page.getByLabel('MusicBrainz ID',{exact:true}).fill('11111111-1111-1111-1111-111111111111');
  const release=page.getByRole('combobox').filter({has:page.getByLabel('具体发行版本',{exact:true})});
  await expect(release).toContainText('测试发行');
  await release.click();
  await page.getByRole('option',{name:'特别版 · 2024-01-02 · JP · CD · 12 首曲目',exact:true}).click();
  await expect(release).toContainText('特别版');
  await page.getByRole('button',{name:'读取资料',exact:true}).click();
  await expect.poll(()=>api.requests.find(row=>row.path==='/api/music/query/edit_metadata')?.body.selection.musicbrainz_release_id).toBe('22222222-2222-2222-2222-222222222222');
  await page.getByLabel('MusicBrainz ID',{exact:true}).fill('33333333-3333-3333-3333-333333333333');
  await expect(release).toContainText('测试发行');
  await expect(release).not.toContainText('特别版');
});

for(const width of [390,1440]) test(`音乐分页按 MP 左右排列且固定可见，目录导航隐藏磁盘路径 ${width}px`,async({page,api},testInfo)=>{
  music(api);await page.setViewportSize({width,height:1000});
  api.respond('GET','/api/music/library',{library_path:'/media/Media/Link/Music',items:[track],tracks:Array.from({length:60},(_,i)=>({...track,path:`album/${i}.flac`,title:`第 ${i+1} 首歌曲`})),total:1000});
  await page.goto('/music');
  const footer=page.getByRole('contentinfo',{name:'音乐库分页'});
  await expect(footer).toBeVisible();
  expect(await footer.evaluate(node=>getComputedStyle(node).backgroundImage)).toBe('none');
  expect(await footer.evaluate(node=>getComputedStyle(node).backdropFilter)).toBe('none');
  const start=await footer.boundingBox();expect(start.y+start.height).toBeLessThanOrEqual(1001);expect(start.y).toBeGreaterThan(0);
  expect(await page.getByTestId('music-library').locator('nav[aria-label="音乐目录"]').count()).toBe(0);
  await expect(page.getByTestId('music-library-path')).not.toContainText('/media/Media/Link/Music');
  const sizeBox=await footer.getByRole('combobox').boundingBox(),pagesBox=await footer.locator('.v-pagination').boundingBox();
  expect(sizeBox.x-start.x).toBeLessThan(20);
  expect(start.x+start.width-pagesBox.x-pagesBox.width).toBeLessThan(20);
  if(width>960) expect(Math.abs(sizeBox.y-pagesBox.y)).toBeLessThan(6);
  await page.evaluate(()=>window.scrollTo(0,document.documentElement.scrollHeight));
  const end=await footer.boundingBox();expect(Math.abs(end.y-start.y)).toBeLessThan(2);
  await footer.getByRole('button',{name:'下一页',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/music/library').at(-1)?.query.page).toBe('2');
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
  await page.screenshot({path:testInfo.outputPath('fixed-pagination.png')});
});

test('点击搜索歌词从对应时间播放本地歌曲，首次加载也能跳转',async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/lyrics_search',{items:[{source:'netease',id:'remote-song',title:'候选歌词'}]});
  api.respond('POST','/api/music/query/lyrics_detail',{variants:[{value:'original',title:'原文',text:'[00:02.00]点击这一句'}]});
  const wav=Buffer.alloc(44+8000*6*2);wav.write('RIFF');wav.writeUInt32LE(wav.length-8,4);wav.write('WAVEfmt ',8);wav.writeUInt32LE(16,16);wav.writeUInt16LE(1,20);wav.writeUInt16LE(1,22);wav.writeUInt32LE(8000,24);wav.writeUInt32LE(16000,28);wav.writeUInt16LE(2,32);wav.writeUInt16LE(16,34);wav.write('data',36);wav.writeUInt32LE(wav.length-44,40);
  const audioRequests=[];
  await page.route('**/api/music/audio?*',route=>{audioRequests.push(new URL(route.request().url()).searchParams.get('path'));return route.fulfill({status:200,contentType:'audio/wav',body:wav});});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();await page.getByRole('button',{name:'搜索替换歌词'}).click();await page.getByText('候选歌词',{exact:true}).click();
  await page.getByRole('button',{name:'跳至 2.00 秒：点击这一句',exact:true}).click();
  const player=page.getByLabel('本地歌曲试听',{exact:true});
  await expect.poll(()=>player.evaluate(node=>!node.paused && node.currentTime>=2)).toBe(true);
  expect(audioRequests.length).toBeGreaterThan(0);expect(audioRequests.every(path=>path===track.path)).toBe(true);
});

test('时间轴试听只播放所选歌词区间，同时间译文不截断原句',async({page,api})=>{
  music(api);
  api.respond('POST','/api/music/query/inspect',{...track,lyrics:'[00:01]原句\n[00:01]译文\n[00:02]下一句\n[00:04]最后一句'});
  const wav=Buffer.alloc(44+8000*6*2);wav.write('RIFF');wav.writeUInt32LE(wav.length-8,4);wav.write('WAVEfmt ',8);wav.writeUInt32LE(16,16);wav.writeUInt16LE(1,20);wav.writeUInt16LE(1,22);wav.writeUInt32LE(8000,24);wav.writeUInt32LE(16000,28);wav.writeUInt16LE(2,32);wav.writeUInt16LE(16,34);wav.write('data',36);wav.writeUInt32LE(wav.length-44,40);
  await page.route('**/api/music/audio?*',route=>{
    const match=route.request().headers().range?.match(/bytes=(\d+)-(\d*)/);
    const start=match?Number(match[1]):0,end=match?.[2]?Number(match[2]):wav.length-1;
    return route.fulfill({status:match?206:200,contentType:'audio/wav',headers:{'Accept-Ranges':'bytes',...(match?{'Content-Range':`bytes ${start}-${end}/${wav.length}`}:{})},body:wav.subarray(start,end+1)});
  });
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();await page.getByRole('button',{name:'时间轴编辑器',exact:true}).click();
  const player=page.getByLabel('本地歌曲试听',{exact:true});
  await page.getByRole('button',{name:'试听第 1 行',exact:true}).click();
  await expect.poll(()=>player.evaluate(node=>!node.paused && node.currentTime>=1 && node.currentTime<2)).toBe(true);
  await expect.poll(()=>player.evaluate(node=>({paused:node.paused,time:Math.round(node.currentTime*100)/100}))).toEqual({paused:true,time:2});
  await page.getByRole('button',{name:'试听第 2 行',exact:true}).click();
  await expect.poll(()=>player.evaluate(node=>!node.paused && node.currentTime>=1 && node.currentTime<2)).toBe(true);
  await expect.poll(()=>player.evaluate(node=>({paused:node.paused,time:Math.round(node.currentTime*100)/100}))).toEqual({paused:true,time:2});
  await page.getByRole('button',{name:'试听第 4 行',exact:true}).click();
  await expect.poll(()=>player.evaluate(node=>!node.paused && node.currentTime>=4)).toBe(true);
  await expect.poll(()=>player.evaluate(node=>node.ended)).toBe(true);
});

for(const theme of ['light','dark'])for(const width of [390,1440])test(`音乐新工具与固定分页可读 ${theme} ${width}px`,async({page,api},testInfo)=>{
  music(api);await page.setViewportSize({width,height:1000});
  await page.addInitScript(theme=>localStorage.setItem('moviepilot-theme-customizer',JSON.stringify({theme,glassEnabled:true,glassAppearance:'frosted',glassQuality:'css'})),theme);
  api.respond('POST','/api/music/query/inspect',{...track,lyrics:'[00:01]Into The New World — long line for checking readable lyrics and controls'});
  api.respond('POST','/api/music/query/lyrics_translate',{translation:'[00:01]再次重逢的世界 — 检查歌词显示及按钮是否完整可读',bilingual:'[00:01]Into The New World\n[00:01]再次重逢的世界',provider:'youdao'});
  await page.goto('/music');
  const footer=page.getByRole('contentinfo',{name:'音乐库分页'});
  const current=footer.locator('.v-pagination__item--is-active .v-btn__content');
  await expect(current).toBeVisible();
  for(const reading of await measureReadingContrast(page,[{role:'当前页码',locator:current}]))expect(reading.contrast).toBeGreaterThanOrEqual(4.5);
  await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();await page.getByRole('button',{name:'机器翻译',exact:true}).click();await page.getByRole('button',{name:'开始翻译',exact:true}).click();
  await expect(page.getByLabel('歌词译文',{exact:true})).toBeVisible();
  const source=page.locator('.lyrics-translation .v-chip--selected .v-chip__content');
  for(const reading of await measureReadingContrast(page,[{role:'翻译服务',locator:source},{role:'译文',locator:page.getByLabel('歌词译文',{exact:true})}]))expect(reading.contrast).toBeGreaterThanOrEqual(4.5);
  for(const element of [page.getByLabel('歌词译文',{exact:true}),page.getByRole('button',{name:'应用双语歌词',exact:true}),source]){
    const size=await element.evaluate(node=>({width:node.clientWidth,scrollWidth:node.scrollWidth,height:node.clientHeight,scrollHeight:node.scrollHeight}));expect(size.scrollWidth).toBeLessThanOrEqual(size.width+1);expect(size.scrollHeight).toBeLessThanOrEqual(size.height+1);
  }
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();await page.screenshot({path:testInfo.outputPath('translation-readable.png')});
  await page.getByRole('button',{name:'取消',exact:true}).click();
  await page.getByRole('button',{name:'时间轴编辑器',exact:true}).click();
  for(const control of [page.getByLabel('第 1 行时间',{exact:true}),page.getByLabel('第 1 行歌词',{exact:true}),page.getByRole('button',{name:'第 1 行使用当前时间',exact:true}),page.getByRole('button',{name:'应用时间轴',exact:true})]){
    const box=await control.boundingBox();expect(box.x).toBeGreaterThanOrEqual(0);expect(box.x+box.width).toBeLessThanOrEqual(width+1);
  }
  await page.screenshot({path:testInfo.outputPath('timeline-readable.png')});
});


for(const width of [390,1440]) test(`重新刮削曲目选项与 CUE 可读且传入预览 ${width}px`,async({page,api},testInfo)=>{
  music(api);await page.setViewportSize({width,height:1000});
  api.respond('POST','/api/music/query/selection',{paths:[track.path],collection_root:'专辑'});
  await page.goto('/music');await page.getByRole('button',{name:'选择',exact:true}).click();await page.getByRole('checkbox',{name:`选择 ${track.name}`}).check();await page.getByRole('button',{name:'批量重新刮削',exact:true}).click();
  await page.getByRole('combobox').filter({has:page.getByLabel('曲目对齐方式',{exact:true})}).click();
  for(const text of ['严格校验','忽略时长','跳过对齐验证'])await expect(page.getByRole('option',{name:text,exact:true})).toBeVisible();
  await page.getByRole('option',{name:'忽略时长',exact:true}).click();
  await page.getByRole('checkbox',{name:'识别 CUE',exact:true}).uncheck();
  await page.screenshot({path:testInfo.outputPath('rescrape-options.png')});
  await page.getByRole('button',{name:'预览',exact:true}).click();
  const body=api.requests.find(row=>row.path==='/api/music/jobs/preview').body;
  expect(body.music_type).toBe('album');expect(body.music_track_match).toBe('ignore_duration');expect(body.music_cue_enable).toBe(false);
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
});

test('艺术家合集隐藏统一专辑选择并保留目录范围',async({page,api})=>{
  music(api);api.respond('POST','/api/music/query/selection',{paths:[track.path],collection_root:'歌手'});
  await page.goto('/music');await page.getByRole('button',{name:'选择',exact:true}).click();await page.getByRole('checkbox',{name:`选择 ${track.name}`}).check();await page.getByRole('button',{name:'批量重新刮削',exact:true}).click();
  await page.getByRole('combobox').filter({has:page.getByLabel('音乐实体',{exact:true})}).click();await page.getByRole('option',{name:'艺术家合集',exact:true}).click();
  await expect(page.getByRole('button',{name:'选择音乐资料',exact:true})).toHaveCount(0);
  await page.getByRole('button',{name:'预览',exact:true}).click();
  const body=api.requests.find(row=>row.path==='/api/music/jobs/preview').body;
  expect(body.music_type).toBe('artist_collection');expect(body.collection_root).toBe('歌手');expect(body.selection).toBeNull();
});

for(const width of [390,1440]) test(`标签编辑的对齐与 CUE 贯穿读取预览保存 ${width}px`,async({page,api},testInfo)=>{
  music(api);await page.setViewportSize({width,height:1000});
  const group='11111111-1111-1111-1111-111111111111';
  api.respond('POST','/api/music/query/inspect',{...track,music_cue_enable:false});
  api.respond('POST','/api/music/query/detail',{media_source:'musicbrainz',media_id:group,title:'专辑',musicbrainz_release_id:'22222222-2222-2222-2222-222222222222',releases:[{media_id:'22222222-2222-2222-2222-222222222222',title:'发行版'}]});
  api.respond('POST','/api/music/query/edit_metadata',{title:'识别曲名'});
  api.respond('GET','/api/tasks/result/music-preview',{state:'completed',result:{preview:{operation:'edit',items:[previewItem]}}});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await expect(page.getByRole('checkbox',{name:'识别 CUE',exact:true})).not.toBeChecked();
  await page.getByRole('combobox').filter({has:page.getByLabel('音乐类型',{exact:true})}).click();
  await expect(page.getByRole('option',{name:'艺术家合集',exact:true})).toHaveCount(0);
  await page.getByRole('option',{name:'专辑',exact:true}).click();
  await page.getByRole('combobox').filter({has:page.getByLabel('曲目对齐方式',{exact:true})}).click();
  for(const name of ['严格校验','忽略时长','跳过对齐验证'])await expect(page.getByRole('option',{name,exact:true})).toBeVisible();
  await page.getByRole('option',{name:'忽略时长',exact:true}).click();
  await page.getByLabel('MusicBrainz ID',{exact:true}).fill(group);
  await page.getByRole('button',{name:'读取资料',exact:true}).click();
  await expect(page.getByLabel('标题',{exact:true})).toHaveValue('识别曲名');
  const read=api.requests.find(row=>row.path==='/api/music/query/edit_metadata').body;
  expect(read.music_track_match).toBe('ignore_duration');expect(read.music_cue_enable).toBe(false);
  await page.getByLabel('备注',{exact:true}).fill('核对后修改');
  await page.getByRole('button',{name:'预览修改',exact:true}).click();
  await expect(page.getByRole('button',{name:'保存修改',exact:true})).toBeEnabled();
  const body=api.requests.filter(row=>row.path==='/api/music/jobs/preview').at(-1).body;
  expect(body.selection).toEqual(read.selection);expect(body.music_track_match).toBe('ignore_duration');expect(body.music_cue_enable).toBe(false);expect(body.metadata.title).toBe('识别曲名');expect(body.metadata.comment).toBe('核对后修改');
  expect(api.requests.filter(row=>row.path==='/api/music/jobs/execute')).toHaveLength(0);
  await page.getByRole('button',{name:'返回编辑',exact:true}).click();
  await expect(page.getByRole('combobox').filter({has:page.getByLabel('曲目对齐方式',{exact:true})})).toContainText('忽略时长');
  await expect(page.getByRole('checkbox',{name:'识别 CUE',exact:true})).not.toBeChecked();
  await page.getByRole('checkbox',{name:'识别 CUE',exact:true}).scrollIntoViewIfNeeded();
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
  await page.screenshot({path:testInfo.outputPath('editor-processing-options.png')});
  await page.getByRole('button',{name:'预览修改',exact:true}).click();await page.getByRole('button',{name:'保存修改',exact:true}).click();
  expect(api.requests.find(row=>row.path==='/api/music/jobs/execute').body).toEqual({preview_task_id:'music-preview'});
});


test('专辑搜索默认携带当前歌曲发行年份并在重开后重置',async({page,api})=>{
  music(api);api.respond('POST','/api/music/query/inspect',{...track,album:'GIRLS’ GENERATION II ～Girls & Peace～',release_year:2012});
  await page.goto('/music');await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('combobox').filter({has:page.getByLabel('音乐类型',{exact:true})}).click();await page.getByRole('option',{name:'专辑',exact:true}).click();
  const query=page.getByLabel('搜索音乐资料',{exact:true}).and(page.locator('input'));
  await expect(query).toHaveValue('GIRLS’ GENERATION II ～Girls & Peace～ (2012)');
  await query.fill('修改的搜索词');await page.getByRole('button',{name:'取消',exact:true}).click();
  await page.getByTestId('music-library').getByText(track.name,{exact:true}).click();
  await page.getByRole('combobox').filter({has:page.getByLabel('音乐类型',{exact:true})}).click();await page.getByRole('option',{name:'专辑',exact:true}).click();
  await expect(page.getByLabel('搜索音乐资料',{exact:true}).and(page.locator('input'))).toHaveValue('GIRLS’ GENERATION II ～Girls & Peace～ (2012)');
});
