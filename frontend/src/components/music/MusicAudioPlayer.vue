<script setup>
import { computed, onBeforeUnmount, ref, watch, nextTick } from 'vue';
import { parseLyrics } from './lyrics';
const props = defineProps({ path:String, lyrics:String });
const emit = defineEmits(['timeupdate']);
const audio = ref(null), lyricPanel = ref(null), time = ref(0), error = ref('');
const rows = computed(()=>parseLyrics(props.lyrics || ''));
const current = computed(()=>rows.value.findLastIndex(row=>row.time<=time.value));
const url = computed(()=>`/api/music/audio?path=${encodeURIComponent(props.path || '')}`);
let pendingSeek=null, rangeEnd=null, rangeTimer;
function clearRangeTimer(){clearTimeout(rangeTimer);}
function stop() { clearRangeTimer();rangeEnd=null;audio.value?.pause(); }
function finishRange(){const end=rangeEnd;stop();if(end!==null && audio.value){audio.value.currentTime=end;time.value=end;emit('timeupdate',end);}}
function scheduleRange(){
  clearRangeTimer();
  if(rangeEnd===null || !audio.value || audio.value.paused || pendingSeek!==null)return;
  const remaining=rangeEnd-audio.value.currentTime;
  if(remaining<=0){finishRange();return;}
  rangeTimer=setTimeout(scheduleRange,Math.max(10,remaining/audio.value.playbackRate*1000));
}
function updateTime(){
  if(rangeEnd!==null && audio.value.currentTime>=rangeEnd)finishRange();
  time.value=audio.value.currentTime;emit('timeupdate',time.value);
}
function position(){
  if(pendingSeek===null || !audio.value || !Number.isFinite(audio.value.duration))return;
  const target=Math.max(0,Math.min(pendingSeek,audio.value.duration));
  audio.value.currentTime=target;time.value=target;pendingSeek=null;
}
function seek(value, end=null) {
  if(!audio.value || !Number.isFinite(value))return;
  clearRangeTimer();rangeEnd=Number.isFinite(end) && end>value ? end : null;
  pendingSeek=value;error.value='';position();
  // 首次试听 preload=none；开始加载后在 loadedmetadata 应用所选歌词时间。
  audio.value.play()?.catch(()=>{error.value='浏览器未能开始播放，请使用播放器的播放按钮。';});
}
defineExpose({seek});
watch(current, async()=>{await nextTick();const row=lyricPanel.value?.querySelector('[aria-current="true"]');if(row) lyricPanel.value.scrollTop=row.offsetTop-lyricPanel.value.clientHeight/2+row.clientHeight/2;});
watch(()=>props.path,()=>{stop();pendingSeek=null;time.value=0;error.value='';});
onBeforeUnmount(stop);
</script>
<template>
  <div class="music-audio-player">
    <audio ref="audio" :key="path" :src="url" controls preload="none" aria-label="本地歌曲试听" @loadedmetadata="position" @timeupdate="updateTime" @playing="scheduleRange" @seeked="scheduleRange" @ratechange="scheduleRange" @pause="clearRangeTimer" @waiting="clearRangeTimer" @ended="stop" @error="stop();error='无法试听此文件，请检查目录挂载和浏览器是否支持该音频格式。'" />
    <VAlert v-if="error" type="error" variant="tonal" class="mt-2">{{ error }}</VAlert>
    <div v-if="rows.length" ref="lyricPanel" class="player-lyrics mt-4" aria-label="同步歌词">
      <button v-for="(row,index) in rows" :key="index" type="button" class="lyric-line" :class="{'text-primary lyric-line--active':index===current}" :aria-current="index===current ? 'true' : undefined" :aria-label="`跳至 ${row.time.toFixed(2)} 秒：${row.text || '间奏'}`" @click="seek(row.time)">{{ row.text || '♪' }}</button>
    </div>
    <pre v-else-if="lyrics" class="player-lyrics player-lyrics--plain mt-4">{{ lyrics }}</pre>
  </div>
</template>
<style scoped>
.music-audio-player{min-inline-size:0;}audio{display:block;inline-size:100%;min-inline-size:0;}.player-lyrics{position:relative;max-block-size:18rem;overflow:auto;padding:.75rem;border-radius:8px;background:rgba(var(--v-theme-on-surface),.035);}.lyric-line{display:block;inline-size:100%;padding:.6rem .75rem;text-align:center;white-space:normal;overflow-wrap:anywhere;line-height:1.7;border-radius:6px;}.lyric-line--active{font-weight:700;background:rgba(var(--v-theme-primary),.1);}.lyric-line:hover,.lyric-line:focus-visible{background:rgba(var(--v-theme-primary),.08);}.player-lyrics--plain{white-space:pre-wrap;overflow-wrap:anywhere;font-family:inherit;line-height:1.8;}
</style>
