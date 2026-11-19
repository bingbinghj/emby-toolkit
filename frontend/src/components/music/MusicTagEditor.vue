<!-- Form/selector/split preview adapted from MP ReorganizeDialog and music-transfer (MIT). -->
<script setup>
import { computed, ref, onBeforeUnmount } from 'vue';
import axios from 'axios';
import { useDisplay } from 'vuetify';
import DialogCloseBtn from '@/components/theme/DialogCloseBtn.vue';
import MusicIdSelector from './MusicIdSelector.vue';
import { musicSources } from './utils';
import { shiftLyrics } from './lyrics';
const props = defineProps({ item: Object, error: String, busy: Boolean });
const emit = defineEmits(['close','save']);
const { smAndDown } = useDisplay();
const draft = ref(props.item ? JSON.parse(JSON.stringify(props.item)) : null);
const source = ref(props.item?.media_source || 'musicbrainz'), entity = ref(props.item?.music_type === 'album' ? 'album' : 'recording'), identity = ref(props.item?.media_id || '');
const searchOpen = ref(false), lookupBusy = ref(false), error = ref(''), offset = ref(0);
const performers = ref(Object.entries(props.item?.performers || {}).map(([role,names])=>({role,names})));
let generation = 0;
const validId = computed(() => !identity.value || (source.value==='musicbrainz' ? /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(identity.value) : /^\d+$/.test(identity.value)));
const fields = [['title','标题'],['album','专辑'],['album_artist','专辑艺术家'],['isrc','ISRC']];
const numbers = [['track_number','曲序'],['total_tracks','总曲数'],['disc_number','碟号'],['total_discs','总碟数'],['release_year','发行年份'],['original_year','原始年份']];
const credits = [['artists','艺术家'],['composers','作曲'],['conductors','指挥'],['orchestras','乐团']];
const ids = [['media_id','MusicBrainz 录音 ID'],['musicbrainz_release_id','MusicBrainz 发行 ID'],['musicbrainz_release_group_id','MusicBrainz 专辑组 ID'],['musicbrainz_release_track_id','MusicBrainz 发行曲目 ID']];
async function applySelection(item) {
  if (!draft.value || lookupBusy.value) return;
  identity.value = item.id || identity.value; entity.value = item.music_type || entity.value;
  searchOpen.value = false; lookupBusy.value = true; error.value = '';
  const version = ++generation;
  try {
    const {data} = await axios.post('/api/music/query/edit_metadata', {path:draft.value.path,selection:{media_source:source.value,media_id:identity.value,music_type:entity.value}}, {timeout:190000});
    if (version !== generation) return;
    for (const key of [...fields,...numbers,...credits,...ids].map(([key])=>key).concat(['media_source','music_type','cover_url'])) {
      if (data[key] != null && data[key] !== '' && (!Array.isArray(data[key]) || data[key].length)) draft.value[key] = data[key];
    }
    if (Object.keys(data.performers || {}).length) performers.value = Object.entries(data.performers).map(([role,names])=>({role,names}));
  } catch(cause) { if (version===generation) error.value = cause.response?.data?.message || cause.message; }
  finally { if (version===generation) lookupBusy.value=false; }
}
function shift() { try { draft.value.lyrics=shiftLyrics(draft.value.lyrics || '', offset.value); offset.value=0; error.value=''; } catch(cause) { error.value=cause.message; } }
function save() {
  if (!draft.value || props.busy || lookupBusy.value) return;
  const values={};
  for(const item of performers.value) if(item.role.trim() && item.names?.length) values[item.role.trim()]=[...(values[item.role.trim()] || []),...item.names];
  emit('save',{...draft.value,performers:values});
}
onBeforeUnmount(()=>generation++);
</script>

