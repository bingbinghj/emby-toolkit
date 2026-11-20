<!-- Adapted from MoviePilot-Frontend v3 db17d9c (MIT). -->
<script setup>
import VDialogCloseBtn from '@/components/theme/DialogCloseBtn.vue';
import axios from "axios";
import { ref, watch, onMounted, onBeforeUnmount } from "vue";
import { useMusicI18n as useI18n } from "./i18n";
const { t } = useI18n();
const props = defineProps({ type: String, musicTypes: Array, initialKeyword: String, browseTracks:Boolean, initialAlbumId:String });
const emit = defineEmits(["update:modelValue", "select", "close"]);
const items = ref([]);
const keyword = ref(props.initialKeyword?.trim() || "");
const album=ref(null);
const loading = ref(false), error = ref('');
let searchRequestId = 0;
const inputKeyword = ref(null);
async function selectMedia(item) {
  if(props.browseTracks && item.music_type==='album') {
    const requestId=++searchRequestId; loading.value=true; error.value='';
    try {
      const {data}=await axios.post('/api/music/query/detail',{source:props.type,id:item.id,kind:'album'},{timeout:35000});
      if(requestId!==searchRequestId)return;
      album.value=item;items.value=(data.tracks || []).map(row=>({id:row.media_id,poster:row.cover_url || data.cover_url,title:row.title,music_type:'recording',label:'单曲',overview:[row.artists?.join(' / '),row.album].filter(Boolean).join(' · ')}));
    } catch(cause){if(requestId===searchRequestId)error.value=cause.response?.data?.message || cause.message;}
    finally{if(requestId===searchRequestId)loading.value=false;}
    return;
  }
  emit("update:modelValue", item.id);
  emit("select", item);
  emit("close");
}
async function searchMedias() {
  const searchKeyword = keyword.value.trim();
  if (!searchKeyword) return;
  const requestId = ++searchRequestId;
  try {
    loading.value = true; error.value = '';album.value=null;
    const { data } = await axios.post("/api/music/query/search", { query: searchKeyword, sources: [props.type], kind: props.musicTypes?.length === 1 ? props.musicTypes[0] : void 0, count: 20 }, { timeout: 19e4 });
    const result = data.items || [];
    if (requestId !== searchRequestId) return;
    error.value = Object.values(data.errors || {}).join('；');
    items.value = [];
    for (const item of result) {
      if (item.media_source !== props.type) continue;
      if (props.musicTypes?.length && item.music_type && !props.musicTypes.includes(item.music_type)) {
        continue;
      }
      const mediaId = item.media_id?.toString().trim();
      if (!mediaId) continue;
      const musicAlbum = item.music_type === "album" || item.album === item.title ? void 0 : item.album;
      const musicEntityLabels = {
        recording: t("music.entityRecording"),
        album: t("music.entityAlbum"),
        artist: t("music.entityArtist")
      };
      const musicLabels = [
        item.music_type ? musicEntityLabels[item.music_type] : void 0,
        item.album_type,
        ...item.secondary_types || []
      ].filter(Boolean);
      items.value.push({
        id: mediaId,
        poster: item.cover_url || item.poster_path || '',
        type: item.type,
        music_type: item.music_type,
        album_type: item.album_type,
        secondary_types: item.secondary_types,
        title: item.year ? `${item.title}\uFF08${item.year}\uFF09` : item.title || "",
        label: item.type === "\u97F3\u4E50" ? musicLabels.join(" \xB7 ") || item.type : item.type || "",
        overview: item.type === "\u97F3\u4E50" ? [item.artist, musicAlbum].filter(Boolean).join(" \xB7 ") : item.overview || ""
      });
    }
  } catch (e) {
    if (requestId === searchRequestId) {items.value=[]; error.value=e.response?.data?.message || e.message;}
  } finally {
    if (requestId === searchRequestId) loading.value = false;
  }
}
onMounted(() => {
  if(props.initialAlbumId) void selectMedia({id:props.initialAlbumId,music_type:'album',title:props.initialAlbumId});
  else if (keyword.value) void searchMedias();
  focusTimer = setTimeout(() => {
    inputKeyword.value?.focus();
  }, 500);
});
watch(
  () => [props.initialKeyword?.trim() || "", props.type, props.musicTypes?.join(",") || ""],
  ([nextKeyword, nextType, nextMusicTypes], [previousKeyword, previousType, previousMusicTypes]) => {
    const keywordChanged = nextKeyword !== previousKeyword && nextKeyword !== keyword.value;
    const scopeChanged = nextType !== previousType || nextMusicTypes !== previousMusicTypes;
    if (!keywordChanged && !scopeChanged) return;
    searchRequestId += 1;
    loading.value = false;
    if (keywordChanged) keyword.value = nextKeyword;
    items.value = [];
    if (keyword.value) void searchMedias();
  }
);
let focusTimer;
onBeforeUnmount(() => {
  searchRequestId++;
  clearTimeout(focusTimer);
});
</script>

