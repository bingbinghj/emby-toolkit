<script setup>
import { computed, ref } from 'vue';
import { useDisplay } from 'vuetify';
import DialogCloseBtn from '@/components/theme/DialogCloseBtn.vue';
import MusicAudioPlayer from './MusicAudioPlayer.vue';
import { timelineRows, serializeTimeline, formatLyricTime } from './lyrics';
const props=defineProps({path:String,lyrics:String});
const emit=defineEmits(['close','apply']);
const {smAndDown}=useDisplay();
const rows=ref([]), error=ref(''), time=ref(0), player=ref(null);
try{rows.value=timelineRows(props.lyrics || '');}catch(cause){error.value=cause.message;}
const result=computed(()=>{try{return {text:serializeTimeline(rows.value)};}catch(cause){return {error:cause.message};}});
function seek(row){
  const seconds=value=>{const [minutes,second]=value.split(':').map(Number);return minutes*60+second;};
  const start=seconds(row.time), next=rows.value.map(item=>seconds(item.time)).filter(time=>time>start);
  player.value?.seek(start,next.length ? Math.min(...next) : null);
}
</script>
<template>
  <VDialog :model-value="true" @update:model-value="!$event && emit('close')" max-width="60rem" :fullscreen="smAndDown" scrollable>
    <VCard class="lyrics-timeline"><VCardItem title="歌词时间轴编辑器" class="pe-16" /><DialogCloseBtn @click="emit('close')" /><VDivider />
      <VCardText><MusicAudioPlayer ref="player" :path="path" @timeupdate="time=$event" /><p class="mt-3 mb-4">当前播放位置 {{ formatLyricTime(time) }}。单句试听在下一句开始时暂停，最后一句播放到歌曲结束。可以逐行修改时间和歌词，或用当前播放位置给该行打点。</p>
        <VAlert v-if="error || result.error" type="error" variant="tonal" class="mb-4">{{ error || result.error }}</VAlert>
        <div v-for="(row,index) in rows" :key="index" class="timeline-row">
          <VTextField v-model="row.time" :label="`第 ${index+1} 行时间`" placeholder="00:00.000" hide-details :mobile-layout="false" />
          <VTextarea v-model="row.text" :label="`第 ${index+1} 行歌词`" rows="1" auto-grow hide-details spellcheck="false" />
          <div class="timeline-actions"><VBtn icon="mdi-play-circle-outline" variant="text" :aria-label="`试听第 ${index+1} 行`" :disabled="!row.time || !!result.error" @click="seek(row)" /><VBtn variant="tonal" :aria-label="`第 ${index+1} 行使用当前时间`" @click="row.time=formatLyricTime(time)">打点</VBtn></div>
        </div>
      </VCardText><VDivider /><VCardActions class="app-dialog-actions"><VSpacer /><VBtn color="default" variant="text" @click="emit('close')">取消</VBtn><VBtn color="primary" variant="flat" :disabled="!!error || !!result.error" @click="emit('apply',result.text)">应用时间轴</VBtn></VCardActions>
    </VCard>
  </VDialog>
</template>
<style scoped>
.lyrics-timeline :deep(.v-card-text){min-inline-size:0;}.timeline-row{display:grid;grid-template-columns:9rem minmax(0,1fr) auto;align-items:start;gap:.75rem;padding-block:.75rem;border-block-end:1px solid rgba(var(--v-border-color),var(--v-border-opacity));}.timeline-actions{display:flex;align-items:center;gap:.25rem;}.timeline-row :deep(textarea){line-height:1.7;}.timeline-row>.v-input{min-inline-size:0;}@media(max-width:600px){.timeline-row{grid-template-columns:minmax(0,1fr) auto;}.timeline-row>.v-textarea{grid-column:1/-1;grid-row:2;}.timeline-actions{grid-column:2;grid-row:1;}}
</style>
