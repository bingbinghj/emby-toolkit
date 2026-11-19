<!-- Adapted from MoviePilot-Frontend v3 db17d9c (MIT); see ../../../third-party/MoviePilot-LICENSE. -->
<script setup>
import { computed, ref, watch } from 'vue';
import { useMusicI18n as useI18n } from './i18n';
import { formatMusicDuration, sourceLabel as label } from './utils';
const { t }=useI18n();
const props=defineProps({music:Object});
const emit=defineEmits(['open','artist','album']);
const imageLoadError=ref(false);
const artistLinks=computed(()=>(props.music?.artists||[]).map((name,index)=>({name,id:props.music?.artist_ids?.[index]})));
const sourceLabel=computed(()=>label(props.music?.media_source));
const sourceMeta=computed(()=>({color:props.music?.media_source==='musicbrainz'?'#eb743b':props.music?.media_source==='doubanmusic'?'#00b51d':'#35a7a0',icon:props.music?.media_source==='theaudiodb'?'mdi-music-box-multiple':'mdi-music-circle'}));
const entityMeta=computed(()=>({album:{icon:'mdi-album',label:'专辑'},artist:{icon:'mdi-account-music',label:'艺术家'},recording:{icon:'mdi-music-note',label:'单曲'}}[props.music?.music_type||'recording']));
const metaItems=computed(()=>[{hideOnNarrow:true,icon:'mdi-label-outline',label:props.music?.metadata_category||props.music?.album_type},{icon:'mdi-calendar-blank-outline',label:props.music?.release_date||props.music?.year},{icon:'mdi-clock-outline',label:formatMusicDuration(props.music?.duration)},{icon:'mdi-waveform',label:props.music?.audio_specs},{hideOnNarrow:true,icon:'mdi-counter',label:props.music?.track_number ? `第 ${props.music.track_number} 首` : ''},{hideOnNarrow:true,icon:'mdi-chart-line',label:props.music?.listen_count ? `${props.music.listen_count.toLocaleString()} 次播放` : ''}].filter(item=>item.label));
const coverUrl=computed(()=>props.music?.cover_url||props.music?.image_url||props.music?.poster_path||'');
const showCover=computed(()=>Boolean(coverUrl.value)&&!imageLoadError.value);
watch(coverUrl,()=>{imageLoadError.value=false;});
const goDetail=()=>emit('open',props.music);
const goAlbum=()=>emit('album',{...props.music,media_id:props.music.album_id,music_type:'album'});
const goArtist=(id,name)=>emit('artist',{media_source:props.music.media_source,media_id:id,music_type:'artist',title:name});
</script>
<template>
  <VHover>
    <template #default="hover">
      <div v-bind="hover.props" class="music-card-hover-area h-100">
        <VCard
          class="music-card app-hover-lift-card h-100 cursor-pointer"
          :class="{ 'app-hover-lift-card--hovering': hover.isHovering }"
          @click="goDetail"
        >
          <div class="music-card-content">
            <div class="music-card-cover-column">
              <div class="music-card-cover-shell">
                <VImg v-if="showCover" :src="coverUrl" cover class="music-card-cover" @error="imageLoadError = true">
                  <template #placeholder>
                    <VSkeletonLoader class="h-100" />
                  </template>
                </VImg>
                <VIcon v-else :icon="entityMeta.icon" size="44" color="medium-emphasis" />

                <VChip :prepend-icon="entityMeta.icon" size="x-small" variant="flat" class="music-card-entity">
                  {{ entityMeta.label }}
                </VChip>
              </div>
              <div class="music-card-source-row">
                <VChip
                  data-testid="music-source"
                  :color="sourceMeta.color"
                  :prepend-icon="sourceMeta.icon"
                  class="music-card-source"
                  size="x-small"
                  variant="tonal"
                >
                  {{ sourceLabel }}
                </VChip>
              </div>
            </div>

            <div class="music-card-body">
              <div class="music-card-heading">
                <div class="music-card-title" :title="props.music?.title">{{ props.music?.title || props.music?.name }}</div>
                <VChip
                  v-if="props.music?.version"
                  :title="props.music.version"
                  size="x-small"
                  variant="tonal"
                  class="music-card-version"
                >
                  {{ props.music.version }}
                </VChip>
              </div>

              <div v-if="props.music?.music_type !== 'artist'" class="music-card-supporting text-medium-emphasis">
                <VIcon icon="mdi-account-music" size="16" />
                <div class="music-card-artists">
                  <template v-if="artistLinks.length">
                    <template v-for="(artist, index) in artistLinks" :key="`${artist.name}-${index}`">
                      <span v-if="index > 0"> / </span>
                      <a
                        v-if="artist.id"
                        class="music-card-link"
                        role="link"
                        tabindex="0"
                        @click.stop="goArtist(artist.id, artist.name)"
                        @keydown.enter.stop="goArtist(artist.id, artist.name)"
                        >{{ artist.name }}</a
                      >
                      <span v-else>{{ artist.name }}</span>
                    </template>
                  </template>
                  <span v-else>{{ t('common.unknown') }}</span>
                </div>
              </div>

              <div
                v-if="props.music?.album && props.music?.music_type !== 'album'"
                class="music-card-supporting text-medium-emphasis"
              >
                <VIcon icon="mdi-album" size="16" />
                <div class="music-card-album">
                  <span>{{ t('music.album') }}：</span>
                  <a
                    v-if="props.music.album_id"
                    class="music-card-link"
                    role="link"
                    tabindex="0"
                    @click.stop="goAlbum"
                    @keydown.enter.stop="goAlbum"
                    >{{ props.music.album }}</a
                  >
                  <span v-else>{{ props.music.album }}</span>
                </div>
              </div>

              <div class="music-card-footer">
                <div class="music-card-meta">
                  <VChip
                    v-for="item in metaItems"
                    :key="`${item.icon}-${item.label}`"
                    :prepend-icon="item.icon"
                    :class="{ 'music-card-meta-item--narrow-optional': item.hideOnNarrow }"
                    size="x-small"
                    variant="tonal"
                  >
                    {{ item.label }}
                  </VChip>
                </div>

                <div class="music-card-actions"><slot name="actions" /></div>

              </div>
            </div>
          </div>
        </VCard>
      </div>
    </template>
  </VHover>
