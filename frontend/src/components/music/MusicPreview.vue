<!-- Adapted from MoviePilot-Frontend v3 db17d9c (MIT). -->
<script setup>
import {canCorrectMusicGroup, previewSourceKey} from './previewState';
import { computed, ref, watch } from "vue";
import { useMusicI18n as useI18n } from "./i18n";
const props = defineProps({ groups: Array, disabled: Boolean, scopeComplete: Boolean, corrected: Boolean });
const emit = defineEmits(["correct"]);
const { t, te } = useI18n();
const prefix = "dialog.reorganize.musicPreview.";
const page = ref(1);
const pages = computed(() => Math.ceil(props.groups.length / 10));
const visible = computed(() => props.groups.slice((page.value - 1) * 10, page.value * 10));
watch(
  () => props.groups,
  () => {
    page.value = Math.min(page.value, pages.value || 1);
  }
);
function title(group) {
  const item = group.items.find((row) => row.music?.file_role === "audio") ?? group.items[0];
  return item.music?.album || item.music?.title || item.collection_group || item.title || t(prefix + "unknownAlbum");
}
function status(info) {
  return t(prefix + "states." + state(info));
}
function state(info) {
  return info?.status === "matched" && !info.online_confirmed ? "metadata" : info?.status ?? "metadata";
}
function color(info) {
  if (info?.status === "matched" && info.online_confirmed) return "success";
  if (["local_tags", "local_cue", "manual"].includes(info?.status ?? "")) return "info";
  return "warning";
}
const tagFields=[['comment','备注'],['description','简介 / 描述'],['title','曲名'],['artists','艺人'],['album','专辑'],['album_artist','专辑艺人'],['track_number','曲序'],['total_tracks','总曲数'],['disc_number','碟号'],['total_discs','总碟数'],['release_year','发行年份'],['original_year','原始年份'],['isrc','ISRC'],['composers','作曲'],['conductors','指挥'],['orchestras','乐团'],['performers','演奏者'],['media_id','MusicBrainz 录音 ID'],['musicbrainz_release_id','MusicBrainz 发行 ID'],['musicbrainz_release_group_id','MusicBrainz 专辑组 ID'],['musicbrainz_release_track_id','MusicBrainz 发行曲目 ID']];
const tagKeys={artists:'artist',album_artist:'albumartist',track_number:'tracknumber',total_tracks:'tracknumber',disc_number:'discnumber',total_discs:'discnumber',release_year:'date',original_year:'originaldate',composers:'composer',conductors:'conductor',orchestras:'orchestra',performers:'performer',media_id:'musicbrainz_trackid',musicbrainz_release_id:'musicbrainz_albumid',musicbrainz_release_group_id:'musicbrainz_releasegroupid',musicbrainz_release_track_id:'musicbrainz_releasetrackid'};
const after=(item,field)=>item.tag_updates && !Object.hasOwn(item.tag_updates,tagKeys[field] || field) ? item.before?.[field] : item.metadata?.[field];
const fieldText=value=>Array.isArray(value) ? value.join(' / ') : value && typeof value==='object' ? Object.entries(value).map(([role,names])=>`${role}: ${names.join(' / ')}`).join('；') : value ?? '—';
const afterText=(item,field)=>{const value=after(item,field);return value==null || fieldText(value)==='' ? '已清空' : fieldText(value);};
function fields(item, changed=true) {
  return tagFields.filter(([field])=>{
    const before=fieldText(item.before?.[field]), value=fieldText(after(item,field));
    return [before,value].some(text=>text!=='' && text!=='—') && (before!==value)===changed;
  });
}
function evidence(item) {
  return ["title", "artists", "album", "album_artist", "year", "track_number", "disc_number"].flatMap((field) => {
    const source = item.music?.field_sources?.[field];
    return source && te(prefix + "origins." + source) ? [t(prefix + "fields." + field) + ": " + t(prefix + "origins." + source)] : [];
  }).join(" \xB7 ");
}
</script>

