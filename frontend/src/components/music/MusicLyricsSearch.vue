<script setup>
import { computed, onMounted, onBeforeUnmount, ref } from 'vue';
import axios from 'axios';
import { useDisplay } from 'vuetify';
import DialogCloseBtn from '@/components/theme/DialogCloseBtn.vue';
import MusicAudioPlayer from './MusicAudioPlayer.vue';
import { lyricSources, formatMusicDuration, musicSearchTerm } from './utils';
import { mergeLyricsTranslation } from './lyrics';
const props=defineProps({item:Object});
const emit=defineEmits(['close','apply']);
const {smAndDown}=useDisplay();
const source=ref('netease'), query=ref(musicSearchTerm(props.item));
const kind=ref('recording'), album=ref(null);
const supportsAlbumId=computed(()=>['netease','qqmusic','kugou','kuwo'].includes(source.value));
const queryLabel=computed(()=>kind.value==='album_id' ? '专辑 ID 或链接' : kind.value==='album' ? '搜索专辑或歌手' : '搜索歌曲或歌手');
const items=ref([]), selected=ref(null), variants=ref([]), variant=ref(''), page=ref(1), more=ref(false), loading=ref(false), reading=ref(false), error=ref('');
const bilingual=ref(false);
const alignmentProvider=ref('youdao'), alignmentLoading=ref(false), references=ref([]);
const alignmentProviders=[{title:'有道翻译',value:'youdao'},{title:'必应翻译',value:'bing'},{title:'谷歌翻译',value:'google'}];
let alignmentController, alignmentVersion=0;
const translated=computed(()=>variants.value.find(item=>item.value==='translation')?.text || '');
const combined=computed(()=>mergeLyricsTranslation(props.item.lyrics || '', variants.value.find(item=>['original','synced','plain'].includes(item.value))?.text || '', translated.value,references.value));
const pendingReference=computed(()=>combined.value.pendingLineIndices.filter(index=>!references.value.some(row=>row.index===index)));
const text=computed(()=>bilingual.value ? combined.value.text : variants.value.find(item=>item.value===variant.value)?.text || '');
let searchController, detailController, version=0, detailVersion=0;
function resetAlignment(){alignmentVersion++;alignmentController?.abort();alignmentLoading.value=false;references.value=[];}
function changeAlignmentProvider(){
  const confirmed=new Set(combined.value.splits.flatMap(segment=>segment.indices));
  alignmentVersion++;alignmentController?.abort();alignmentLoading.value=false;error.value='';
  references.value=references.value.filter(row=>confirmed.has(row.index));
}
function resetDetail(){resetAlignment();detailVersion++;detailController?.abort();selected.value=null;variants.value=[];variant.value='';bilingual.value=false;reading.value=false;}
async function alignPending(){
  if(!pendingReference.value.length || alignmentLoading.value)return;
  const request=++alignmentVersion;alignmentController=new AbortController();alignmentLoading.value=true;error.value='';
  try{
    const {data}=await axios.post('/api/music/query/lyrics_translate',{lyrics:props.item.lyrics,provider:alignmentProvider.value,line_indices:pendingReference.value},{signal:alignmentController.signal,timeout:120000});
    if(request===alignmentVersion){
      if(!Array.isArray(data.line_translations))throw new Error('当前服务端尚不支持辅助分行，请更新 ETK 后重试。');
      references.value=[...references.value,...data.line_translations];
    }
  }catch(cause){if(request===alignmentVersion && !axios.isCancel(cause))error.value=cause.response?.data?.message || cause.message;}
  finally{if(request===alignmentVersion)alignmentLoading.value=false;}
}
async function search(reset=true) {
  if(reset) page.value=1;
  const request=++version;
  searchController?.abort();searchController=new AbortController();resetDetail();loading.value=false;error.value='';items.value=[];more.value=false;
  if(!album.value && !query.value?.trim())return;
  loading.value=true;
  try{const {data}=await axios.post('/api/music/query/lyrics_search',{source:source.value,query:query.value,page:page.value,kind:kind.value,album_id:album.value?.id || (kind.value==='album_id' ? query.value : undefined)},{signal:searchController.signal,timeout:35000});if(request!==version)return;items.value=data.items || [];more.value=!!data.has_more;if(data.album)album.value=data.album;if(data.source)source.value=data.source;}
  catch(cause){if(request===version && !axios.isCancel(cause))error.value=cause.response?.data?.message || cause.message;}
  finally{if(request===version)loading.value=false;}
}
async function choose(item) {
  if(item.music_type==='album'){album.value=item;search();return;}
  resetDetail();selected.value=item;reading.value=true;error.value='';const request=detailVersion;detailController=new AbortController();
  try{const {data}=await axios.post('/api/music/query/lyrics_detail',{source:item.source,id:item.id},{signal:detailController.signal,timeout:35000});if(request!==detailVersion)return;variants.value=data.variants || [];variant.value=variants.value[0]?.value || '';}
  catch(cause){if(request===detailVersion && !axios.isCancel(cause))error.value=cause.response?.data?.message || cause.message;}
  finally{if(request===detailVersion)reading.value=false;}
}
function apply(){if(!text.value || reading.value || alignmentLoading.value || !selected.value || bilingual.value && !combined.value.matched)return;emit('apply',{text:text.value,source:`${lyricSources.find(item=>item.value===selected.value.source)?.title} · ${selected.value.title}${bilingual.value ? ' · 双语歌词' : ''}`});}
function changeSource(){album.value=null;if(kind.value==='album_id'){if(!supportsAlbumId.value)kind.value='album';changeKind();}else search();}
function changeKind(){album.value=null;query.value=kind.value==='album_id' ? '' : musicSearchTerm(props.item,kind.value);search();}
onMounted(()=>{if(query.value.trim())search();});
onBeforeUnmount(()=>{resetAlignment();version++;detailVersion++;searchController?.abort();detailController?.abort();});
</script>
<template>
  <VDialog :model-value="true" @update:model-value="!$event && emit('close')" max-width="70rem" :fullscreen="smAndDown" scrollable>
    <VCard><VCardItem title="搜索并替换歌词" class="pe-16" /><DialogCloseBtn @click="emit('close')" /><VDivider />
      <VCardText><div class="lyrics-search-controls"><VSelect :mobile-layout="false" v-model="source" :items="lyricSources" label="歌词来源" hide-details @update:model-value="changeSource" /><VTextField :mobile-layout="false" v-model="query" :label="queryLabel" clearable hide-details @keyup.enter="album=null;search()"><template #append-inner><VBtn icon="mdi-magnify" variant="text" aria-label="搜索歌词" :loading="loading" @click="album=null;search()" /></template></VTextField></div>
        <VChipGroup v-model="kind" mandatory color="primary" @update:model-value="changeKind"><VChip value="recording" filter>搜索歌曲</VChip><VChip value="album" filter>搜索专辑</VChip><VChip v-if="supportsAlbumId" value="album_id" filter>专辑 ID / 链接</VChip></VChipGroup>
        <p v-if="kind==='album_id'" class="text-caption mt-2">输入所选来源的专辑 ID，或粘贴网易云、QQ 音乐专辑链接（自动识别来源），然后选择曲目查看歌词。</p>
        <p v-if="kind==='album' && (source.startsWith('gd_') || source==='lrclib')" class="text-caption mt-2">按此来源搜索返回的歌曲归并专辑，仅展示已返回的曲目。</p>
        <div v-if="album" class="d-flex align-center flex-wrap ga-2 my-3"><VBtn v-if="kind!=='album_id'" variant="text" prepend-icon="mdi-arrow-left" @click="album=null;search()">返回专辑</VBtn><span class="text-break">{{ album.title }}</span></div>
        <p class="text-caption text-medium-emphasis mt-3 mb-4">点击带时间的歌词行，从对应位置试听当前本地歌曲。应用后将在标签编辑器中预览保存。</p>
        <VAlert v-if="error" type="error" variant="tonal" class="mb-4">{{ error }}</VAlert><VProgressLinear v-if="loading" indeterminate color="primary" />
        <VRow><VCol cols="12" md="5"><VList class="lyrics-results" aria-label="歌词搜索结果"><VListItem v-for="item in items" :key="`${item.source}:${item.id}`" :active="selected?.id===item.id" color="primary" rounded="lg" @click="choose(item)"><VListItemTitle class="lyrics-result-title">{{ item.title }}</VListItemTitle><VListItemSubtitle class="lyrics-result-subtitle">{{ [item.artists?.join(' / '),item.album,formatMusicDuration(item.duration)].filter(Boolean).join(' · ') }}</VListItemSubtitle><template #append><VIcon v-if="item.synced" icon="mdi-text-box-check-outline" size="20" title="同步歌词" /></template></VListItem></VList><p v-if="!loading && !items.length" class="pa-4 text-medium-emphasis">没有找到歌词候选，可修改关键词或切换来源。</p><div class="d-flex align-center justify-center ga-3 mt-3"><VBtn icon="mdi-chevron-left" variant="text" aria-label="歌词上一页" :disabled="page<=1 || loading" @click="page--;search(false)" /><span>{{ page }}</span><VBtn icon="mdi-chevron-right" variant="text" aria-label="歌词下一页" :disabled="!more || loading" @click="page++;search(false)" /></div></VCol>
          <VCol cols="12" md="7"><VProgressLinear v-if="reading" indeterminate color="primary" /><template v-if="variants.length"><VChipGroup v-model="variant" color="primary" mandatory :disabled="bilingual"><VChip v-for="item in variants" :key="item.value" :value="item.value" filter>{{ item.title }}</VChip></VChipGroup><VCheckbox v-if="translated" v-model="bilingual" label="使用译文制作双语歌词" hide-details /><p v-if="bilingual" class="text-body-2 my-3">保留当前原文和时间轴，已匹配 {{ combined.matched }}/{{ combined.total }} 句；未匹配的句子保留原样。</p><p v-if="bilingual" class="text-body-2 mb-3">译文有明确分句且与原文行对应时，自动逐行换行并沿用原时间点；无法确定分界时，整句保留在句段起点，可在时间轴编辑器中调整。</p><VCard v-if="bilingual && (combined.splits.length || combined.pendingSegments.length)" variant="outlined" class="pa-3 mb-3">
              <p class="text-body-2">已自动分行 {{ combined.splits.length }} 个句段，仍有 {{ combined.pendingSegments.length }} 个句段保留整句。</p>
              <template v-if="combined.pendingSegments.length"><p class="text-body-2 mt-2">仅翻译未确认分界的原文作参考，确认后仍使用来源译文与本地时间轴。</p><div class="alignment-controls mt-3"><VSelect v-model="alignmentProvider" :items="alignmentProviders" label="辅助分行翻译服务" hide-details :disabled="alignmentLoading" @update:model-value="changeAlignmentProvider" /><VBtn variant="tonal" color="primary" :loading="alignmentLoading" :disabled="!pendingReference.length" @click="alignPending">翻译未确认段落并对照</VBtn></div></template>
              <details v-if="references.length" class="alignment-reference mt-3"><summary>查看机器翻译参考</summary><p class="text-body-2 mt-2">对照后仍无明确分界的句段保持整句。</p><div v-for="row in references" :key="row.index" class="alignment-reference-row"><p>{{ props.item.lyrics.split('\n')[row.index] }}</p><p>{{ row.text }}</p></div></details>
            </VCard><VAlert v-if="bilingual && !combined.matched" type="warning" variant="tonal" class="mb-3">此版本的原文与当前歌词无法匹配，请选择其他版本。</VAlert><MusicAudioPlayer :path="props.item.path" :lyrics="text" /><VTextarea :model-value="text" label="歌词内容" readonly rows="5" class="mt-4" hide-details /></template><p v-else class="pa-4 text-medium-emphasis">选择歌曲查看歌词。</p></VCol></VRow>
        <div v-if="source.startsWith('gd_')" class="text-caption mt-4"><a href="https://music.gdstudio.xyz" target="_blank" rel="noopener noreferrer">歌词来源：GD 音乐台（music.gdstudio.xyz）</a></div>
      </VCardText><VDivider /><VCardActions class="app-dialog-actions"><VSpacer /><VBtn color="default" variant="text" @click="emit('close')">取消</VBtn><VBtn color="primary" variant="flat" :disabled="!selected || !text || loading || reading || alignmentLoading || (bilingual && !combined.matched)" @click="apply">{{ bilingual ? '应用双语歌词' : '应用歌词' }}</VBtn></VCardActions>
    </VCard>
  </VDialog>
