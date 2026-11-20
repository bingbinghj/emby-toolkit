<script setup>
import { onMounted, onBeforeUnmount, ref } from 'vue';
import axios from 'axios';
import { useDisplay } from 'vuetify';
import DialogCloseBtn from '@/components/theme/DialogCloseBtn.vue';
import MusicAudioPlayer from './MusicAudioPlayer.vue';
import { formatMusicDuration } from './utils';
const props=defineProps({item:Object});
const emit=defineEmits(['close','edit']);
const {smAndDown}=useDisplay();
const detail=ref(null), error=ref(''), coverError=ref(false);
const controller=new AbortController();
onMounted(async()=>{try{const {data}=await axios.post('/api/music/query/inspect',{path:props.item.path},{signal:controller.signal});detail.value=data;}catch(cause){if(!controller.signal.aborted) error.value=cause.response?.data?.message || cause.message;}});
onBeforeUnmount(()=>controller.abort());
</script>
<template>
  <VDialog :model-value="true" @update:model-value="!$event && emit('close')" max-width="48rem" :fullscreen="smAndDown" scrollable>
    <VCard><VCardItem title="歌曲预览" class="pe-16" /><DialogCloseBtn @click="emit('close')" /><VDivider />
      <VCardText><VAlert v-if="error" type="error" variant="tonal">{{ error }}</VAlert><VProgressLinear v-else-if="!detail" indeterminate color="primary" />
        <template v-if="detail"><div class="song-preview-heading mb-5"><button type="button" class="song-preview-cover" aria-label="编辑封面" @click="emit('edit',{...detail,editField:'cover_url'})"><VImg v-if="!coverError" :src="`/api/music/image?path=${encodeURIComponent(item.path)}&v=${encodeURIComponent(detail.signature?.mtime || '')}`" aspect-ratio="1" cover @error="coverError=true" /><VIcon v-else icon="mdi-music-box-outline" size="72" /></button><div class="song-preview-info"><button class="text-h6" @click="emit('edit',{...detail,editField:'title'})">{{ detail.title || item.name || item.path.split('/').pop() }}</button><button @click="emit('edit',{...detail,editField:'artists'})">{{ detail.artists?.join(' / ') || '未知歌手' }}</button><button @click="emit('edit',{...detail,editField:'album'})">{{ detail.album || '未知专辑' }}</button><span class="text-caption text-medium-emphasis">{{ [detail.audio_specs,formatMusicDuration(detail.duration)].filter(Boolean).join(' · ') }}</span></div></div>
          <MusicAudioPlayer :path="item.path" :lyrics="detail.lyrics" /><div class="d-flex justify-end mt-3"><VBtn variant="text" prepend-icon="mdi-pencil-outline" @click="emit('edit',{...detail,editField:'lyrics'})">编辑歌词</VBtn></div>
        </template>
      </VCardText><VDivider /><VCardActions class="app-dialog-actions"><VSpacer /><VBtn variant="text" color="default" @click="emit('close')">关闭</VBtn><VBtn color="primary" variant="flat" :disabled="!detail" @click="emit('edit',detail)">编辑标签</VBtn></VCardActions>
    </VCard>
  </VDialog>
</template>
<style scoped>
.song-preview-heading{display:flex;gap:1.5rem;align-items:center;}.song-preview-cover{inline-size:9rem;flex:0 0 9rem;aspect-ratio:1;border-radius:8px;overflow:hidden;background:rgba(var(--v-theme-on-surface),.04);}.song-preview-info{display:flex;flex-direction:column;gap:.75rem;min-inline-size:0;}.song-preview-info button{text-align:start;overflow-wrap:anywhere;}.song-preview-info button:hover{color:rgb(var(--v-theme-primary));}@media(max-width:600px){.song-preview-heading{gap:1rem;}.song-preview-cover{inline-size:5rem;flex-basis:5rem;}}
</style>