</template>

<style scoped>
.music-card-hover-area {
  inline-size: 100%;
}

.music-card {
  min-block-size: 144px;
  overflow: hidden;
}

.music-card-content {
  display: grid;
  block-size: 100%;
  gap: 1rem;
  grid-template-columns: 112px minmax(0, 1fr);
  padding: 1rem;
}

.music-card-cover-column {
  display: flex;
  flex-direction: column;
  gap: 0.375rem;
  inline-size: 112px;
  min-inline-size: 0;
}

.music-card-cover-shell {
  position: relative;
  display: flex;
  overflow: hidden;
  align-items: center;
  align-self: start;
  justify-content: center;
  aspect-ratio: 1;
  background: rgba(var(--v-theme-on-surface), 0.06);
  border: 1px solid rgba(var(--v-theme-on-surface), 0.1);
  border-radius: 8px;
  inline-size: 112px;
}

.music-card-cover {
  block-size: 100%;
  inline-size: 100%;
  transition: transform 0.3s ease;
}

.music-card.app-hover-lift-card--hovering .music-card-cover {
  transform: scale(1.035);
}

.music-card-entity {
  position: absolute;
  z-index: 1;
  background: rgba(var(--v-theme-surface), 0.88) !important;
  color: rgb(var(--v-theme-on-surface)) !important;
  inset-block-end: 0.5rem;
  inset-inline-start: 0.5rem;
  max-inline-size: calc(100% - 1rem);
}

.music-card-body {
  display: flex;
  flex-direction: column;
  min-inline-size: 0;
}

.music-card-source-row {
  display: flex;
  align-items: center;
  justify-content: center;
  min-block-size: 20px;
  min-inline-size: 0;
}

.music-card-source {
  max-inline-size: 100%;
  min-inline-size: 0;
}

.music-card-heading {
  display: flex;
  align-items: flex-start;
  gap: 0.5rem;
}

.music-card-title {
  overflow: hidden;
  display: -webkit-box;
  color: rgb(var(--v-theme-on-surface));
  font-size: 1rem;
  flex: 1 1 auto;
  font-weight: 600;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  letter-spacing: 0;
  line-height: 1.4;
  min-inline-size: 0;
}

.music-card-version {
  flex: 0 1 auto;
  margin-block-start: 0.125rem;
  max-inline-size: min(45%, 12rem);
  min-inline-size: 0;
}

.music-card-source :deep(.v-chip__content),
.music-card-version :deep(.v-chip__content) {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.music-card-supporting {
  display: flex;
  align-items: center;
  font-size: 0.8125rem;
  gap: 0.375rem;
  line-height: 1.35;
  margin-block-start: 0.35rem;
}

.music-card-artists,
.music-card-album {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.music-card-link {
  color: inherit;
  cursor: pointer;
  font-weight: 500;
  text-decoration: none;
  transition: color 0.2s ease;
}

.music-card-link:hover {
  color: rgb(var(--v-theme-primary));
}

.music-card-footer {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 0.75rem;
  margin-block-start: auto;
  padding-block-start: 0.65rem;
}

.music-card-meta,
.music-card-actions {
  display: flex;
  align-items: center;
}

.music-card-meta {
  flex: 1 1 auto;
  flex-wrap: wrap;
  gap: 0.3rem;
  min-inline-size: 0;
}

.music-card-actions {
  flex: 0 0 auto;
  gap: 0.25rem;
}

@media (width <= 600px) {
  .music-card {
    min-block-size: 120px;
  }

  .music-card-content {
    gap: 0.75rem;
    grid-template-columns: 88px minmax(0, 1fr);
    padding: 0.75rem;
  }

  .music-card-cover-shell {
    inline-size: 88px;
  }

  .music-card-cover-column {
    inline-size: 88px;
  }

  .music-card-body {
    position: relative;
  }

  .music-card-heading {
    align-items: center;
    min-block-size: 40px;
    padding-inline-end: 5.5rem;
  }

  .music-card-entity {
    inset-block-end: 0.375rem;
    inset-inline-start: 0.375rem;
    max-inline-size: calc(100% - 0.75rem);
  }

  .music-card-footer {
    display: block;
    padding-block-start: 0.5rem;
  }

  .music-card-actions {
    position: absolute;
    inset-block-start: 0;
    inset-inline-end: 0;
  }

  .music-card-actions :deep(.v-btn) {
    block-size: 40px;
    inline-size: 40px;
  }
}

@media (width <= 360px) {
  .music-card-meta-item--narrow-optional {
    display: none;
  }
}
</style>
