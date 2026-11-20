<script setup>
import { computed, onMounted, onBeforeUnmount, ref, watch } from 'vue';
import axios from 'axios';
import { useDisplay } from 'vuetify';
import DialogCloseBtn from '@/components/theme/DialogCloseBtn.vue';
import MusicAudioPlayer from './MusicAudioPlayer.vue';
import { lyricSources, formatMusicDuration, musicSearchTerm } from './utils';
const props=defineProps({item:Object});
const emit=defineEmits(['close','apply']);
const {smAndDown}=useDisplay();
const source=ref('netease'), query=ref(musicSearchTerm(props.item));
const kind=ref('recording'), album=ref(null);
const albumSupported=computed(()=>['netease','qqmusic','kuwo'].includes(source.value));
const items=ref([]), selected=ref(null), variants=ref([]), variant=ref(''), page=ref(1), more=ref(false), loading=ref(false), reading=ref(false), error=ref('');
const text=computed(()=>variants.value.find(item=>item.value===variant.value)?.text || '');
let searchController, detailController, version=0, detailVersion=0;
function resetDetail(){detailVersion++;detailController?.abort();selected.value=null;variants.value=[];variant.value='';reading.value=false;}
async function search(reset=true) {
  if(reset) page.value=1;
  const request=++version;
  searchController?.abort();searchController=new AbortController();resetDetail();loading.value=true;error.value='';items.value=[];
  try{const {data}=await axios.post('/api/music/query/lyrics_search',{source:source.value,query:query.value,page:page.value,kind:kind.value,album_id:album.value?.id},{signal:searchController.signal,timeout:35000});if(request!==version)return;items.value=data.items || [];more.value=!!data.has_more;}
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
function apply(){if(!text.value || reading.value)return;emit('apply',{text:text.value,source:`${lyricSources.find(item=>item.value===source.value)?.title} · ${selected.value.title}`});}
watch(source,()=>{album.value=null;if(!albumSupported.value)kind.value='recording';search();});
function changeKind(){album.value=null;query.value=musicSearchTerm(props.item,kind.value);search();}
onMounted(()=>{if(query.value.trim())search();});
onBeforeUnmount(()=>{version++;detailVersion++;searchController?.abort();detailController?.abort();});
</script>
<template>
  <VDialog :model-value="true" @update:model-value="!$event && emit('close')" max-width="70rem" :fullscreen="smAndDown" scrollable>
    <VCard><VCardItem title="搜索并替换歌词" class="pe-16" /><DialogCloseBtn @click="emit('close')" /><VDivider />
      <VCardText><div class="lyrics-search-controls"><VSelect :mobile-layout="false" v-model="source" :items="lyricSources" label="歌词来源" hide-details /><VTextField :mobile-layout="false" v-model="query" :label="kind==='album' ? '搜索专辑或歌手' : '搜索歌曲或歌手'" clearable hide-details @keyup.enter="album=null;search()"><template #append-inner><VBtn icon="mdi-magnify" variant="text" aria-label="搜索歌词" :loading="loading" @click="album=null;search()" /></template></VTextField></div>
        <VChipGroup v-model="kind" mandatory color="primary" @update:model-value="changeKind"><VChip value="recording" filter>搜索歌曲</VChip><VChip value="album" filter :disabled="!albumSupported">搜索专辑</VChip></VChipGroup>
        <div v-if="album" class="d-flex align-center ga-2 mb-3"><VBtn variant="text" prepend-icon="mdi-arrow-left" @click="album=null;search()">返回专辑</VBtn><span>{{ album.title }}</span></div>
        <p class="text-caption text-medium-emphasis mt-3 mb-4">点击带时间的歌词行，从对应位置试听当前本地歌曲。应用后将在标签编辑器中预览保存。</p>
        <VAlert v-if="error" type="error" variant="tonal" class="mb-4">{{ error }}</VAlert><VProgressLinear v-if="loading" indeterminate color="primary" />
        <VRow><VCol cols="12" md="5"><VList class="lyrics-results" aria-label="歌词搜索结果"><VListItem v-for="item in items" :key="`${item.source}:${item.id}`" :active="selected?.id===item.id" color="primary" rounded="lg" @click="choose(item)"><VListItemTitle class="lyrics-result-title">{{ item.title }}</VListItemTitle><VListItemSubtitle class="lyrics-result-subtitle">{{ [item.artists?.join(' / '),item.album,formatMusicDuration(item.duration)].filter(Boolean).join(' · ') }}</VListItemSubtitle><template #append><VIcon v-if="item.synced" icon="mdi-text-box-check-outline" size="20" title="同步歌词" /></template></VListItem></VList><p v-if="!loading && !items.length" class="pa-4 text-medium-emphasis">没有找到歌词候选，可修改关键词或切换来源。</p><div class="d-flex align-center justify-center ga-3 mt-3"><VBtn icon="mdi-chevron-left" variant="text" aria-label="歌词上一页" :disabled="page<=1 || loading" @click="page--;search(false)" /><span>{{ page }}</span><VBtn icon="mdi-chevron-right" variant="text" aria-label="歌词下一页" :disabled="!more || loading" @click="page++;search(false)" /></div></VCol>
          <VCol cols="12" md="7"><VProgressLinear v-if="reading" indeterminate color="primary" /><template v-if="text"><VChipGroup v-model="variant" color="primary" mandatory><VChip v-for="item in variants" :key="item.value" :value="item.value" filter>{{ item.title }}</VChip></VChipGroup><MusicAudioPlayer :path="props.item.path" :lyrics="text" /><VTextarea :model-value="text" label="歌词内容" readonly rows="5" class="mt-4" hide-details /></template><p v-else class="pa-4 text-medium-emphasis">选择歌曲查看歌词。</p></VCol></VRow>
        <div v-if="source.startsWith('gd_')" class="text-caption mt-4"><a href="https://music.gdstudio.xyz" target="_blank" rel="noopener noreferrer">歌词来源：GD 音乐台（music.gdstudio.xyz）</a></div>
      </VCardText><VDivider /><VCardActions class="app-dialog-actions"><VSpacer /><VBtn color="default" variant="text" @click="emit('close')">取消</VBtn><VBtn color="primary" variant="flat" :disabled="!text || loading || reading" @click="apply">应用歌词</VBtn></VCardActions>
    </VCard>
  </VDialog>
</template>
<style scoped>
:deep(.v-chip--selected){background:rgb(var(--v-theme-primary))!important;color:rgb(var(--v-theme-on-primary))!important;}:deep(.v-chip--selected :is(.v-chip__underlay, .v-chip__overlay)){opacity:0!important;}

.lyrics-search-controls{display:flex;gap:1rem;flex-wrap:wrap;}.lyrics-search-controls>.v-input:first-child{flex:1 1 14rem;}.lyrics-search-controls>.v-input:last-child{flex:2 1 18rem;}.lyrics-results{max-block-size:30rem;overflow:auto;}.lyrics-result-title{color:rgb(var(--v-theme-on-surface));}.lyrics-result-subtitle{color:rgba(var(--v-theme-on-surface),.8);}.lyrics-result-title,.lyrics-result-subtitle{white-space:normal;overflow-wrap:anywhere;-webkit-line-clamp:unset;line-height:1.6;}.lyrics-result-subtitle{margin-block-start:.35rem;}.lyrics-results :deep(.v-list-item){padding-block:.8rem;}a{color:rgb(var(--v-theme-primary));}
</style>
