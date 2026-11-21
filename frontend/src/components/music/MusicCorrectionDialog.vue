<!-- Adapted from MoviePilot-Frontend v3 db17d9c (MIT). -->
<script setup>
import MediaIdSelector from './MusicIdSelector.vue';
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useMusicI18n as useI18n } from "./i18n";
import axios from "axios";
const getApiBusinessErrorMessage = (cause) => cause.response?.data?.message || cause.message;
import { musicSources, musicReleaseOptions } from "./utils";
const isMusicMediaSource = (source2) => musicSources.some((item) => item.value === source2);
const props = defineProps({ group: Object, busy: Boolean });
const emit = defineEmits(["close", "apply"]);
const { t } = useI18n();
const prefix = "dialog.reorganize.musicPreview.";
const original = props.group.items.find((item) => item.music?.file_role === "audio")?.music;
const source = ref(
  original?.media_source && isMusicMediaSource(original.media_source) ? original.media_source : "musicbrainz"
);
const albumId = ref(
  original?.musicbrainz_release_group_id ?? (original?.music_type === "album" ? original.media_id : "") ?? ""
);
const album = ref();
const releaseId = ref("");
const releaseDraft = ref("");
const loading = ref(false);
const error = ref("");
let requestId = 0;
const sources = ref(musicSources);
const sourceItems = computed(() => sources.value.filter((item) => isMusicMediaSource(item.value)));
const keyword = computed(
  () => [original?.album_artist || original?.artists?.[0], original?.album || original?.title].filter(Boolean).join(" ")
);
const candidates = computed(
  () => props.group.items.flatMap((item) => item.music?.candidates ?? []).filter((item, index, all) => all.findIndex((other) => JSON.stringify(other) === JSON.stringify(item)) === index).slice(0, 5)
);
const editions = computed(() => musicReleaseOptions(album.value || {}));
const ready = computed(
  () => Boolean(
    album.value?.tracks?.length && album.value.media_id === albumId.value && album.value.media_source === source.value && (source.value !== "musicbrainz" || releaseId.value && releaseDraft.value.trim().toLowerCase() === releaseId.value)
  )
);
function resetAlbum() {
  requestId += 1;
  album.value = void 0;
  albumId.value = "";
  releaseId.value = "";
  releaseDraft.value = "";
  error.value = "";
  loading.value = false;
}
watch(source, resetAlbum, { flush: "sync" });
async function loadAlbum(selectedRelease) {
  const current = ++requestId;
  loading.value = true;
  error.value = "";
  const knownReleases = album.value?.releases;
  album.value = void 0;
  try {
    const { data: result } = await axios.post("/api/music/query/detail", { source: source.value, id: albumId.value, kind: "album", release: selectedRelease }, { timeout: 19e4 });
    if (current !== requestId) return;
    if (result.media_source !== source.value || result.media_id !== albumId.value || !result.tracks?.length || selectedRelease && result.musicbrainz_release_id !== selectedRelease.toLowerCase())
      throw new Error(t(prefix + "editionUnavailable"));
    album.value = { ...result, releases: result.releases?.length ? result.releases : knownReleases };
    releaseId.value = result.musicbrainz_release_id ?? "";
    releaseDraft.value = releaseId.value;
  } catch (cause) {
    if (current === requestId) error.value = getApiBusinessErrorMessage(cause) || t(prefix + "editionUnavailable");
  } finally {
    if (current === requestId) loading.value = false;
  }
}
function selectAlbum(item) {
  albumId.value = item.id;
  void loadAlbum();
}
function selectCandidate(candidate) {
  const id = candidate.album_id || (candidate.music_type === "album" ? candidate.media_id : "");
  if (!id) return;
  source.value = candidate.media_source ?? "musicbrainz";
  albumId.value = id;
  void loadAlbum(source.value === "musicbrainz" ? candidate.release_id ?? void 0 : void 0);
}
function readRelease() {
  const id = releaseDraft.value.trim().toLowerCase();
  if (!/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(id)) {
    error.value = t(prefix + "releaseIdInvalid");
    return;
  }
  void loadAlbum(id);
}
function apply() {
  if (!ready.value || loading.value || props.busy) return;
  emit("apply", {
    media_source: source.value,
    media_id: albumId.value,
    ...source.value === "musicbrainz" ? { musicbrainz_release_id: releaseId.value } : {}
  });
}
onMounted(() => {
  if (albumId.value) void loadAlbum(original?.musicbrainz_release_id ?? void 0);
});
onUnmounted(() => {
  requestId += 1;
});
</script>

