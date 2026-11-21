<!-- 发行选项沿用 MP music-transfer/CorrectionDialog，复用现有专辑详情接口。 -->
<script setup>
import { ref, watch, onBeforeUnmount } from 'vue';
import axios from 'axios';
import { musicReleaseOptions } from './utils';
const props=defineProps({albumId:String,disabled:Boolean});
const selected=defineModel({default:''});
const emit=defineEmits(['loading','album','error']);
const items=ref([]), loading=ref(false), error=ref('');
let version=0, controller;
async function load(release){
  const request=++version, id=props.albumId?.trim().toLowerCase();
  controller?.abort();if(!release)items.value=[];error.value='';selected.value='';emit('album',null);
  loading.value=false;emit('loading',false);
  if(!/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(id || ''))return;
  controller=new AbortController();loading.value=true;emit('loading',true);
  try{
    const {data}=await axios.post('/api/music/query/detail',{source:'musicbrainz',id,kind:'album',...(release ? {release} : {})},{signal:controller.signal,timeout:190000});
    if(request!==version)return;
    if(data.media_id!==id || data.media_source!=='musicbrainz' || release && data.musicbrainz_release_id!==release)throw new Error('未取得当前专辑的发行版本，请重试');
    if(data.releases?.length || !items.value.length)items.value=musicReleaseOptions(data);
    if(!items.value.length)throw new Error('此专辑暂未提供可选发行版本');
    selected.value=data.musicbrainz_release_id || '';
    emit('album',data);
  }catch(cause){if(request===version && !axios.isCancel(cause))error.value=cause.response?.data?.message || cause.response?.data?.error || cause.message;}
  finally{if(request===version){loading.value=false;emit('loading',false);}}
}
watch(()=>props.albumId,()=>load(),{immediate:true});
watch(error,value=>emit('error',value));
onBeforeUnmount(()=>{version++;controller?.abort();emit('loading',false);emit('error','');});
</script>
<template>
  <div>
  <VSelect :model-value="selected" :items="[{value:'',title:'按发行偏好自动选择'},...items]" label="具体发行版本" prepend-inner-icon="mdi-album" :loading="loading" :disabled="disabled || loading || !items.length" :error-messages="error" hint="选择专辑后，可按发行日期、地区、介质和曲目数区分版本。" persistent-hint class="music-release-select" @update:model-value="load($event)">
    <template #item="{props:itemProps}"><VListItem v-bind="itemProps" role="option" class="music-release-option" /></template>
  </VSelect>
  <VBtn v-if="error" prepend-icon="mdi-refresh" variant="text" :disabled="disabled" @click="load()">重新读取发行版本</VBtn>
  </div>
</template>
<style scoped>
.music-release-select :deep(.v-select__selection-text){white-space:normal;overflow-wrap:anywhere;line-height:1.5;}.music-release-option :deep(.v-list-item-title){white-space:normal;overflow-wrap:anywhere;line-height:1.5;}
</style>