<template>
  <VCard class="media-id-selector mx-auto" width="100%">
    <VAlert v-if="error" type="error" variant="tonal">{{ error }}</VAlert>
    <VCardTitle class="d-flex align-center justify-space-between">
      <span>{{ t('dialog.reorganize.mediaSearchInput') }}</span>
      <VDialogCloseBtn
        :aria-label="t('common.close')"
        inner-class="media-id-selector__close"
        @click="
          () => {
            emit('close')
          }
        "
      />
    </VCardTitle>
    <VCardText class="media-id-selector__search">
      <VTextField
        ref="inputKeyword"
        v-model="keyword"
        :mobile-layout="false"
        single-line
        :placeholder="t('dialog.reorganize.mediaSearchPlaceholder')"
        variant="outlined"
        append-inner-icon="mdi-magnify"
        flat
        hide-details
        :loading="loading"
        @click:append-inner="searchMedias"
        @keydown.enter="searchMedias"
      />
    </VCardText>
    <div v-if="album" class="px-4 pb-3 d-flex align-center ga-2"><VBtn variant="text" prepend-icon="mdi-arrow-left" :disabled="loading" @click="searchMedias">返回专辑</VBtn><span>{{ album.title }}</span></div>
    <VProgressLinear v-if="loading" indeterminate color="primary" />
    <VDivider />
    <VList v-if="items.length > 0" class="media-id-selector__results" lines="three">
      <template v-for="item in items" :key="`${item.type || 'media'}-${item.id}`">
        <VListItem @click="selectMedia(item)">
          <template #prepend>
            <VImg
              height="64"
              width="64"
              :src="item.poster"
              aspect-ratio="1"
              class="object-cover rounded ring-gray-500 me-3"
              cover
            >
              <template #placeholder>
                <div class="w-full h-full">
                  <VSkeletonLoader class="object-cover aspect-w-2 aspect-h-3" />
                </div>
              </template>
            </VImg>
          </template>
          <VListItemTitle>
            {{ item.title }}
          </VListItemTitle>
          <VListItemSubtitle class="mt-2"
            ><span class="text-primary">{{ item.label }}</span> {{ item.overview }}</VListItemSubtitle
          >
        </VListItem>
      </template>
    </VList>
  </VCard>
</template>

<style lang="scss" scoped>
.media-id-selector :deep(.v-list-item-title),.media-id-selector :deep(.v-list-item-subtitle){white-space:normal;overflow-wrap:anywhere;-webkit-line-clamp:unset;}
.media-id-selector {
  overflow: hidden !important;
}

.media-id-selector__search {
  flex: 0 0 auto;
  overflow: visible !important;
  padding-block: 0.25rem 1rem !important;
}

.media-id-selector__close {
  position: relative;
  flex: 0 0 auto;
}

.media-id-selector__results {
  flex: 1 1 auto;
  min-block-size: 0;
  overflow-anchor: none;
  overflow-y: auto;
  overscroll-behavior: contain;
  scrollbar-gutter: stable;
}
</style>