<template>
  <div class="music-preview">
    <VAlert type="info" variant="tonal" density="compact" class="mb-3">{{ t(prefix + 'explanation') }}</VAlert>
    <VAlert v-if="corrected" type="info" variant="tonal" density="compact" class="mb-3">{{
      t(prefix + 'frozenScope')
    }}</VAlert>
    <VAlert
      v-if="corrected && groups.some(group => group.items.some(item => item.success === false))"
      type="warning"
      variant="tonal"
      class="mb-3"
      >{{ t(prefix + 'pending') }}</VAlert
    >
    <section v-for="group in visible" :key="group.id" class="music-preview-group" :aria-label="title(group)">
      <header class="music-preview-group__header">
        <div class="music-preview-group__heading">
          <h3 class="text-subtitle-1">{{ title(group) }}</h3>
          <div class="text-caption text-medium-emphasis">
            {{
              group.items.find(item => item.music?.album_artist)?.music?.album_artist ||
              group.items[0].music?.artists?.join(' / ')
            }}
          </div>
          <div class="text-caption">
            {{ group.items[0].source_storage || group.items[0].source_item?.storage }} ·
            {{ group.items[0].music?.group_directory }}
          </div>
          <div class="d-flex flex-wrap ga-1 mt-2">
            <VChip
              v-for="state in [...new Set(group.items.map(item => state(item.music)))]"
              :key="state"
              size="small"
              variant="tonal"
            >
              {{ t(prefix + 'states.' + state) }}
            </VChip>
          </div>
        </div>
        <VBtn
          size="small"
          variant="tonal"
          color="primary"
          :disabled="disabled || !scopeComplete || !canCorrectMusicGroup(group)"
          :aria-label="t(prefix + 'correctAlbum', { album: title(group) })"
          @click="emit('correct', group)"
        >
          {{ t(prefix + 'correct') }}
        </VBtn>
      </header>
      <p v-if="!scopeComplete || !canCorrectMusicGroup(group)" class="text-caption text-medium-emphasis px-3 pb-2">
        {{ t(prefix + 'cannotCorrect') }}
      </p>
      <details :open="groups.length === 1">
        <summary class="pa-3">{{ t(prefix + 'files', { count: group.items.length }) }}</summary>
        <VCard v-for="item in group.items" :key="previewSourceKey(item)" tag="article" variant="outlined" class="music-preview-file" :aria-label="`预览 ${item.music?.title || item.title || item.path}`">
          <div class="d-flex flex-wrap align-center ga-2">
            <VIcon icon="mdi-file-music-outline" color="primary" />
            <VChip size="small" :color="item.success===false ? 'error' : color(item.music)" variant="tonal">{{ item.success===false ? '校验失败' : status(item.music) }}</VChip>
            <strong>{{ item.music?.title || item.title }}</strong>
            <span v-if="item.music?.disc_number || item.music?.track_number" class="text-caption">
              {{
                t(prefix + 'position', { disc: item.music?.disc_number || '-', track: item.music?.track_number || '-' })
              }}
            </span>
            <span v-if="item.music?.read_status" class="text-caption">{{
              t(prefix + 'read.' + item.music.read_status)
            }}</span>
          </div>
          <p v-if="item.music?.artists?.length" class="text-caption mt-1">{{ item.music.artists.join(' / ') }}</p>
          <p v-if="evidence(item)" class="text-caption text-medium-emphasis mt-1">{{ evidence(item) }}</p>
          <p class="music-preview-path mt-3"><VIcon icon="mdi-file-outline" size="18" /><span>{{ item.source || item.path }}</span></p>
          <div class="d-flex flex-wrap ga-2 my-3">
            <VChip size="small" color="primary" variant="tonal">{{ fields(item).length ? `${fields(item).length} 项标签变更` : '标签不变' }}</VChip>
            <VChip v-if="item.cover_upload || item.cover_edit" size="small" prepend-icon="mdi-image-outline" variant="tonal">替换封面</VChip>
            <VChip v-if="item.lyrics_edit" size="small" prepend-icon="mdi-text-box-outline" variant="tonal">更新歌词</VChip>
          </div>
          <section v-for="[field,label] in fields(item)" :key="field" class="music-field-change" :aria-label="label">
            <h4>{{ label }}</h4>
            <div class="music-field-value"><span class="music-field-caption">当前值</span><p>{{ fieldText(item.before?.[field]) || '未填写' }}</p></div>
            <div class="music-field-value music-field-value--after"><span class="music-field-caption"><VIcon icon="mdi-arrow-right" size="16" /> 修改后</span><p>{{ afterText(item,field) }}</p></div>
          </section>
          <details v-if="fields(item,false).length" class="music-unchanged mt-3">
            <summary>未修改的标签（{{ fields(item,false).length }} 项）</summary>
            <dl><template v-for="[field,label] in fields(item,false)" :key="field"><dt>{{ label }}</dt><dd>{{ fieldText(after(item,field)) }}</dd></template></dl>
          </details>
          <section v-if="item.cover_upload || item.cover_edit" class="music-asset-preview mt-3" aria-label="待保存封面">
            <h4 class="mb-3"><VIcon icon="mdi-image-outline" size="20" /> 待保存封面</h4>
            <VImg :src="item.cover_upload || item.cover_edit" width="160" height="160" contain rounded><template #error><div class="pa-3">封面预览暂不可用</div></template></VImg>
            <p v-if="item.cover_edit" class="text-caption mt-2">{{ item.cover_edit }}</p>
          </section>
          <details v-if="item.lyrics_edit" open class="music-asset-preview mt-3"><summary><VIcon icon="mdi-text-box-outline" size="20" /> 待保存歌词</summary><pre class="lyrics-preview">{{ item.lyrics_edit.text }}</pre></details>
          <p v-if="item.success === false || item.message" class="text-caption text-error mt-2">{{ item.message }}</p>
          <p v-if="item.recovery_action" class="text-caption">{{ item.recovery_action }}</p>
        </VCard>
      </details>
    </section>
    <nav v-if="pages > 1" class="d-flex align-center justify-center ga-3 mt-3" :aria-label="t(prefix + 'albums')">
      <VBtn
        icon="mdi-chevron-left"
        variant="text"
        :disabled="page === 1"
        :aria-label="t(prefix + 'previous')"
        @click="page--"
      />
      <span>{{ page }} / {{ pages }}</span>
      <VBtn
        icon="mdi-chevron-right"
        variant="text"
        :disabled="page === pages"
        :aria-label="t(prefix + 'next')"
        @click="page++"
      />
    </nav>
  </div>
