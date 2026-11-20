<!-- Music layouts and controls adapted from MoviePilot-Frontend v3 db17d9c (MIT). -->
<script setup>
import { computed, onMounted, onBeforeUnmount, ref, watch } from 'vue';
import axios from 'axios';
import { useDisplay } from 'vuetify';
import { useRouter } from 'vue-router';
import { useTaskStatus } from '@/composables/useTaskStatus';
import DialogCloseBtn from '@/components/theme/DialogCloseBtn.vue';
import MusicCard from './MusicCard.vue';
import MusicFileBrowser from './MusicFileBrowser.vue';
import MusicLibraryPath from './MusicLibraryPath.vue';
import MusicFolderTracks from './MusicFolderTracks.vue';
import MusicTagEditor from './MusicTagEditor.vue';
import MusicSongPreview from './MusicSongPreview.vue';
import MusicArtistCard from './MusicArtistCard.vue';
import MusicDoubanFilters from './MusicDoubanFilters.vue';
import MusicTrackList from './MusicTrackList.vue';
import MusicDetailLayout from './MusicDetailLayout.vue';
import MusicPreview from './MusicPreview.vue';
import MusicCorrectionDialog from './MusicCorrectionDialog.vue';
import MusicIdSelector from './MusicIdSelector.vue';
import MusicExploreFilters from './MusicExploreFilters.vue';
import MusicRecognitionCachePanel from './MusicRecognitionCachePanel.vue';
import { groupMusicPreview } from './previewState';
import { musicSources, streamingSources, sourceLabel, formatMusicDuration, musicReleaseError } from './utils';

const router = useRouter(), { smAndDown } = useDisplay();
const { backgroundTaskStatus, refreshTaskStatus } = useTaskStatus();
const libraryRoot = ref('');
const directory = ref(''), exploreMore = ref(false), folderTracks = ref([]);
const tab = ref('library'), query = ref(''), items = ref([]), total = ref(0), page = ref(1), pageSize = ref(50);
const loading = ref(false), error = ref(''), selected = ref([]);
const exploreSource = ref('musicbrainz'), doubanParams=ref({tags:'流行',douban_sort:'U',with_cover:false});
const streamParams = ref({chart:'',region:'cn'}), charts = ref([]), songPreview = ref(null), editorFocus = ref('');
const isStreaming = computed(()=>streamingSources.some(item=>item.value===exploreSource.value));
const detailAlbum=ref(null);
const detailOpen = ref(false), detail = ref(null), detailKind = ref('recording'), detailError = ref(''), detailBusy = ref(false), related = ref([]), editions = ref([]), release = ref('');
const editorOpen = ref(false), editor = ref(null), editorError = ref('');
const actionOpen = ref(false), action = ref('rescrape'), actionPaths = ref([]), selection = ref(null), selectorOpen = ref(false);
const resultOpen=ref(false), resultData=ref(null);
const previewOpen = ref(false), preview = ref(null), previewTask = ref(''), taskId = ref(''), taskState = ref(null), taskError = ref('');
const exploreParams=ref({mode:'chart',entity:'album',range_name:'this_month'}), artistType=ref('album'), relatedPage=ref(1), relatedBusy=ref(false), relatedArtists=ref([]), relatedLabel=ref('相关专辑');
const selectedCache=ref([]);
const cacheConfirm = ref(false), submitting = ref(false), corrections = ref({}), correctionGroup = ref(null), selectorSource = ref('musicbrainz');
const previewGroups = computed(() => groupMusicPreview(preview.value?.items || []));
let request = 0, detailRequest = 0, relatedRequest = 0, editorRequest = 0, polling, active = true;
const busy = computed(() => Boolean(taskId.value || submitting.value));
const pages = computed(() => Math.max(1, Math.ceil(total.value / pageSize.value)));
const actionLabel = computed(() => ({ rescrape: '重新刮削', edit: '保存标签' })[action.value]);
const selectedReleaseError=computed(()=>musicReleaseError(selection.value));
const canExecute = computed(() => preview.value?.items?.length > 0 && preview.value.items.every(item => item.success) && !busy.value);
const detailFacts=computed(()=>{const d=detail.value || {};return [['类型',d.artist_type || d.metadata_category || d.album_type],['专辑',d.album],['专辑艺术家',d.album_artist],['发行日期',d.release_date || d.year],['曲目数',d.total_tracks],['碟数',d.total_discs],['时长',formatMusicDuration(d.duration)],['曲序',d.track_number],['ISRC',d.isrc],['播放次数',d.listen_count],['评分',d.rating],['评分人数',d.rating_votes],['地区',d.area || d.country],['活动时间',d.life_span],['性别',d.gender],['别名',d.aliases?.slice(0,6).join('、')],[`${sourceLabel(d.media_source)} ID`,d.media_id]].filter(([,value])=>value);});
const failure = cause => cause.response?.data?.message || cause.message || '音乐操作失败';
const api = async (operation, body = {}) => (await axios.post(`/api/music/query/${operation}`, body, { timeout: 190000 })).data;

