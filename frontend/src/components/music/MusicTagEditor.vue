<!-- Form/selector/split preview adapted from MP ReorganizeDialog and music-transfer (MIT). -->
<script setup>
import { computed, ref, onMounted, onBeforeUnmount, nextTick, watch } from 'vue';
import axios from 'axios';
import { useDisplay } from 'vuetify';
import DialogCloseBtn from '@/components/theme/DialogCloseBtn.vue';
import MusicIdSelector from './MusicIdSelector.vue';
import MusicAudioPlayer from './MusicAudioPlayer.vue';
import MusicLyricsSearch from './MusicLyricsSearch.vue';
import { metadataSources as musicSources, streamingSources, gdSources, musicSearchTerm, musicReleaseError, musicFieldOptions } from './utils';
import { shiftLyrics, formatLyrics } from './lyrics';
import { prepareMusicCover } from './cover';
import MusicLyricsTimeline from './MusicLyricsTimeline.vue';
import MusicLyricsTranslation from './MusicLyricsTranslation.vue';
import MusicReleaseSelect from './MusicReleaseSelect.vue';
import MusicProcessingOptions from './MusicProcessingOptions.vue';
const props = defineProps({ item: Object, error: String, busy: Boolean, focusField: String, visible: {type:Boolean,default:true} });
const emit = defineEmits(['close','save']);
const { smAndDown } = useDisplay();
const draft = ref(props.item ? JSON.parse(JSON.stringify(props.item)) : null);
const source = ref(props.item?.media_source || 'musicbrainz'), entity = ref(props.item?.music_search_type === 'recording' ? 'recording' : 'album');
const identity = ref(entity.value === (props.item?.music_type || 'recording') ? props.item?.media_id || '' : entity.value === 'album' && source.value === 'musicbrainz' ? props.item?.musicbrainz_release_group_id || '' : '');
const selectedRelease=ref(''), releaseLoading=ref(false),releaseLoadError=ref('');
const sourceMetadata=ref(null), albumDetails=ref(null), chosenTrack=ref(null);
const trackMatch=ref('strict'), cueEnable=ref(props.item?.music_cue_enable !== false), appliedSelection=ref(null);
const options=key=>musicFieldOptions(key,sourceMetadata.value,albumDetails.value);
const withCurrent=(key,current)=>[...new Set([...(Array.isArray(current)?current:[current]),...options(key)].filter(value=>value!==null && value!==undefined && value!==''))];
const albumTracks=computed(()=>(albumDetails.value?.tracks || []).map((track,index)=>({value:index,title:[`${track.disc_number || 1}.${track.track_number || index+1}`,track.title,track.artist || track.artists?.join(' / ')].filter(Boolean).join(' · ')})));
function receiveAlbum(value){albumDetails.value=value;chosenTrack.value=null;sourceMetadata.value=null;}
function selectTrack(index){
  appliedSelection.value=null;
  chosenTrack.value=index;
  const album=albumDetails.value,row=album?.tracks?.[index];
  if(row)applyMetadata({...row,media_source:row.media_source || source.value,music_type:'recording',album:row.album || album.title,
    album_artist:row.album_artist || album.artist,total_tracks:row.total_tracks ?? album.total_tracks,total_discs:row.total_discs ?? album.total_discs,
    musicbrainz_release_id:row.musicbrainz_release_id || album.musicbrainz_release_id,musicbrainz_release_group_id:row.musicbrainz_release_group_id || album.media_id});
}
const releaseError=computed(()=>releaseLoadError.value || musicReleaseError({media_source:source.value,music_type:entity.value,media_id:identity.value,musicbrainz_release_id:selectedRelease.value}));
watch([source,entity,identity],()=>{selectedRelease.value='';albumDetails.value=null;chosenTrack.value=null;sourceMetadata.value=null;},{flush:'sync'});
watch(entity,()=>{identity.value='';trackMatch.value='strict';},{flush:'sync'});
watch([source,entity,identity,selectedRelease],()=>{appliedSelection.value=null;},{flush:'sync'});
const searchOpen = ref(false), lookupBusy = ref(false), error = ref(''), offset = ref(0);
const lyricsSearchOpen = ref(false), form = ref(null), coverOnly = ref(false), coverError = ref(false);
const searchAlbumId=ref(''), coverInput=ref(null), coverBusy=ref(false), timelineOpen=ref(false), translationOpen=ref(false);
let coverGeneration=0;
const defaultSearchKeyword=computed(()=>musicSearchTerm(draft.value || {},entity.value));
const searchKeyword=ref(defaultSearchKeyword.value);
watch(defaultSearchKeyword,(value,previous)=>{if(searchKeyword.value===previous)searchKeyword.value=value;});
watch(searchOpen,open=>{if(!open)searchKeyword.value=defaultSearchKeyword.value;});
const isStreaming=computed(()=>[...streamingSources,...gdSources].some(item=>item.value===source.value));
const entityOptions=[{title:'单曲',value:'recording'},{title:'专辑',value:'album'}];
const coverPreview=computed(()=>draft.value?.cover_upload || draft.value?.cover_url || `/api/music/image?path=${encodeURIComponent(props.item?.path || '')}&v=${encodeURIComponent(props.item?.signature?.mtime || '')}`);
function searchMetadata(onlyCover=false){searchAlbumId.value='';coverOnly.value=onlyCover;searchOpen.value=true;}
function readIdentity(){if(releaseError.value || releaseLoading.value)return;coverOnly.value=false;if(isStreaming.value && entity.value==='album'){searchAlbumId.value=identity.value;searchOpen.value=true;}else applySelection({});}
const performers = ref(Object.entries(props.item?.performers || {}).map(([role,names])=>({role,names})));
let generation = 0;
const validId = computed(() => !identity.value || (source.value==='musicbrainz' ? /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(identity.value) : /^[A-Za-z0-9_=-]{1,200}$/.test(identity.value)));
const fields = [['title','标题'],['album','专辑'],['album_artist','专辑艺术家'],['isrc','ISRC']];
const numbers = [['track_number','曲序'],['total_tracks','总曲数'],['disc_number','碟号'],['total_discs','总碟数'],['release_year','发行年份'],['original_year','原始年份']];
const credits = [['artists','艺术家'],['composers','作曲'],['conductors','指挥'],['orchestras','乐团']];
const ids = [['media_id','MusicBrainz 录音 ID'],['musicbrainz_release_id','MusicBrainz 发行 ID'],['musicbrainz_release_group_id','MusicBrainz 专辑组 ID'],['musicbrainz_release_track_id','MusicBrainz 发行曲目 ID']];
async function applySelection(item) {
  if (!draft.value || lookupBusy.value) return;
  entity.value = item.music_type || entity.value; identity.value = item.id || identity.value;
  searchOpen.value = false;
  if(item.id && entity.value==='album' && source.value==='musicbrainz' && !coverOnly.value)return;
  lookupBusy.value = true; error.value = '';
  const version = ++generation;
  try {
    const selection={media_source:source.value,media_id:identity.value,music_type:entity.value,...(source.value==='musicbrainz' && entity.value==='album' && selectedRelease.value ? {musicbrainz_release_id:selectedRelease.value.trim().toLowerCase()} : {})};
    const {data} = await axios.post('/api/music/query/edit_metadata', {path:draft.value.path,selection,music_track_match:trackMatch.value,music_cue_enable:cueEnable.value}, {timeout:190000});
    if (version !== generation) return;
    if(!coverOnly.value)appliedSelection.value=selection.music_type==='album' ? selection : null;
    applyMetadata(data,coverOnly.value);
  } catch(cause) { if (version===generation) error.value = cause.response?.data?.message || cause.message; }
  finally { if (version===generation) lookupBusy.value=false; }
}
function applyMetadata(data,onlyCover=false){
  sourceMetadata.value=onlyCover ? {...sourceMetadata.value,cover_url:data.cover_url} : data;
  const keys=onlyCover ? ['cover_url'] : [...fields,...numbers,...credits,...(isStreaming.value ? [] : ids)].map(([key])=>key).concat(isStreaming.value ? ['cover_url'] : ['media_source','music_type','cover_url']);
  for(const key of keys)if(data[key]!=null && data[key]!=='' && (!Array.isArray(data[key]) || data[key].length))draft.value[key]=data[key];
  if(data.cover_url)draft.value.cover_upload='';
  coverError.value=false;
  if(onlyCover && !data.cover_url)error.value='此资料没有可用封面，请选择其他结果';
  if(!onlyCover && Object.keys(data.performers || {}).length)performers.value=Object.entries(data.performers).map(([role,names])=>({role,names}));
}
function shift() { try { draft.value.lyrics=shiftLyrics(draft.value.lyrics || '', offset.value); offset.value=0; error.value=''; } catch(cause) { error.value=cause.message; } }
function applyLyrics(value){draft.value.lyrics=value.text;draft.value.lyrics_source=value.source;lyricsSearchOpen.value=false;offset.value=0;}
function save() {
  if (!draft.value || props.busy || lookupBusy.value || coverBusy.value) return;
  const values={};
  for(const item of performers.value) if(item.role.trim() && item.names?.length) values[item.role.trim()]=[...(values[item.role.trim()] || []),...item.names];
  emit('save',{...draft.value,performers:values},{music_type:entity.value,music_track_match:entity.value==='album' ? trackMatch.value : 'strict',music_cue_enable:cueEnable.value,selection:appliedSelection.value});
}
onBeforeUnmount(()=>{generation++;coverGeneration++;});
async function uploadCover(event){
  const file=event.target.files?.[0];event.target.value='';if(!file)return;
  const version=++coverGeneration;coverBusy.value=true;error.value='';
  try{const data=await prepareMusicCover(file);if(version!==coverGeneration)return;draft.value.cover_upload=data;draft.value.cover_url='';coverError.value=false;}
  catch(cause){if(version===coverGeneration)error.value=cause.message || '图片无法读取，请选择其他图片';}
  finally{if(version===coverGeneration)coverBusy.value=false;}
}
function formatTime(){try{draft.value.lyrics=formatLyrics(draft.value.lyrics || '');error.value='';}catch(cause){error.value=cause.message;}}
async function focusEditor(){
  await nextTick();
  if(!props.focusField)return;
  const input=form.value?.$el?.querySelector(`[data-music-field="${props.focusField}"] input, [data-music-field="${props.focusField}"] textarea`);
  input?.scrollIntoView({block:'center'});input?.focus({preventScroll:true});
}
onMounted(focusEditor);
</script>

