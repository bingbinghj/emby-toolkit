<script setup>
import { onBeforeUnmount, ref, watch } from 'vue';
import axios from 'axios';
import { useDisplay } from 'vuetify';
import DialogCloseBtn from '@/components/theme/DialogCloseBtn.vue';
import { lyricTranslationRows } from './lyrics';
const props=defineProps({lyrics:String});
const emit=defineEmits(['close','apply']);
const {smAndDown}=useDisplay();
const providers=[{title:'有道翻译',value:'youdao'},{title:'必应翻译',value:'bing'},{title:'谷歌翻译',value:'google'}];
const provider=ref('youdao'), result=ref(null), loading=ref(false), error=ref('');
const rows=lyricTranslationRows(props.lyrics);
const selected=ref(rows.filter(row=>row.foreign).map(row=>row.index));
let controller, version=0;
function reset(){version++;controller?.abort();loading.value=false;result.value=null;error.value='';}
async function translate(){
  if(!selected.value.length)return;
  reset();const current=version;controller=new AbortController();loading.value=true;
  try{const {data}=await axios.post('/api/music/query/lyrics_translate',{lyrics:props.lyrics,provider:provider.value,line_indices:selected.value},{signal:controller.signal,timeout:120000});if(current===version)result.value=data;}
  catch(cause){if(current===version && !axios.isCancel(cause))error.value=cause.response?.data?.message || cause.message;}
  finally{if(current===version)loading.value=false;}
}
watch(selected,reset,{deep:true,flush:'sync'});
onBeforeUnmount(reset);
</script>
<template>
  <VDialog :model-value="true" @update:model-value="!$event && emit('close')" max-width="52rem" :fullscreen="smAndDown" scrollable>
    <VCard class="lyrics-translation"><VCardItem title="歌词机器翻译" subtitle="自动识别语言，翻译为简体中文" class="pe-16" /><DialogCloseBtn @click="emit('close')" /><VDivider />
      <VCardText><div class="translation-toolbar"><VChipGroup v-model="provider" column mandatory color="primary" @update:model-value="reset"><VChip v-for="item in providers" :key="item.value" :value="item.value" filter>{{ item.title }}</VChip></VChipGroup><VBtn color="primary" variant="tonal" prepend-icon="mdi-translate" :loading="loading" :disabled="!selected.length" @click="translate">开始翻译</VBtn></div>
        <VAlert v-if="error" type="error" variant="tonal" class="my-4">{{ error }}</VAlert>
        <p class="text-body-2 mt-4">勾选要翻译的歌词，可只选某一段。默认选中含外语的行，未选行保留原文与时间轴；汉字行也可手动勾选。</p>
        <section class="translation-block mt-4"><h3>原文<span class="text-body-2">已选 {{ selected.length }} 行</span></h3>
          <div class="translation-selection-actions"><VBtn size="small" variant="text" @click="selected=rows.filter(row=>row.foreign).map(row=>row.index)">仅选外语行</VBtn><VBtn size="small" variant="text" @click="selected=rows.filter(row=>row.selectable).map(row=>row.index)">全选</VBtn><VBtn size="small" variant="text" @click="selected=[]">取消全选</VBtn></div>
          <div class="translation-original" aria-label="歌词原文"><template v-for="row in rows" :key="row.index"><VCheckbox v-if="row.selectable" v-model="selected" :value="row.index" :label="row.text" :aria-label="`翻译第 ${row.index+1} 行：${row.text}`" hide-details color="primary" /><p v-else class="translation-metadata">{{ row.text }}</p></template></div>
        </section>
        <section class="translation-block mt-4"><h3>译文<VChip v-if="result" size="small" variant="tonal">{{ providers.find(item=>item.value===result.provider)?.title }}</VChip></h3><VProgressLinear v-if="loading" indeterminate color="primary" /><pre v-if="result" aria-label="歌词译文">{{ result.translation }}</pre><p v-else class="pa-4">{{ loading ? '正在翻译歌词…' : '选择翻译服务后开始翻译。' }}</p></section>
      </VCardText><VDivider /><VCardActions class="app-dialog-actions"><VSpacer /><VBtn color="default" variant="text" @click="emit('close')">取消</VBtn><VBtn color="primary" variant="tonal" :disabled="!result || loading" @click="emit('apply',result.translation)">应用译文</VBtn><VBtn color="primary" variant="flat" :disabled="!result || loading" @click="emit('apply',result.bilingual)">应用双语歌词</VBtn></VCardActions>
    </VCard>
  </VDialog>
</template>
<style scoped>
.translation-toolbar{display:flex;align-items:center;gap:.75rem;flex-wrap:wrap;}.translation-toolbar>.v-chip-group{flex:1 1 22rem;min-inline-size:0;}.translation-block{border:1px solid rgba(var(--v-border-color),var(--v-border-opacity));border-radius:10px;overflow:hidden;}.translation-block h3{display:flex;align-items:center;gap:.75rem;flex-wrap:wrap;font-size:1rem;padding:.75rem 1rem;background:rgba(var(--v-theme-primary),.07);}.translation-block pre{white-space:pre-wrap;overflow-wrap:anywhere;font-family:inherit;line-height:1.9;padding:1rem;max-block-size:20rem;overflow:auto;color:rgb(var(--v-theme-on-surface));}.lyrics-translation :deep(.v-card-text){min-inline-size:0;}:deep(.v-chip--selected){background:var(--mp-primary-soft)!important;color:var(--mp-primary-fg)!important;}:deep(.v-chip--selected :is(.v-chip__underlay,.v-chip__overlay)){opacity:0!important;}:deep(.v-chip--selected .v-chip__content){color:inherit!important;}
.translation-selection-actions{display:flex;flex-wrap:wrap;gap:.25rem;padding:.5rem;}
.translation-original{max-block-size:20rem;overflow:auto;padding:.5rem 1rem;min-inline-size:0;}
.translation-original :deep(.v-label){white-space:pre-wrap;overflow-wrap:anywhere;line-height:1.8;opacity:1;min-inline-size:0;color:rgb(var(--v-theme-on-surface));}
.translation-original :deep(.v-selection-control){align-items:flex-start;}
.translation-original :deep(.v-selection-control__input){flex-shrink:0;}
.translation-original :deep(.v-selection-control .v-label){padding-block:.6rem;}
.translation-metadata{white-space:pre-wrap;overflow-wrap:anywhere;line-height:1.8;color:rgb(var(--v-theme-on-surface));}
</style>
