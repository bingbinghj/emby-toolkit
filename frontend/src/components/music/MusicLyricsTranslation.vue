<script setup>
import { onBeforeUnmount, ref } from 'vue';
import axios from 'axios';
import { useDisplay } from 'vuetify';
import DialogCloseBtn from '@/components/theme/DialogCloseBtn.vue';
const props=defineProps({lyrics:String});
const emit=defineEmits(['close','apply']);
const {smAndDown}=useDisplay();
const providers=[{title:'有道翻译',value:'youdao'},{title:'必应翻译',value:'bing'},{title:'谷歌翻译',value:'google'}];
const provider=ref('youdao'), result=ref(null), loading=ref(false), error=ref('');
let controller, version=0;
function reset(){version++;controller?.abort();loading.value=false;result.value=null;error.value='';}
async function translate(){
  reset();const current=version;controller=new AbortController();loading.value=true;
  try{const {data}=await axios.post('/api/music/query/lyrics_translate',{lyrics:props.lyrics,provider:provider.value},{signal:controller.signal,timeout:120000});if(current===version)result.value=data;}
  catch(cause){if(current===version && !axios.isCancel(cause))error.value=cause.response?.data?.message || cause.message;}
  finally{if(current===version)loading.value=false;}
}
onBeforeUnmount(reset);
</script>
<template>
  <VDialog :model-value="true" @update:model-value="!$event && emit('close')" max-width="52rem" :fullscreen="smAndDown" scrollable>
    <VCard class="lyrics-translation"><VCardItem title="歌词机器翻译" subtitle="自动识别语言，翻译为简体中文" class="pe-16" /><DialogCloseBtn @click="emit('close')" /><VDivider />
      <VCardText><div class="translation-toolbar"><VChipGroup v-model="provider" column mandatory color="primary" @update:model-value="reset"><VChip v-for="item in providers" :key="item.value" :value="item.value" filter>{{ item.title }}</VChip></VChipGroup><VBtn color="primary" variant="tonal" prepend-icon="mdi-translate" :loading="loading" @click="translate">开始翻译</VBtn></div>
        <VAlert v-if="error" type="error" variant="tonal" class="my-4">{{ error }}</VAlert>
        <section class="translation-block mt-4"><h3>原文</h3><pre aria-label="歌词原文">{{ lyrics }}</pre></section>
        <section class="translation-block mt-4"><h3>译文<VChip v-if="result" size="small" variant="tonal">{{ providers.find(item=>item.value===result.provider)?.title }}</VChip></h3><VProgressLinear v-if="loading" indeterminate color="primary" /><pre v-if="result" aria-label="歌词译文">{{ result.translation }}</pre><p v-else class="pa-4">{{ loading ? '正在翻译歌词…' : '选择翻译服务后开始翻译。' }}</p></section>
      </VCardText><VDivider /><VCardActions class="app-dialog-actions"><VSpacer /><VBtn color="default" variant="text" @click="emit('close')">取消</VBtn><VBtn color="primary" variant="tonal" :disabled="!result || loading" @click="emit('apply',result.translation)">应用译文</VBtn><VBtn color="primary" variant="flat" :disabled="!result || loading" @click="emit('apply',result.bilingual)">应用双语歌词</VBtn></VCardActions>
    </VCard>
  </VDialog>
</template>
<style scoped>
.translation-toolbar{display:flex;align-items:center;gap:.75rem;flex-wrap:wrap;}.translation-toolbar>.v-chip-group{flex:1 1 22rem;min-inline-size:0;}.translation-block{border:1px solid rgba(var(--v-border-color),var(--v-border-opacity));border-radius:10px;overflow:hidden;}.translation-block h3{display:flex;align-items:center;gap:.75rem;flex-wrap:wrap;font-size:1rem;padding:.75rem 1rem;background:rgba(var(--v-theme-primary),.07);}.translation-block pre{white-space:pre-wrap;overflow-wrap:anywhere;font-family:inherit;line-height:1.9;padding:1rem;max-block-size:20rem;overflow:auto;color:rgb(var(--v-theme-on-surface));}.lyrics-translation :deep(.v-card-text){min-inline-size:0;}:deep(.v-chip--selected){background:var(--mp-primary-soft)!important;color:var(--mp-primary-fg)!important;}:deep(.v-chip--selected :is(.v-chip__underlay,.v-chip__overlay)){opacity:0!important;}:deep(.v-chip--selected .v-chip__content){color:inherit!important;}
</style>