async function load() {
  const version = ++request;
  loading.value = true; error.value = '';
  try {
    let result;
    if (tab.value === 'library') result = (await axios.get('/api/music/library', { params: { directory:directory.value, query: query.value, page: page.value, page_size: pageSize.value } })).data;
    else if (tab.value === 'explore') result = await api('explore', { ...(isStreaming.value ? streamParams.value : exploreSource.value==='musicbrainz' ? exploreParams.value : doubanParams.value), source:exploreSource.value, page: page.value, count:30 });
    else result = await api('cache');
    if (!active || version !== request) return;
    items.value = result.items || []; total.value = result.total ?? items.value.length; exploreMore.value = result.has_more ?? items.value.length >= 30;
    if(tab.value==='explore' && isStreaming.value){charts.value=result.charts || [];streamParams.value.chart=result.chart || '';}
    if(tab.value==='library'){folderTracks.value=result.tracks || items.value.filter(item=>item.type!=='dir' && item.is_audio!==false);libraryRoot.value=result.library_path || libraryRoot.value;}
    selected.value = [];
    if (Object.keys(result.errors || {}).length) error.value = Object.entries(result.errors).map(([source, text]) => `${sourceLabel(source)}：${text}`).join('；');
  } catch (cause) { if (active && version === request) { error.value = failure(cause); items.value = []; folderTracks.value=[]; total.value = 0; } }
  finally { if (active && version === request) loading.value = false; }
}
function search() { page.value = 1; load(); }
function exploreChanged(params) {exploreParams.value=params; search();}
function doubanChanged(params) {doubanParams.value=params; search();}
function navigateDirectory(path) { directory.value=path; query.value=''; page.value=1; load(); }
async function openAction(operation, paths = selected.value) {
  if (busy.value) return;
  submitting.value = true; taskError.value = '';
  try {
    const result = await api('selection',{paths});
    if (!active) return;
    action.value = operation; actionPaths.value = result.paths; selection.value = null; corrections.value = {}; actionOpen.value = true;
  } catch(cause) { if(active) taskError.value=failure(cause); }
  finally { if(active) submitting.value=false; }
}
async function openEditor(item) {
  const version = ++editorRequest;
  songPreview.value = null; editorFocus.value = item.editField || '';
  editorOpen.value = true; editor.value = null; editorError.value = '';
  try { const value = await api('inspect', { path: item.path }); if (active && version === editorRequest) editor.value = value; }
  catch (cause) { if (active && version === editorRequest) editorError.value = failure(cause); }
}
async function openDetail(item, selectedRelease) {
  const version = ++detailRequest;
  relatedRequest++; relatedBusy.value = false;
  detailOpen.value = true; detail.value = null; detailAlbum.value=null; detailBusy.value = true; detailError.value = ''; related.value = []; relatedArtists.value=[]; relatedPage.value=1; artistType.value='album';
  const source = item.media_source, id = item.media_id, type = item.music_type || 'recording';
  if(streamingSources.some(entry=>entry.value===source)){
    detail.value=item;detailKind.value='recording';editions.value=[];release.value='';detailBusy.value=false;return;
  }
  try {
    const result = await api('detail', { source, id, kind: type, release: selectedRelease });
    if (!active || version !== detailRequest) return;
    if (!result) throw new Error('没有找到音乐详情');
    detail.value = { ...result, music_type: type }; detailKind.value = type;
    editions.value = result.releases?.length ? result.releases : selectedRelease ? editions.value : [];
    release.value = result.musicbrainz_release_id || '';
    if(type==='recording' && result.album_id){const album=await api('detail',{source,id:result.album_id,kind:'album'}); if(active && version===detailRequest) detailAlbum.value=album;}
    const artistId = type==='artist' ? id : ['musicbrainz','theaudiodb'].includes(source) ? result.artist_ids?.find(Boolean) : undefined;
    relatedLabel.value = artistId ? '艺术家作品' : '相关专辑';
    if (artistId || type === 'album') {
      const relatedVersion = ++relatedRequest;
      const more = await api('related', { source, id:artistId || id, kind:artistId ? 'artist_albums' : 'album', album_type:type==='artist' ? artistType.value : undefined });
      if (active && version === detailRequest && relatedVersion === relatedRequest) related.value = more.items || [];
      if(artistId && source==='musicbrainz'){const extra=await api('related',{source,id:artistId,kind:'artist'}); if(active && version===detailRequest) relatedArtists.value=extra.items||[];}
    }
  } catch (cause) { if (active && version === detailRequest) detailError.value = failure(cause); }
  finally { if (active && version === detailRequest) detailBusy.value = false; }
}
async function loadArtistAlbums(reset=false) {
  if(reset) relatedPage.value=1;
  const version=detailRequest, requestId=++relatedRequest, artist=detail.value;
  relatedBusy.value=true;
  try { const result=await api('related',{source:artist.media_source,id:artist.media_id,kind:'artist_albums',page:relatedPage.value,album_type:artistType.value}); if(active && version===detailRequest && requestId===relatedRequest) related.value=result.items||[]; }
  catch(cause){if(active && version===detailRequest && requestId===relatedRequest) detailError.value=failure(cause);}
  finally{if(version===detailRequest && requestId===relatedRequest)relatedBusy.value=false;}
}
function chooseMusic(item) { selection.value = {media_source: selectorSource.value, media_id:item.id, music_type:item.music_type || 'album'}; selectorOpen.value = false; }
function correctGroup(chosen) {
  const directory = correctionGroup.value.items[0].music.group_directory;
  corrections.value = {...corrections.value, [directory]: {...chosen, music_type:'album'}};
  correctionGroup.value = null;
  submit('preview', {operation:preview.value.operation, paths:preview.value.items.map(row=>row.path), selection:selection.value, selections:corrections.value, previous_preview_task_id:previewTask.value});
}
async function submit(operation, body) {
  if (busy.value) return;
  submitting.value = true; taskError.value = ''; resultData.value=null;
  try {
    const { data } = await axios.post(`/api/music/jobs/${operation}`, body);
    taskId.value = data.task_id; sessionStorage.setItem('etk.music.task', data.task_id); taskState.value = { current_action: operation === 'scan' ? '扫描本地音乐库' : '处理所选音乐', state: 'running' };
    actionOpen.value = false; editorOpen.value = false; previewOpen.value = false;
    refreshTaskStatus(); poll();
  } catch (cause) { taskError.value = failure(cause); }
  finally { submitting.value = false; }
}
async function poll() {
  clearTimeout(polling);
  if (!active || !taskId.value) return;
  const id = taskId.value;
  try {
    const { data } = await axios.get(`/api/tasks/result/${id}`);
    if (!active || id !== taskId.value) return;
    taskState.value = data;
    if (['completed', 'completed_with_warnings', 'failed', 'cancelled'].includes(data.state)) {
      taskId.value = '';
      if (data.result?.preview) { preview.value = data.result.preview; previewTask.value = id; corrections.value=preview.value.selections || {}; selection.value=preview.value.selection || null; previewOpen.value = true; }
      else { resultData.value=data.result; sessionStorage.removeItem('etk.music.task'); if (data.state === 'failed') taskError.value = data.result?.error || data.message || '音乐任务失败'; await load(); }
      return;
    }
  } catch (cause) { if (active) {taskError.value = failure(cause); if(cause.response?.status===404){taskId.value='';sessionStorage.removeItem('etk.music.task');}} }
  if (active && taskId.value) polling = setTimeout(poll, 1500);
}
function previewAction() { if(selectedReleaseError.value){taskError.value=selectedReleaseError.value;return;} if(selection.value?.musicbrainz_release_id)selection.value.musicbrainz_release_id=selection.value.musicbrainz_release_id.trim().toLowerCase(); submit('preview', { operation: action.value, paths: actionPaths.value, selection: selection.value, selections:corrections.value }); }
function saveEditor(metadata) { action.value = 'edit'; submit('preview', { operation: 'edit', paths: [metadata.path], metadata }); }
async function clearSelectedCache() {try{await api('cache',{keys:selectedCache.value});selectedCache.value=[];load();}catch(cause){error.value=failure(cause);}}
async function clearCache(key) {
  try { await api('cache', key ? { key } : { clear: true }); cacheConfirm.value = false; load(); }
  catch (cause) { error.value = failure(cause); }
}
watch(exploreSource, () => {exploreParams.value={mode:'chart',entity:'album',range_name:'this_month'};doubanParams.value={tags:'流行',douban_sort:'U',with_cover:false};streamParams.value={chart:'',region:'cn'};charts.value=[]; if(tab.value==='explore')search();});
watch(tab, () => { query.value = ''; page.value = 1; selected.value = []; selectedCache.value=[]; load(); });
watch(backgroundTaskStatus, status => {
  if (!taskId.value && status.is_running && /音乐/.test(status.current_action || '')) { taskId.value = status.task_id; poll(); }
});
onMounted(() => { load(); const pending = sessionStorage.getItem('etk.music.task'); if (pending) {taskId.value=pending; poll();} });
onBeforeUnmount(() => { active = false; request++; detailRequest++; editorRequest++; clearTimeout(polling); });
</script>