<template>
  <VDialog :model-value="!searchOpen" @update:model-value="!$event && emit('close')" :max-width="'72rem'" :fullscreen="smAndDown" scrollable>
    <VCard class="reorganize-dialog-card"><VCardItem class="py-3 pe-16"><template #prepend><VIcon icon="mdi-music-box-outline" /></template><VCardTitle>编辑音乐标签</VCardTitle><VCardSubtitle class="editor-path">{{ item?.path }}</VCardSubtitle></VCardItem><DialogCloseBtn @click="emit('close')" /><VDivider />
      <VCardText><VAlert v-if="props.error || error" type="error" variant="tonal" class="mb-4">{{ props.error || error }}</VAlert><VProgressLinear v-if="(!draft && !props.error) || lookupBusy" indeterminate color="primary" class="mb-4" />
        <VForm v-if="draft" :disabled="lookupBusy || busy" @submit.prevent="save">
          <VRow><VCol cols="12" md="6"><VSelect v-model="source" :items="musicSources" label="资料来源" prepend-inner-icon="mdi-database-search" @update:model-value="identity=''" /></VCol><VCol cols="12" md="6"><VSelect v-model="entity" :items="[{title:'单曲',value:'recording'},{title:'专辑',value:'album'}]" label="音乐类型" prepend-inner-icon="mdi-music-box-multiple" /></VCol>
          <VCol cols="12"><VTextField v-model="identity" :label="`${musicSources.find(item=>item.value===source)?.title || source} ID`" prepend-inner-icon="mdi-identifier" hint="搜索资料并填入下方标签，核对后预览保存。" persistent-hint :error-messages="validId ? [] : ['ID 格式不正确']" @keyup.enter.prevent="validId && identity && applySelection({})"><template #append-inner><VBtn icon="mdi-magnify" variant="text" aria-label="搜索音乐资料" :disabled="lookupBusy" @click="searchOpen=true" /></template><template #append><VBtn variant="tonal" :loading="lookupBusy" :disabled="!identity || !validId" @click="applySelection({})">读取资料</VBtn></template></VTextField></VCol></VRow>
          <VDivider class="my-6" />
          <VRow><VCol v-for="[key,label] in fields" :key="key" cols="12" md="6"><VTextField v-model="draft[key]" :label="label" /></VCol>
          <VCol v-for="[key,label] in credits" :key="key" cols="12" md="6"><VCombobox v-model="draft[key]" :label="label" multiple chips closable-chips /></VCol>
          <VCol v-for="[key,label] in numbers" :key="key" cols="6" md="4"><VTextField v-model.number="draft[key]" :label="label" type="number" min="0" /></VCol></VRow>
          <div class="d-flex align-center ga-3 my-4"><h3 class="text-subtitle-1">演奏者</h3><VBtn variant="text" prepend-icon="mdi-plus" @click="performers.push({role:'',names:[]})">添加乐器 / 声部</VBtn></div>
          <VRow v-for="(performer,index) in performers" :key="index"><VCol cols="12" md="4"><VTextField v-model="performer.role" label="乐器 / 声部" /></VCol><VCol cols="10" md="7"><VCombobox v-model="performer.names" label="演奏者姓名" multiple chips closable-chips /></VCol><VCol cols="2" md="1"><VBtn icon="mdi-close" variant="text" aria-label="移除演奏者" @click="performers.splice(index,1)" /></VCol></VRow>
          <VExpansionPanels class="my-5" variant="accordion"><VExpansionPanel title="MusicBrainz 身份标签"><VExpansionPanelText><VRow><VCol v-for="[key,label] in ids.filter(([key])=>key!=='media_id' || draft.media_source==='musicbrainz' && draft.music_type==='recording')" :key="key" cols="12" md="6"><VTextField v-model="draft[key]" :label="label" /></VCol></VRow></VExpansionPanelText></VExpansionPanel></VExpansionPanels>
          <VTextField v-model="draft.cover_url" label="替换封面地址" prepend-inner-icon="mdi-image-outline" hint="留空保留当前封面；填写后将在保存时替换内嵌封面。" persistent-hint />
          <h3 class="text-subtitle-1 mt-6 mb-3">歌词</h3><div v-if="draft.lyrics_source" class="text-caption text-medium-emphasis mb-3">{{ draft.lyrics_source }}</div>
          <VTextarea v-model="draft.lyrics" label="旁挂歌词（LRC / 纯文本）" rows="12" spellcheck="false" hint="按 MP 的旁挂方式保存为同名歌词文件。修改只在预览确认后写入。" persistent-hint class="lyrics-editor" />
          <div class="lyrics-offset mt-4"><VTextField v-model.number="offset" label="整体偏移（秒）" type="number" step="0.1" hint="正数延后，负数提前；适用于整首偏差相同的歌词。" persistent-hint /><VBtn variant="tonal" @click="shift">应用到歌词</VBtn></div>
        </VForm>
      </VCardText><VDivider /><VCardActions class="app-dialog-actions"><VSpacer /><VBtn color="default" variant="text" @click="emit('close')">取消</VBtn><VBtn color="primary" variant="flat" :disabled="!draft || busy || lookupBusy" @click="save">预览修改</VBtn></VCardActions>
    </VCard>
  </VDialog>
  <VDialog v-model="searchOpen" max-width="700" :fullscreen="smAndDown" scrollable><MusicIdSelector :type="source" :music-types="[entity]" :initial-keyword="entity==='album' ? draft?.album : draft?.title" @select="applySelection" @close="searchOpen=false" /></VDialog>
</template>

<style scoped>
.editor-path{white-space:normal;overflow-wrap:anywhere;}.lyrics-offset{display:flex;align-items:flex-start;gap:1rem;flex-wrap:wrap;}.lyrics-offset>.v-input{flex:1 1 240px;}.lyrics-editor :deep(textarea){font-family:monospace;line-height:1.7;}.reorganize-dialog-card :deep(.v-card-text){min-inline-size:0;}
</style>