</template>
<style scoped>
:deep(.v-chip--selected){background:var(--mp-primary-soft)!important;color:var(--mp-primary-fg)!important;}:deep(.v-chip--selected :is(.v-chip__underlay, .v-chip__overlay)){opacity:0!important;}

.lyrics-search-controls{display:flex;gap:1rem;flex-wrap:wrap;}.lyrics-search-controls>.v-input:first-child{flex:1 1 14rem;}.lyrics-search-controls>.v-input:last-child{flex:2 1 18rem;}.lyrics-results{max-block-size:30rem;overflow:auto;}.lyrics-result-title{color:rgb(var(--v-theme-on-surface));}.lyrics-result-subtitle{color:rgba(var(--v-theme-on-surface),.8);}.lyrics-result-title,.lyrics-result-subtitle{white-space:normal;overflow-wrap:anywhere;-webkit-line-clamp:unset;line-height:1.6;}.lyrics-result-subtitle{margin-block-start:.35rem;}.lyrics-results :deep(.v-list-item){padding-block:.8rem;}a{color:rgb(var(--v-theme-primary));}
.alignment-controls{display:flex;align-items:center;flex-wrap:wrap;gap:.75rem;}.alignment-controls>.v-input{flex:1 1 12rem;min-inline-size:0;}.alignment-controls>.v-btn{max-inline-size:100%;}.alignment-reference{line-height:1.8;}.alignment-reference summary{cursor:pointer;}.alignment-reference-row{padding:.75rem 0;border-block-end:1px solid rgba(var(--v-border-color),var(--v-border-opacity));overflow-wrap:anywhere;}.alignment-reference-row p{margin:0;white-space:pre-wrap;}
</style>