<template>
  <div class="mp-page music-page-shell">
    <div class="music-toolbar"><VTabs v-model="tab" color="primary" show-arrows><VTab value="library">音乐库</VTab><VTab value="explore">探索</VTab><VTab value="cache">识别缓存</VTab></VTabs><VSpacer /><VBtn icon="mdi-cog-outline" variant="text" aria-label="音乐设置" @click="router.push('/settings/general?tab=music')" /></div>
    <div v-if="tab !== 'explore'" class="music-toolbar my-4">
      <VTextField :mobile-layout="false" v-if="tab !== 'cache'" v-model="query" :label="tab === 'library' ? '搜索音乐库' : '搜索音乐资料'" :placeholder="tab === 'library' ? '歌名、艺术家、专辑或文件名' : '单曲、专辑、艺术家'" prepend-inner-icon="mdi-magnify" variant="solo" flat rounded="pill" clearable hide-details @keyup.enter="search" @click:clear="query = ''; search()"><template #append-inner><VBtn icon="mdi-magnify" variant="text" aria-label="搜索音乐" :loading="loading" @click="search" /></template></VTextField>
      <VBtn v-if="tab === 'library'" prepend-icon="mdi-folder-sync-outline" variant="tonal" :disabled="busy" @click="submit('scan', {})">扫描音乐库</VBtn>
      <VTextField v-if="tab === 'cache'" v-model="query" label="搜索识别缓存" prepend-inner-icon="mdi-magnify" hide-details clearable /><VBtn v-if="tab==='cache' && selectedCache.length" color="error" variant="tonal" @click="clearSelectedCache">删除选中缓存</VBtn>
      <VBtn v-if="tab === 'cache'" color="error" variant="tonal" prepend-icon="mdi-delete-outline" @click="cacheConfirm = true">清空识别缓存</VBtn>
      <VBtn v-if="preview && !busy" variant="tonal" @click="previewOpen = true">查看预览</VBtn><VBtn icon="mdi-refresh" variant="text" aria-label="刷新音乐" :loading="loading" @click="load" />
    </div>
    <VCard v-if="tab === 'explore'" class="my-4"><VCardText>
      <div class="music-filter-row"><VLabel>来源</VLabel><VChipGroup column color="primary" v-model="exploreSource" mandatory class="music-source-chips"><VChip value="musicbrainz" filter tile>MusicBrainz</VChip><VChip value="doubanmusic" filter tile>豆瓣音乐</VChip><VChip v-for="source in streamingSources" :key="source.value" :value="source.value" filter tile>{{ source.title }}</VChip></VChipGroup></div>
      <div v-if="error" class="d-flex ga-2 mb-3"><VBtn variant="tonal" :loading="loading" @click="load">重试</VBtn><VBtn v-if="exploreSource==='musicbrainz'" variant="text" @click="exploreSource='doubanmusic'">豆瓣音乐</VBtn></div>
      <MusicExploreFilters v-if="exploreSource === 'musicbrainz'" @change="exploreChanged" />
      <MusicDoubanFilters v-else-if="exploreSource === 'doubanmusic'" @change="doubanChanged" />
      <template v-else>
        <div class="music-filter-row"><VLabel>榜单</VLabel><VChipGroup column color="primary" v-model="streamParams.chart" mandatory class="streaming-chart-chips" @update:model-value="search"><VChip v-for="chart in charts" :key="chart.value" :value="chart.value" filter tile>{{ chart.title }}</VChip></VChipGroup></div>
        <div v-if="exploreSource==='apple'" class="music-filter-row"><VLabel>地区</VLabel><VChipGroup column color="primary" v-model="streamParams.region" mandatory @update:model-value="search"><VChip v-for="region in [{title:'中国大陆',value:'cn'},{title:'中国香港',value:'hk'},{title:'中国台湾',value:'tw'},{title:'美国',value:'us'},{title:'日本',value:'jp'},{title:'韩国',value:'kr'},{title:'英国',value:'gb'}]" :key="region.value" :value="region.value" filter tile>{{ region.title }}</VChip></VChipGroup></div>
      </template>
    </VCardText></VCard>
    <VAlert v-if="error || taskError" type="error" variant="tonal" class="my-3">{{ error || taskError }}</VAlert>
    <VAlert v-if="taskId" type="info" variant="tonal" class="my-3"><div class="music-toolbar"><span>{{ taskState?.current_action }} · {{ taskState?.message }}</span><VSpacer /><VBtn variant="text" to="/logs">查看日志</VBtn></div><VProgressLinear class="mt-2" :model-value="taskState?.progress" :indeterminate="!taskState?.progress_known" /></VAlert>
    <VAlert v-if="resultData && !busy" :type="resultData.failed || resultData.warning_details?.length ? 'warning' : 'success'" variant="tonal" class="my-3"><div class="music-toolbar"><span>{{ resultData.message }}</span><VSpacer /><VBtn variant="text" @click="resultOpen=true">查看详情</VBtn></div></VAlert>
    <template v-if="tab === 'library'">
      <MusicLibraryPath :directory="directory" :root="libraryRoot" :loading="loading" @navigate="navigateDirectory" />
      <div class="music-workbench">
        <div class="music-workbench-files">
      <MusicFileBrowser v-model:selected="selected" :items="items" :directory="directory" :searching="!!query" :loading="loading" :busy="busy" @navigate="navigateDirectory" @edit="openEditor" @preview="songPreview=$event" @rescrape="openAction('rescrape', $event)" />

        </div>
        <MusicFolderTracks :searching="!!query" :items="folderTracks" :directory="directory" :loading="loading" :busy="busy" @edit="openEditor" @preview="songPreview=$event" />
      </div>
      <Teleport to="body"><VFooter app role="contentinfo" height="auto" color="surface" class="music-pagination music-library-footer" aria-label="音乐库分页"><VSelect :mobile-layout="false" v-model="pageSize" :items="[25,50,100,500,1000]" label="每页条数" hide-details density="compact" @update:model-value="search" /><span>{{ total ? (page-1)*pageSize+1 : 0 }} - {{ Math.min(page*pageSize,total) }} / {{ total }}</span><VPagination v-model="page" :length="pages" :total-visible="3" show-first-last-page color="primary" @update:model-value="load" /></VFooter></Teleport>
    </template>
    <MusicRecognitionCachePanel v-else-if="tab==='cache'" v-model:selected-items="selectedCache" :items="items.filter(item=>[item.title,item.album,(item.artists||[]).join(' '),item.key].join(' ').toLowerCase().includes((query||'').toLowerCase()))" :loading="loading" @delete="clearCache" />
    <template v-else><VProgressLinear v-if="loading" indeterminate color="primary" /><VRow class="mt-2"><VCol v-for="item in items" :key="`${item.media_source}:${item.music_type}:${item.media_id}`" cols="12" md="6" lg="4"><MusicCard :music="item" @open="openDetail" @artist="openDetail" @album="openDetail" /></VCol></VRow><div v-if="!loading && !items.length" class="text-center pa-8 text-medium-emphasis">暂无音乐结果</div><div v-if="tab === 'explore'" class="music-pagination"><VBtn variant="text" :disabled="page <= 1 || loading" @click="page--; load()">上一页</VBtn><span>{{ page }}</span><VBtn variant="text" :disabled="!exploreMore || loading" @click="page++; load()">下一页</VBtn></div></template>

    <VDialog v-model="detailOpen" max-width="76rem" :fullscreen="smAndDown" scrollable><VCard><VCardItem title="音乐详情" class="pe-16" /><DialogCloseBtn @click="detailOpen = false; detailRequest++" /><VDivider /><VCardText><VProgressLinear v-if="detailBusy" indeterminate /><VAlert v-if="detailError" type="error" variant="tonal">{{ detailError }}</VAlert><MusicDetailLayout v-if="detail" :title="detail.title || detail.name" :cover="detail.cover_url || detail.image_url" :rounded="detailKind === 'artist'" :attributes="[sourceLabel(detail.media_source),detail.release_date,detail.audio_specs,formatMusicDuration(detail.duration)].filter(Boolean)"><template #subtitle><span v-if="detailKind==='artist'">{{ detail.disambiguation || detail.sort_name }}</span><VBtn v-for="(artist,index) in detail.artists || []" :key="index" variant="text" :disabled="!detail.artist_ids?.[index]" @click="openDetail({media_source:detail.media_source,media_id:detail.artist_ids[index],music_type:'artist'})">{{ artist }}</VBtn></template><template #body><p v-if="detail.overview" class="music-overview-text">{{ detail.overview }}</p><VBtn v-if="/^https?:\/\//i.test(detail.detail_link || '')" :href="detail.detail_link" target="_blank" rel="noopener noreferrer" variant="text" prepend-icon="mdi-link">{{ sourceLabel(detail.media_source) }}</VBtn><VSelect v-if="editions.length" :model-value="release" :items="editions.map(item=>({value:item.media_id,title:[item.title,item.date,item.country,(item.formats||[]).join('/')].filter(Boolean).join(' · ')}))" label="发行版本" :disabled="detailBusy" @update:model-value="openDetail(detail,$event)" /><template v-if="detailAlbum"><VBtn variant="text" color="primary" prepend-icon="mdi-album" @click="openDetail({...detailAlbum,music_type:'album'})">{{ detailAlbum.title }}</VBtn><MusicTrackList :tracks="detailAlbum.tracks || []" :active-media-id="detail.media_id" @open="openDetail" /></template><MusicTrackList v-if="detail.tracks?.length" :tracks="detail.tracks" @open="openDetail" /></template><template #facts><div v-for="[label,value] in detailFacts" :key="label" class="music-detail-fact"><span>{{ label }}</span><span>{{ value }}</span></div><div v-if="detail.genres?.length"><strong>流派</strong><p>{{ detail.genres.join(' / ') }}</p></div><div v-if="detail.tags?.length"><strong>标签</strong><p>{{ detail.tags.join(' / ') }}</p></div><div v-for="field in ['composers','conductors','orchestras']" :key="field"><template v-if="detail[field]?.length"><strong>{{ {composers:'作曲',conductors:'指挥',orchestras:'乐团'}[field] }}</strong><p>{{ detail[field].join(' / ') }}</p></template></div></template><template v-if="detailKind==='artist'"><h3 class="text-h6 mt-4">艺术家作品</h3><VTabs v-model="artistType" color="primary" show-arrows @update:model-value="loadArtistAlbums(true)"><VTab value="album">专辑</VTab><VTab value="ep">EP</VTab><VTab value="single">单曲</VTab><VTab value="compilation">合辑</VTab><VTab value="soundtrack">原声</VTab><VTab value="live">现场</VTab><VTab value="remix">混音</VTab></VTabs><VProgressLinear v-if="relatedBusy" indeterminate /></template><h3 v-else-if="related.length" class="text-h6 mt-4">{{ relatedLabel }}</h3><VRow class="mt-4"><VCol v-for="item in related" :key="item.media_id" cols="12" md="6"><MusicCard :music="item" @open="openDetail" @artist="openDetail" @album="openDetail" /></VCol></VRow><div v-if="detailKind==='artist'" class="music-pagination"><VBtn :disabled="relatedPage<=1 || relatedBusy" variant="text" @click="relatedPage--; loadArtistAlbums()">上一页</VBtn><span>{{ relatedPage }}</span><VBtn :disabled="related.length<30 || relatedBusy" variant="text" @click="relatedPage++; loadArtistAlbums()">下一页</VBtn></div><template v-if="relatedArtists.length"><h3 class="text-h6 mt-4">相关艺术家</h3><VRow><VCol v-for="item in relatedArtists" :key="item.media_id" cols="12" md="6"><MusicArtistCard :artist="item" @open="openDetail" /></VCol></VRow></template></MusicDetailLayout></VCardText></VCard></VDialog>

    <MusicSongPreview v-if="songPreview" :key="songPreview.path" :item="songPreview" @close="songPreview=null" @edit="openEditor" />
    <MusicCorrectionDialog v-if="correctionGroup" :group="correctionGroup" :busy="busy" @close="correctionGroup = null" @apply="correctGroup" />
    <MusicTagEditor v-if="editorOpen" :key="editor?.path || 'loading'" :item="editor" :focus-field="editorFocus" :error="editorError || taskError" :busy="busy" @close="editorOpen=false; editorRequest++" @save="saveEditor" />

    <VDialog :model-value="actionOpen && !selectorOpen" @update:model-value="actionOpen = $event" max-width="42rem" :fullscreen="smAndDown" scrollable><VCard><VCardItem :title="actionLabel" :subtitle="`共 ${actionPaths.length} 项`" class="pe-16" /><DialogCloseBtn @click="actionOpen = false" /><VDivider /><VCardText><VAlert v-if="taskError" type="error" variant="tonal" class="mb-3">{{ taskError }}</VAlert><p class="mb-4">按音乐刮削设置更新所选文件的标签、封面和歌词，保留当前文件名和位置。</p><VTextField :model-value="selection ? `${sourceLabel(selection.media_source)} · ${selection.media_id}` : ''" label="指定音乐或专辑" placeholder="留空自动识别" readonly clearable @click:clear="selection = null"><template #append-inner><VBtn icon="mdi-magnify" variant="text" aria-label="选择音乐资料" @click="selectorOpen = true" /></template></VTextField><VTextField v-if="selection?.media_source === 'musicbrainz' && selection?.music_type === 'album'" v-model="selection.musicbrainz_release_id" :error-messages="selectedReleaseError" label="MusicBrainz 具体发行版本 ID（可选）" prepend-inner-icon="mdi-album" hint="保留专辑 ID；预览时会核对发行归属和曲目。" persistent-hint /></VCardText><VDivider /><VCardActions class="app-dialog-actions"><VSpacer /><VBtn color="default" variant="text" @click="actionOpen = false">取消</VBtn><VBtn color="primary" variant="flat" :disabled="busy || !actionPaths.length || !!selectedReleaseError" @click="previewAction">预览</VBtn></VCardActions></VCard></VDialog>

    <VDialog v-model="selectorOpen" max-width="700" :fullscreen="smAndDown" scrollable><VCard><VCardText><VSelect v-model="selectorSource" :items="musicSources" label="音乐数据源" /><MusicIdSelector :key="selectorSource" :type="selectorSource" :music-types="actionPaths.length > 1 ? ['album'] : ['recording','album']" @select="chooseMusic" @close="selectorOpen = false" /></VCardText></VCard></VDialog>

    <VDialog :model-value="previewOpen && !correctionGroup" @update:model-value="previewOpen = $event" max-width="64rem" :fullscreen="smAndDown" scrollable><VCard><VCardItem title="音乐刮削预览" :subtitle="`共 ${preview?.items?.length || 0} 项`" class="pe-16" /><DialogCloseBtn @click="previewOpen = false" /><VDivider /><VCardText><MusicPreview :groups="previewGroups" :disabled="busy || preview?.operation === 'edit'" :scope-complete="true" :corrected="Object.keys(corrections).length > 0" @correct="correctionGroup = $event" /></VCardText><VDivider /><VCardActions class="app-dialog-actions"><VSpacer /><VBtn color="default" variant="text" @click="previewOpen = false">取消</VBtn><VBtn color="primary" variant="flat" :disabled="!canExecute" @click="submit('execute', {preview_task_id: previewTask})">确认</VBtn></VCardActions></VCard></VDialog>
    <VDialog v-model="resultOpen" max-width="48rem" scrollable><VCard><VCardItem title="音乐任务结果" class="pe-16" /><DialogCloseBtn @click="resultOpen=false" /><VCardText><VAlert v-for="warning in resultData?.warning_details || []" :key="warning" type="warning" variant="tonal" class="mb-3">{{ warning }}</VAlert><VList><VListItem v-for="item in resultData?.items || []" :key="item.path" :title="item.path" :subtitle="item.message || '已扫描'"><template #prepend><VIcon :icon="item.success===false ? 'mdi-alert-circle-outline' : 'mdi-check-circle-outline'" :color="item.success===false ? 'error' : 'success'" /></template></VListItem></VList></VCardText><VCardActions class="app-dialog-actions"><VSpacer /><VBtn variant="text" to="/logs">查看日志</VBtn><VBtn variant="flat" @click="resultOpen=false">关闭</VBtn></VCardActions></VCard></VDialog>
    <VDialog v-model="cacheConfirm" max-width="28rem"><VCard title="确认清空音乐识别缓存？"><VCardText>识别结果及音乐资料请求缓存将清空，本地音乐文件保持不变。</VCardText><VCardActions class="app-dialog-actions"><VSpacer /><VBtn color="default" variant="text" @click="cacheConfirm = false">取消</VBtn><VBtn color="error" variant="flat" @click="clearCache()">清空</VBtn></VCardActions></VCard></VDialog>
  </div>