<template>
  <VDialog
    :model-value="true"
    max-width="760"
    scrollable
    :persistent="busy"
    @update:model-value="!busy && emit('close')"
  >
    <VCard :title="t(prefix + 'correctTitle')">
      <VCardText class="music-correction-body">
        <p class="text-body-2 mb-4">{{ t(prefix + 'scopeHint', { count: group.items.length }) }}</p>
        <VSelect
          v-model="source"
          :items="sourceItems"
          :label="t(prefix + 'source')"
          :disabled="busy || loading"
          hide-details
          class="mb-4"
        />
        <div v-if="!albumId">
          <div v-if="candidates.length" class="mb-4">
            <p class="text-caption mb-2">{{ t(prefix + 'candidates') }}</p>
            <VBtn
              v-for="(candidate, index) in candidates"
              :key="index"
              class="ma-1 music-candidate"
              variant="tonal"
              size="small"
              :disabled="!candidate.album_id && !(candidate.music_type === 'album' && candidate.media_id)"
              @click="selectCandidate(candidate)"
            >
              {{ [candidate.title, candidate.artist, candidate.year].filter(Boolean).join(' · ') }}
            </VBtn>
          </div>
          <MediaIdSelector :type="source" :music-types="['album']" :initial-keyword="keyword" @select="selectAlbum" />
        </div>
        <template v-else>
          <VBtn variant="text" prepend-icon="mdi-magnify" :disabled="busy" @click="resetAlbum">{{
            t(prefix + 'searchAgain')
          }}</VBtn>
          <VProgressLinear v-if="loading" indeterminate class="my-3" :aria-label="t('common.loading')" />
          <VAlert v-if="error" type="error" variant="tonal" class="my-3">{{ error }}</VAlert>
          <template v-if="album">
            <h3 class="text-h6 mt-3">{{ album.title }}</h3>
            <p class="text-body-2 mb-4">{{ album.artist || album.artists?.join(' / ') }} · {{ album.release_date }}</p>
            <VSelect
              v-if="source === 'musicbrainz'"
              :model-value="releaseId"
              :items="editions"
              :label="t(prefix + 'edition')"
              :disabled="busy || loading"
              @update:model-value="loadAlbum($event)"
            />
            <p class="text-caption mb-2">{{ t(prefix + 'tracksHint', { count: album.tracks?.length ?? 0 }) }}</p>
            <ol class="music-correction-tracks">
              <li v-for="(track, index) in album.tracks" :key="`${track.media_id}-${index}`">
                <span>{{ track.disc_number || 1 }}.{{ track.track_number || index + 1 }}</span>
                <span>{{ track.title }}</span
                ><span class="text-medium-emphasis">{{ track.artist || track.artists?.join(' / ') }}</span>
              </li>
            </ol>
          </template>
          <details v-if="source === 'musicbrainz' && !editions.length" class="mt-4">
            <summary>{{ t(prefix + 'otherEdition') }}</summary>
            <p class="text-caption my-2">{{ t(prefix + 'releaseIdHint') }}</p>
            <VTextField v-model="releaseDraft" :label="t(prefix + 'releaseId')" :disabled="busy || loading" />
            <VBtn variant="tonal" :disabled="busy || loading" @click="readRelease">{{
              t(prefix + 'readEdition')
            }}</VBtn>
          </details>
        </template>
      </VCardText>
      <VCardActions class="flex-wrap">
        <VBtn data-native-dialog-close :disabled="busy" @click="emit('close')">{{ t('common.cancel') }}</VBtn>
        <VSpacer />
        <VBtn color="primary" variant="flat" :disabled="!ready || loading || busy" :loading="busy" @click="apply">{{
          t(prefix + 'applyPreview')
        }}</VBtn>
      </VCardActions>
    </VCard>
  </VDialog>
</template>

<style scoped>
.music-correction-body {
  overflow-wrap: anywhere;
}
.music-correction-tracks {
  padding: 0;
  list-style: none;
  max-height: 280px;
  overflow-y: auto;
}
.music-correction-tracks li {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  padding: 6px 0;
  border-bottom: 1px solid rgba(var(--v-border-color), var(--v-border-opacity));
}
.music-candidate {
  max-width: 100%;
  height: auto;
  min-height: 30px;
  white-space: normal;
}
</style>