</template>

<style scoped>
.music-preview-file :is(p,dd){white-space:pre-wrap;overflow-wrap:anywhere;}
.lyrics-preview{white-space:pre-wrap;overflow-wrap:anywhere;max-height:20rem;overflow:auto;padding-block-start:1rem;font:inherit;line-height:1.8;}
.music-preview-group {
  margin-bottom: 12px;
  border: 1px solid var(--mp-border);
  border-radius: 10px;
  overflow-wrap: anywhere;
}
.music-preview-group__header {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  padding: 12px;
  flex-wrap: wrap;
}
.music-preview-group__heading {
  flex: 1;
  min-width: 180px;
}
.music-preview-file {
  margin: 0 12px 12px;
  padding: 16px;
  color: rgb(var(--v-theme-on-surface));
}
.music-preview-path{display:flex;align-items:flex-start;gap:8px;font-size:.8125rem;}.music-preview-path span{min-inline-size:0;}
.music-field-change {
  display: grid;
  grid-template-columns: minmax(6rem,.55fr) minmax(0,1fr) minmax(0,1fr);
  gap: 12px;
  padding-block: 12px;
  border-block-start: 1px solid var(--mp-border);
  font-size: .875rem;
}
.music-field-change h4{font-size:inherit;font-weight:600;padding-block:8px;}
.music-field-value{min-inline-size:0;padding:10px 12px;border-radius:8px;background:var(--mp-muted);}
.music-field-value--after{border-inline-start:3px solid rgb(var(--v-theme-primary));background:rgba(var(--v-theme-primary),.07);}
.music-field-caption{display:flex;align-items:center;gap:4px;font-size:.75rem;font-weight:600;margin-block-end:8px;}
.music-field-value p{line-height:1.7;}
.music-unchanged summary,.music-asset-preview summary{cursor:pointer;font-weight:500;}
.music-unchanged dl{display:grid;grid-template-columns:minmax(6rem,.5fr) minmax(0,1fr);gap:10px;padding-block:12px;font-size:.875rem;}
.music-asset-preview{border:1px solid var(--mp-border);border-radius:10px;padding:16px;}
@media (max-width: 600px) {
  .music-field-change{grid-template-columns:minmax(0,1fr);gap:8px;}
  .music-preview-file{margin-inline:8px;padding:12px;}
}
</style>
