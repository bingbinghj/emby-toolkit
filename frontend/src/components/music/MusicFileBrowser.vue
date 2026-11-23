<!-- File list/navigation adapted from MoviePilot-Frontend v3 FileList.vue (MIT). -->
<script setup>
import { computed, ref, watch } from 'vue';
import { useDisplay } from 'vuetify';
const props = defineProps({ items: Array, directory: String, loading: Boolean, busy: Boolean, searching: Boolean });
const selected = defineModel('selected', { default: () => [] });
const emit = defineEmits(['navigate', 'edit', 'rescrape', 'refresh', 'preview']);
const { smAndDown } = useDisplay();
const selectMode = ref(false), sort = ref('name'), descending = ref(false);
const name = item => item.name || item.path.split('/').pop();
const selectable = item => item.type === 'dir' || item.is_audio !== false;
const rows = computed(() => [...(props.items || [])].sort((a,b) => {
  const directoryOrder = Number(b.type === 'dir') - Number(a.type === 'dir');
  const order = sort.value === 'name' ? name(a).localeCompare(name(b), 'zh-CN', { numeric: true }) : (a[sort.value] || 0) - (b[sort.value] || 0);
  return directoryOrder || (descending.value ? -order : order);
}));
const choices = computed(() => rows.value.filter(selectable));
const allSelected = computed(() => choices.value.length > 0 && choices.value.every(item => selected.value.includes(item.path)));
function toggle(item) { if (!selectable(item)) return; selected.value = selected.value.includes(item.path) ? selected.value.filter(path => path !== item.path) : [...selected.value, item.path]; }
function activate(item) { if (selectMode.value) toggle(item); else if (item.type === 'dir') emit('navigate', item.path); else if (item.is_audio !== false || item.audio_path) emit('edit', item.audio_path ? {path:item.audio_path} : item); }
function changeSort(key) { if (sort.value === key) descending.value = !descending.value; else { sort.value = key; descending.value = false; } }
function bytes(value) { if (value == null) return ''; return value >= 1024**3 ? `${(value/1024**3).toFixed(2)} GB` : value >= 1024**2 ? `${(value/1024**2).toFixed(1)} MB` : `${Math.round(value/1024)} KB`; }
function date(item) { const value = item.modify_time || Number(item.signature?.mtime || 0) / 1e9; return value ? new Date(value*1000).toLocaleString() : ''; }
watch(() => props.directory, () => { selectMode.value = false; selected.value = []; });
</script>

