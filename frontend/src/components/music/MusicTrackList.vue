<!-- Adapted from MoviePilot-Frontend v3 db17d9c (MIT); see ../../../third-party/MoviePilot-LICENSE. -->
<script setup>
import { computed } from 'vue';
import { formatMusicDuration } from './utils';
const props=defineProps({tracks:{type:Array,default:()=>[]},activeMediaId:String,hideArtists:Boolean});
defineEmits(['open']);
const discs=computed(()=>{const groups=new Map();for(const track of props.tracks){const disc=track.disc_number||1;if(!groups.has(disc))groups.set(disc,[]);groups.get(disc).push(track);}return [...groups].map(([disc,tracks])=>({disc,tracks}));});
const showDiscTitles=computed(()=>discs.value.length>1);
const getTrackArtists=track=>(track.artists||[]).join(' / ');
</script>
<template>
  <div class="music-track-list">
    <div v-for="group in discs" :key="group.disc">
      <div v-if="showDiscTitles" class="music-track-disc text-caption text-medium-emphasis">
        碟片 {{ group.disc }}
      </div>
      <VList density="compact" bg-color="transparent">
        <VListItem
          v-for="(track, index) in group.tracks"
          :key="`${track.media_id || track.title}:${index}`"
          class="music-track-item"
          :class="{ 'music-track-item--active': track.media_id === props.activeMediaId }"
          :disabled="!track.media_source || !track.media_id"
          @click="$emit('open', track)"
        >
          <template #prepend>
            <span class="music-track-number text-caption text-medium-emphasis">
              {{ track.track_number || '-' }}
            </span>
          </template>
          <VListItemTitle class="music-track-title" :title="track.title">{{ track.title }}</VListItemTitle>
          <VListItemSubtitle v-if="!props.hideArtists && getTrackArtists(track)" class="text-caption">
            {{ getTrackArtists(track) }}
          </VListItemSubtitle>
          <template #append>
            <span class="text-caption text-medium-emphasis">{{ formatMusicDuration(track.duration) }}</span>
          </template>
        </VListItem>
      </VList>
    </div>
  </div>
</template>

<style scoped>
.music-track-disc {
  font-weight: 700;
  margin-block-start: 0.75rem;
  text-transform: uppercase;
}

.music-track-item {
  border-radius: var(--app-control-radius);
  cursor: pointer;
}

.music-track-item--active {
  background: rgba(var(--v-theme-primary), 0.12);
}

.music-track-number {
  display: inline-block;
  inline-size: 2rem;
  text-align: end;
  margin-inline-end: 0.75rem;
}

.music-track-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
