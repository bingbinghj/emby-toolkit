<!-- Adapted from MoviePilot-Frontend v3 cf02b12 (MIT); see ../../../third-party/MoviePilot-LICENSE. -->
<script setup>
import {computed, ref, watch} from 'vue';
const props=defineProps({artist:Object,width:String});
const emit=defineEmits(['open']);
const imageLoadError=ref(false), isImageLoaded=ref(false);
const imageUrl=computed(()=>props.artist?.image_url || props.artist?.poster_path || '');
const showImage=computed(()=>Boolean(imageUrl.value) && !imageLoadError.value);
const subtitle=computed(()=>[props.artist?.relation,props.artist?.artist_type,props.artist?.disambiguation].filter(Boolean).join(' · '));
watch(imageUrl,()=>{imageLoadError.value=false;isImageLoaded.value=false;});
function goArtistDetail(){if(props.artist?.media_id && props.artist.media_source)emit('open',{...props.artist,music_type:'artist'});}
</script>
<template>
  <VHover>
    <template #default="hover">
      <div v-bind="hover.props" class="music-artist-card-hover-area">
        <VCard
          :width="props.width"
          class="app-hover-lift-card"
          :class="{ 'app-hover-lift-card--hovering': hover.isHovering }"
          @click.stop="goArtistDetail"
        >
          <div class="music-artist-card relative cursor-pointer">
            <div style="padding-block-end: 150%">
              <div class="absolute inset-0 flex h-full w-full flex-col items-center p-2">
                <div class="relative mt-2 mb-4 flex h-1/2 w-full justify-center">
                  <VAvatar size="100" class="music-artist-card-avatar" :class="{'music-artist-card-avatar--loaded':showImage && isImageLoaded}">
                    <VImg v-if="showImage" :src="imageUrl" cover @load="isImageLoaded=true" @error="imageLoadError = true" />
                    <VIcon v-else icon="mdi-account-music" size="48" color="medium-emphasis" />
                  </VAvatar>
                </div>
                <div class="music-artist-card-kind text-caption">{{ '艺术家' }}</div>
                <div class="music-artist-card-name w-full truncate text-center font-semibold">{{ props.artist?.name }}</div>
                <div class="music-artist-card-subtitle overflow-hidden whitespace-normal text-center text-sm text-ellipsis line-clamp-2">
                  {{ subtitle }}
                </div>
              </div>
            </div>
          </div>
        </VCard>
      </div>
    </template>
  </VHover>
</template>

<style scoped>
.music-artist-card-hover-area {
  inline-size: 100%;
}

.music-artist-card {
  transition: background-color 0.2s ease;
}

.music-artist-card:hover {
  background-color: var(--mp-muted);
}

.music-artist-card-avatar {
  background: var(--mp-muted);
}

.music-artist-card-avatar--loaded {
  border: 1px solid var(--mp-border);
}

.music-artist-card-kind {
  color: var(--mp-faint);
}

.music-artist-card-name {
  color: var(--mp-fg);
}

.music-artist-card-subtitle {
  color: var(--mp-muted-fg);
}
</style>