</template>

<style scoped>
.music-filter-row :deep(.v-chip--selected){background:rgb(var(--v-theme-primary))!important;color:rgb(var(--v-theme-on-primary))!important;}.music-filter-row :deep(.v-chip--selected :is(.v-chip__underlay, .v-chip__overlay)){opacity:0!important;}.music-filter-row :deep(.v-chip--selected .v-chip__content){color:inherit!important;}

.music-workbench{display:grid;grid-template-columns:minmax(15rem,19rem) minmax(0,1fr);gap:1.25rem;align-items:start;}.music-workbench-files{min-inline-size:0;}.music-pagination :deep(.v-pagination__item--is-active .v-btn){opacity:1!important;color:rgb(var(--v-theme-on-primary))!important;background:rgb(var(--v-theme-primary))!important;}.music-pagination :deep(.v-pagination__item--is-active .v-btn__content){opacity:1!important;}.music-pagination :deep(.v-pagination__item--is-active .v-btn__overlay){opacity:0!important;}.streaming-chart-chips{min-inline-size:0;}
@media(max-width:960px){.music-workbench{grid-template-columns:minmax(0,1fr);}}

.music-pagination.music-library-footer{position:fixed!important;background:rgb(var(--v-theme-surface))!important;gap:.75rem;padding:.75rem 1rem;border-block-start:1px solid rgba(var(--v-border-color),var(--v-border-opacity));box-shadow:0 -3px 14px rgba(0,0,0,.06);}.music-library-footer :deep(.v-pagination){max-inline-size:100%;}
@media(max-width:1279px){.music-pagination.music-library-footer{padding-block-end:calc(80px + env(safe-area-inset-bottom));}}
.music-page-shell { min-inline-size: 0; }
.music-toolbar,.music-selection,.music-pagination,.music-filter-row,.music-preview-header { display:flex; align-items:center; gap:12px; flex-wrap:wrap; }
.music-toolbar > .v-input { flex:1 1 220px; min-inline-size:0; }
.music-toolbar .music-sources { max-inline-size:340px; }.music-toolbar .music-kind { max-inline-size:200px; }
.music-filter-row > .v-chip-group { flex:1 1 15rem; min-inline-size:0; }.music-filter-row > .v-label { min-inline-size:4rem; }.music-filter-row > .v-input { max-inline-size:15rem; }
.music-selection { padding-block:12px; }.music-selection > .v-input { flex:0 0 auto; }
.music-library-identity { display:flex; align-items:center; gap:12px; padding-block:12px; min-inline-size:240px; }
.music-library-identity > div:last-child { min-inline-size:0; }.music-title-link { text-align:start; overflow-wrap:anywhere; font-weight:600; }
.music-path { overflow-wrap:anywhere; }.music-pagination { justify-content:center; padding-block:16px; }.music-pagination > .v-input { max-inline-size:110px; }
.music-detail-fact {display:flex;justify-content:space-between;gap:16px;padding-block:8px;border-bottom:1px solid rgba(var(--v-theme-on-surface),.12);overflow-wrap:anywhere;}.music-detail-fact>span:last-child{text-align:end;min-width:0;}
.music-overview-text { white-space:pre-wrap; overflow-wrap:anywhere; margin-block-end:20px; }
.music-preview-group { margin-block-end:12px; border:1px solid rgba(var(--v-border-color),var(--v-border-opacity)); border-radius:10px; overflow-wrap:anywhere; }
.music-preview-header { padding:12px; }.music-preview-paths { display:grid; grid-template-columns:1fr 1fr; gap:12px; padding:12px; border-top:1px solid rgba(var(--v-border-color),var(--v-border-opacity)); }.music-preview-paths > div { min-inline-size:0; }
@media(max-width:600px){.music-preview-paths{grid-template-columns:1fr;}.music-toolbar .music-sources,.music-toolbar .music-kind{max-inline-size:none;}.music-library :deep(.v-table__wrapper){overflow-x:auto;}}
</style>