<template>
  <VDialog :model-value="visible && !searchOpen && !lyricsSearchOpen && !timelineOpen && !translationOpen" @update:model-value="!$event && emit('close')" :max-width="'72rem'" :fullscreen="smAndDown" scrollable @after-enter="focusEditor">
    <VCard class="reorganize-dialog-card"><VCardItem class="py-3 pe-16"><template #prepend><VIcon icon="mdi-music-box-outline" /></template><VCardTitle>编辑音乐标签</VCardTitle><VCardSubtitle class="editor-path">{{ item?.path }}</VCardSubtitle></VCardItem><DialogCloseBtn @click="emit('close')" /><VDivider />
      <VCardText><VAlert v-if="props.error || error" type="error" variant="tonal" class="mb-4">{{ props.error || error }}</VAlert><VProgressLinear v-if="(!draft && !props.error) || lookupBusy" indeterminate color="primary" class="mb-4" />
        <VForm v-if="draft" ref="form" :disabled="lookupBusy || busy || coverBusy" @submit.prevent="save">
          <MusicAudioPlayer v-if="visible && !searchOpen && !lyricsSearchOpen && !timelineOpen && !translationOpen" :path="draft.path" :lyrics="draft.lyrics" class="mb-5" />
          <VRow><VCol cols="12" md="6"><VSelect v-model="source" :items="musicSources" label="资料来源" prepend-inner-icon="mdi-database-search" @update:model-value="identity=''" /></VCol><VCol cols="12" md="6"><VSelect v-model="entity" :items="entityOptions" @update:model-value="searchKeyword=musicSearchTerm(draft, $event)" label="音乐类型" prepend-inner-icon="mdi-music-box-multiple" /></VCol>
          <VCol cols="12"><VTextField v-model="searchKeyword" label="搜索音乐资料" :placeholder="source === 'musicbrainz' ? '输入名称或 MusicBrainz ID' : '输入音乐名称'" :hint="source === 'musicbrainz' ? 'ID 按所选音乐类型精确查询：专辑使用专辑组 ID，单曲使用录音 ID。' : ''" :persistent-hint="source === 'musicbrainz'" :mobile-layout="false" prepend-inner-icon="mdi-magnify" @keyup.enter.prevent="searchMetadata()"><template #append><VBtn variant="tonal" :disabled="lookupBusy || !searchKeyword.trim()" @click="searchMetadata()">搜索资料</VBtn></template></VTextField></VCol>
          <VCol cols="12"><VTextField v-model.trim="identity" :label="`${musicSources.find(item=>item.value===source)?.title || source} ID`" prepend-inner-icon="mdi-identifier" hint="搜索资料并填入下方标签，核对后预览保存。" persistent-hint :error-messages="validId ? [] : ['ID 格式不正确']" @keyup.enter.prevent="validId && identity && readIdentity()"><template #append-inner><VBtn icon="mdi-magnify" variant="text" aria-label="搜索音乐资料" :disabled="lookupBusy" @click="searchMetadata()" /></template><template #append><VBtn variant="tonal" :loading="lookupBusy" :disabled="!identity || !validId || !!releaseError || releaseLoading" @click="readIdentity()">读取资料</VBtn></template></VTextField></VCol><VCol v-if="source==='musicbrainz' && entity==='album'" cols="12"><MusicReleaseSelect v-model="selectedRelease" :album-id="identity" :disabled="busy || lookupBusy" @loading="releaseLoading=$event" @error="releaseLoadError=$event" @album="receiveAlbum" /></VCol><VCol v-if="albumTracks.length" cols="12"><VSelect :model-value="chosenTrack" :items="albumTracks" label="专辑曲目" :disabled="releaseLoading || lookupBusy || busy" @update:model-value="selectTrack"><template #item="{props:itemProps}"><VListItem v-bind="itemProps" role="option" class="music-track-option" /></template></VSelect></VCol></VRow>
          <MusicProcessingOptions v-model:cue-enable="cueEnable" v-model:track-match="trackMatch" :entity="entity" :disabled="busy || lookupBusy" class="mt-4" />
          <VDivider class="my-6" />
          <VRow class="mb-3"><VCol cols="12" md="6"><VTextarea v-model="draft.comment" label="备注" data-music-field="comment" rows="3" auto-grow hint="保存在音频标签中；清空后预览保存可删除。" persistent-hint /></VCol><VCol cols="12" md="6"><VTextarea v-model="draft.description" label="简介 / 描述" data-music-field="description" rows="3" auto-grow hint="保存在音频标签中，与备注分别编辑。" persistent-hint /></VCol></VRow>
          <VRow><VCol v-for="[key,label] in fields" :key="key" cols="12" md="6"><VSelect v-if="options(key).length" v-model="draft[key]" :items="withCurrent(key,draft[key])" :label="label" :data-music-field="key" /><VTextField v-else v-model="draft[key]" :label="label" :data-music-field="key" /></VCol>
          <VCol v-for="[key,label] in credits" :key="key" cols="12" md="6"><VSelect v-if="options(key).length" v-model="draft[key]" :items="withCurrent(key,draft[key])" :label="label" :data-music-field="key" multiple chips closable-chips /><VCombobox v-else v-model="draft[key]" :label="label" :data-music-field="key" multiple chips closable-chips /></VCol>
          <VCol v-for="[key,label] in numbers" :key="key" cols="6" md="4"><VSelect v-if="options(key).length" v-model="draft[key]" :items="withCurrent(key,draft[key])" :data-music-field="key" :label="label" /><VTextField v-else v-model.number="draft[key]" :data-music-field="key" :label="label" type="number" min="0" /></VCol></VRow>
          <div class="d-flex align-center ga-3 my-4"><h3 class="text-subtitle-1">演奏者</h3><VBtn variant="text" prepend-icon="mdi-plus" @click="performers.push({role:'',names:[]})">添加乐器 / 声部</VBtn></div>
          <VRow v-for="(performer,index) in performers" :key="index"><VCol cols="12" md="4"><VSelect v-if="options('performer_role').length" v-model="performer.role" :items="withCurrent('performer_role',performer.role)" label="乐器 / 声部" /><VTextField v-else v-model="performer.role" label="乐器 / 声部" /></VCol><VCol cols="10" md="7"><VSelect v-if="sourceMetadata?.performers?.[performer.role]?.length" v-model="performer.names" :items="[...new Set([...(performer.names || []),...sourceMetadata.performers[performer.role]])]" label="演奏者姓名" multiple chips closable-chips /><VCombobox v-else v-model="performer.names" label="演奏者姓名" multiple chips closable-chips /></VCol><VCol cols="2" md="1"><VBtn icon="mdi-close" variant="text" aria-label="移除演奏者" @click="performers.splice(index,1)" /></VCol></VRow>
          <VExpansionPanels class="my-5" variant="accordion"><VExpansionPanel title="MusicBrainz 身份标签"><VExpansionPanelText><VRow><VCol v-for="[key,label] in ids.filter(([key])=>key!=='media_id' || draft.media_source==='musicbrainz' && draft.music_type==='recording')" :key="key" cols="12" md="6"><VSelect v-if="options(key).length" v-model="draft[key]" :items="withCurrent(key,draft[key])" :label="label" /><VTextField v-else v-model="draft[key]" :label="label" /></VCol></VRow></VExpansionPanelText></VExpansionPanel></VExpansionPanels>
          <div class="editor-cover mb-4"><VImg v-if="!coverError" :key="coverPreview" :src="coverPreview" width="128" height="128" cover rounded @error="coverError=true" /><VIcon v-else icon="mdi-music-box-outline" size="80" /><input ref="coverInput" type="file" accept="image/jpeg,image/png,image/webp" aria-label="上传本地封面" hidden @change="uploadCover" /><VBtn variant="tonal" prepend-icon="mdi-upload" :loading="coverBusy" @click="coverInput.click()">上传本地封面</VBtn><VBtn v-if="draft.cover_upload" variant="text" @click="draft.cover_upload='';coverError=false">撤销上传</VBtn><VBtn variant="tonal" prepend-icon="mdi-image-search-outline" @click="searchMetadata(true)">从资料源替换封面</VBtn></div>
          <VSelect v-if="options('cover_url').length" v-model="draft.cover_url" :items="withCurrent('cover_url',draft.cover_url).map(value=>({value,title:'资料源封面'}))" label="替换封面" data-music-field="cover_url" clearable @update:model-value="draft.cover_upload='';coverError=false" /><VTextField v-else v-model="draft.cover_url" @update:model-value="draft.cover_upload='';coverError=false" data-music-field="cover_url" label="替换封面地址" clearable prepend-inner-icon="mdi-image-outline" hint="留空保留当前封面；填写后将在保存时替换内嵌封面。" persistent-hint />
          <div class="d-flex align-center flex-wrap ga-3 mt-6 mb-3"><h3 class="text-subtitle-1">歌词</h3><VSpacer /><VBtn prepend-icon="mdi-magnify" variant="tonal" :disabled="busy || lookupBusy" @click="lyricsSearchOpen=true">搜索替换歌词</VBtn></div><div v-if="draft.lyrics_source" class="text-caption text-medium-emphasis mb-3">{{ draft.lyrics_source }}</div>
          <div class="lyrics-tools mb-3"><VBtn variant="tonal" prepend-icon="mdi-format-list-numbered" @click="formatTime">格式化时间</VBtn><VBtn variant="tonal" prepend-icon="mdi-translate" :disabled="!draft.lyrics?.trim()" @click="translationOpen=true">机器翻译</VBtn><VBtn variant="tonal" prepend-icon="mdi-timeline-clock-outline" :disabled="!draft.lyrics?.trim()" @click="timelineOpen=true">时间轴编辑器</VBtn></div>
          <VTextarea v-model="draft.lyrics" data-music-field="lyrics" label="旁挂歌词（LRC / 纯文本）" rows="12" spellcheck="false" hint="按 MP 的旁挂方式保存为同名歌词文件。修改只在预览确认后写入。" persistent-hint class="lyrics-editor" />
          <div class="lyrics-offset mt-4"><VTextField v-model.number="offset" label="整体偏移（秒）" type="number" step="0.1" hint="正数延后，负数提前；适用于整首偏差相同的歌词。" persistent-hint /><VBtn variant="tonal" @click="shift">应用到歌词</VBtn></div>
        </VForm>
      </VCardText><VDivider /><VCardActions class="app-dialog-actions"><VSpacer /><VBtn color="default" variant="text" @click="emit('close')">取消</VBtn><VBtn color="primary" variant="flat" :loading="busy" :disabled="!draft || busy || lookupBusy || coverBusy" @click="save">预览修改</VBtn></VCardActions>
    </VCard>
  </VDialog>
  <VDialog v-model="searchOpen" max-width="700" :fullscreen="smAndDown" scrollable><MusicIdSelector v-if="searchOpen" :type="source" :music-types="[entity]" :browse-tracks="isStreaming" :initial-album-id="searchAlbumId" :initial-keyword="searchKeyword" @select="applySelection" @close="searchOpen=false" /></VDialog>
  <MusicLyricsTimeline v-if="timelineOpen" :path="draft.path" :lyrics="draft.lyrics" @close="timelineOpen=false" @apply="draft.lyrics=$event;timelineOpen=false" />
  <MusicLyricsTranslation v-if="translationOpen" :lyrics="draft.lyrics" @close="translationOpen=false" @apply="draft.lyrics=$event;translationOpen=false" />
  <MusicLyricsSearch v-if="lyricsSearchOpen" :item="draft" @close="lyricsSearchOpen=false" @apply="applyLyrics" />
</template>

<style scoped>
.music-track-option :deep(.v-list-item-title),.reorganize-dialog-card :deep(.v-select__selection-text){white-space:normal;overflow-wrap:anywhere;line-height:1.5;}
.lyrics-tools{display:flex;flex-wrap:wrap;gap:.75rem;}.editor-cover{display:flex;align-items:center;gap:1rem;flex-wrap:wrap;}.editor-cover :deep(.v-img){flex:0 0 128px;}.editor-path{white-space:normal;overflow-wrap:anywhere;}.lyrics-offset{display:flex;align-items:flex-start;gap:1rem;flex-wrap:wrap;}.lyrics-offset>.v-input{flex:1 1 240px;}.lyrics-editor :deep(textarea){font-family:monospace;line-height:1.7;}.reorganize-dialog-card :deep(.v-card-text){min-inline-size:0;}
</style>