<template>
  <VCard class="music-file-browser" data-testid="music-library">
    <div class="file-list__toolbar">
      <span class="text-subtitle-1">文件</span>
      <VSpacer /><VBtn variant="text" :prepend-icon="selectMode ? 'mdi-close' : 'mdi-checkbox-marked-outline'" @click="selectMode = !selectMode; selected = []">{{ selectMode ? '取消选择' : '选择' }}</VBtn>
    </div>
    <div v-if="selectMode" class="file-list__selection">
      <VCheckbox :model-value="allSelected" :indeterminate="selected.length > 0 && !allSelected" :label="`全选本页 ${selected.length}/${choices.length} 项`" hide-details @update:model-value="selected = $event ? choices.map(item=>item.path) : []" />
      <VBtn color="primary" variant="tonal" :disabled="!selected.length || busy" @click="emit('rescrape', selected)">批量重新刮削</VBtn>
      <VBtn variant="text" :disabled="!selected.length" @click="selected = []">取消全选</VBtn>
    </div>
    <div class="file-list__columns"><button v-for="column in [{key:'name',label:'名称'},{key:'size',label:'大小'},{key:'modify_time',label:'修改时间'}]" :key="column.key" :class="{ 'text-primary':sort===column.key }" @click="changeSort(column.key)">{{ column.label }}<VIcon v-if="sort===column.key" :icon="descending ? 'mdi-arrow-down' : 'mdi-arrow-up'" size="14" /></button><span /></div>
    <VProgressLinear v-if="loading" indeterminate color="primary" />
    <VVirtualScroll v-else-if="rows.length" :items="rows" :item-height="smAndDown ? 70 : 58" class="file-list-container" role="list" aria-label="音乐文件管理"><template #default="{ item }">
      <VListItem :key="item.path" class="file-row" :class="{'file-row--active':selected.includes(item.path)}" :aria-label="name(item)" tabindex="0" @click="activate(item)" @keydown.enter.prevent="activate(item)">
        <div class="file-row__name">
          <VCheckbox v-if="selectMode" :model-value="selected.includes(item.path)" :disabled="!selectable(item)" :aria-label="`选择 ${name(item)}`" hide-details density="compact" @click.stop @keydown.stop @update:model-value="toggle(item)" />
          <VIcon v-else :icon="item.type==='dir' ? 'mdi-folder' : item.is_audio!==false ? 'mdi-file-music-outline' : 'mdi-file-outline'" :color="item.type==='dir' ? 'primary' : undefined" size="24" />
          <div class="file-row__label"><span class="file-row__filename" :title="name(item)">{{ name(item) }}</span><small v-if="searching" class="text-medium-emphasis file-row__path">{{ item.path }}</small><small v-if="smAndDown" class="text-medium-emphasis">{{ [bytes(item.size),date(item)].filter(Boolean).join(' · ') }}</small></div>
        </div>
        <span class="file-row__size num">{{ bytes(item.size) }}</span><span class="file-row__time num">{{ date(item) }}</span>
        <template #append><VBtn v-if="!selectMode && item.type!=='dir' && item.is_audio!==false" icon="mdi-play-circle-outline" variant="text" :aria-label="`预览 ${name(item)}`" @click.stop="emit('preview',item)" @keydown.stop /><VMenu v-if="!selectMode && selectable(item)"><template #activator="{ props: menuProps }"><VBtn v-bind="menuProps" icon="mdi-dots-horizontal" variant="text" :aria-label="`操作 ${name(item)}`" @click.stop @keydown.stop /></template><VList><VListItem v-if="item.type!=='dir'" title="歌曲预览" prepend-icon="mdi-play-circle-outline" @click="emit('preview',item)" /><VListItem v-if="item.type!=='dir'" title="编辑标签" prepend-icon="mdi-pencil-outline" @click="emit('edit',item)" /><VListItem title="重新刮削" prepend-icon="mdi-music-box-outline" :disabled="busy" @click="emit('rescrape',[item.path])" /></VList></VMenu></template>
      </VListItem>
    </template></VVirtualScroll>
    <div v-else class="text-center pa-8 text-medium-emphasis">{{ searching ? '没有匹配的音乐，请先扫描音乐库更新搜索索引' : '此目录为空' }}</div>
  </VCard>
</template>

<style scoped>
.music-file-browser{min-inline-size:0;container-type:inline-size;}
.file-list__toolbar,.file-list__selection{display:flex;align-items:center;gap:.5rem;padding:.5rem .75rem;flex-wrap:wrap;}
.file-list__selection>.v-input{flex:0 1 auto;min-inline-size:12rem;}
.file-list__columns{display:grid;grid-template-columns:minmax(0,1fr) 6rem 10rem 5.5rem;gap:1rem;padding:1rem;font-size:.875rem;border-block:1px solid var(--mp-list-row-divider);}
.file-list__columns>button{text-align:start;}
.file-list-container{padding:0;max-block-size:52vh;overflow:auto;}
.file-row{min-block-size:3.625rem;padding-inline:1rem .5rem;border-block-end:1px solid var(--mp-list-row-divider);border-radius:0;cursor:pointer;}
.file-row :deep(.v-list-item__content){display:grid;grid-template-columns:minmax(0,1fr) 6rem 10rem;align-items:center;gap:1rem;}
.file-row :deep(.v-list-item__append){inline-size:5.5rem;margin-inline-start:1rem;justify-content:flex-end;}
.file-row--active{background:var(--mp-primary-soft);border-radius:12px;}
.file-row__name{display:flex;align-items:center;gap:.75rem;min-inline-size:0;}
.file-row__label{display:flex;flex-direction:column;min-inline-size:0;}
.file-row__filename{font-size:.875rem;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;}
.file-row__path{overflow-wrap:anywhere;}
.file-row__size,.file-row__time{font-size:.875rem;color:var(--mp-muted-fg);}
.file-row__name :deep(.v-input){flex:0 0 auto;}
@container(max-width:600px){.file-list__columns{grid-template-columns:minmax(0,1fr) 5rem;}.file-list__columns>button:not(:first-child),.file-row__size,.file-row__time{display:none;}.file-row :deep(.v-list-item__content){grid-template-columns:minmax(0,1fr);}.file-row__filename{white-space:normal;overflow-wrap:anywhere;}.file-row{padding:.5rem .375rem .5rem .75rem;}.file-row :deep(.v-list-item__append){inline-size:5rem;margin-inline-start:.25rem;}.file-row :deep(.v-list-item__append .v-btn){width:36px;height:36px;}.file-row__name{gap:.5rem;}}
</style>
