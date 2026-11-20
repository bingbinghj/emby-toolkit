<script setup>
import { computed } from 'vue';
import { formatMusicDuration } from './utils';
const props = defineProps({ items:Array, directory:String, loading:Boolean, busy:Boolean, searching:Boolean });
const emit = defineEmits(['edit','preview']);
const fields = [['title','标题'],['artists','艺术家'],['album','专辑'],['release_year','年份']];
const tracks = computed(() => props.loading ? [] : props.items || []);
function value(item, field) {
  if (field === 'title') return item.title || item.name || item.path.split('/').pop();
  if (field === 'artists') return item.artists?.join(' / ') || '添加艺术家';
  if (field === 'release_year') return item.release_year || item.year || '添加年份';
  return item.album || '添加专辑';
}
</script>
<template>
  <VCard class="music-folder-tracks" data-testid="music-folder-tracks">
    <VCardItem :title="searching ? '搜索结果' : directory?.split('/').pop() || '当前文件夹'" :subtitle="loading ? '读取歌曲信息…' : `${tracks.length} 首歌曲`" />
    <VDivider /><VProgressLinear v-if="loading" indeterminate color="primary" />
    <div v-if="tracks.length" class="folder-tracks-content">
      <div class="folder-track-columns" aria-hidden="true"><span>封面</span><span v-for="[,label] in fields" :key="label">{{ label }}</span><span>歌词 / 试听</span></div>
      <article v-for="item in tracks" :key="item.path" class="folder-track" :aria-label="item.name || item.title">
        <button class="folder-track-cover" type="button" :aria-label="`编辑封面：${value(item,'title')}`" :disabled="busy" @click="emit('edit',{...item,editField:'cover_url'})">
          <VImg :src="`/api/music/image?path=${encodeURIComponent(item.path)}&v=${encodeURIComponent(item.signature?.mtime || '')}`" aspect-ratio="1" cover><template #error><VIcon icon="mdi-music-box-outline" size="40" /></template></VImg>
        </button>
        <div v-for="[field,label] in fields" :key="field" class="folder-track-cell" :data-label="label">
          <button type="button" :disabled="busy" :aria-label="`编辑${label}：${value(item,'title')}`" @click="emit('edit',{...item,editField:field})">{{ value(item,field) }}</button>
          <small v-if="field==='title'" class="text-medium-emphasis">{{ [item.audio_specs,formatMusicDuration(item.duration)].filter(Boolean).join(' · ') }}</small>
        </div>
        <div class="folder-track-actions"><VBtn variant="text" size="small" :disabled="busy" @click="emit('edit',{...item,editField:'lyrics'})">歌词</VBtn><VBtn icon="mdi-play-circle-outline" variant="text" :aria-label="`试听 ${value(item,'title')}`" @click="emit('preview',item)" /></div>
        <VAlert v-if="item.error" type="error" variant="tonal" class="folder-track-error">{{ item.error }}</VAlert>
      </article>
    </div>
    <p v-else-if="!loading" class="pa-6 text-medium-emphasis">进入专辑或文件夹后，这里会直接显示其中的全部歌曲。</p>
  </VCard>
</template>
<style scoped>
.music-folder-tracks{min-inline-size:0;container-type:inline-size;}.music-folder-tracks :deep(.v-card-title){white-space:normal;overflow-wrap:anywhere;}.folder-tracks-content{padding:0 1rem 1rem;}.folder-track-columns,.folder-track{display:grid;grid-template-columns:4rem minmax(0,1.5fr) minmax(0,1fr) minmax(0,1fr) 4rem 6rem;gap:1rem;align-items:center;}.folder-track-columns{padding-block:1rem;font-size:.875rem;color:rgba(var(--v-theme-on-surface),.75);}.folder-track{padding-block:1rem;border-block-start:1px solid rgba(var(--v-border-color),var(--v-border-opacity));}.folder-track-cover{inline-size:4rem;aspect-ratio:1;border-radius:8px;overflow:hidden;background:rgba(var(--v-theme-on-surface),.05);}.folder-track-cover :deep(.v-img__error){display:grid;place-items:center;}.folder-track-cell{min-inline-size:0;}.folder-track-cell button{display:block;color:rgb(var(--v-theme-on-surface));text-align:start;white-space:normal;overflow-wrap:anywhere;line-height:1.7;}.folder-track-cell button:hover{color:rgb(var(--v-theme-primary));}.folder-track-cell small{display:block;margin-block-start:.35rem;overflow-wrap:anywhere;}.folder-track button:focus-visible{outline:2px solid rgb(var(--v-theme-primary));outline-offset:3px;}.folder-track-actions{display:flex;align-items:center;}.folder-track-error{grid-column:1/-1;}
@container(max-width:660px){.folder-track-columns{display:none;}.folder-track{grid-template-columns:4rem minmax(0,1fr);}.folder-track-cover{grid-row:span 4;align-self:start;}.folder-track-cell{grid-column:2;}.folder-track-cell::before{content:attr(data-label);font-size:.75rem;color:rgba(var(--v-theme-on-surface),.7);}.folder-track-actions{grid-column:2;}}
</style>
