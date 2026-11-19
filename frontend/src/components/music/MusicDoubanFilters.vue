<!-- Music-only filters from MoviePilot-Frontend v3 DoubanView (MIT); see ../../../third-party/MoviePilot-LICENSE. -->
<script setup>
import {ref,watch} from 'vue';
const emit=defineEmits(['change']);
const category=ref('流行'), region=ref(''), sort=ref('U'), cover=ref('all');
const categories=['流行','摇滚','民谣','电子','爵士','古典','原声','独立音乐','纯音乐','R&B','hip-hop'];
const regions=['华语','欧美','日本','韩国','内地','香港','台湾','美国','英国','粤语'];
const sorts={U:'综合排序',S:'评分排序',R:'发行时间',O:'标记人数'};
watch([category,region,sort,cover],()=>emit('change',{tags:[category.value,region.value].filter(Boolean).join(','),douban_sort:sort.value,with_cover:cover.value==='with_cover'}));
</script>
<template>
  <div class="px-3 music-douban-filters">
    <div class="music-filter-row"><VLabel>风格</VLabel><VChipGroup v-model="category" mandatory><VChip v-for="value in categories" :key="value" :value="value" filter tile>{{ value==='hip-hop' ? 'Hip-Hop' : value }}</VChip></VChipGroup></div>
    <div class="music-filter-row"><VLabel>地区</VLabel><VChipGroup v-model="region"><VChip v-for="value in regions" :key="value" :value="value" filter tile>{{ value }}</VChip></VChipGroup></div>
    <div class="music-filter-row"><VLabel>排序</VLabel><VChipGroup v-model="sort" mandatory><VChip v-for="(label,value) in sorts" :key="value" :value="value" filter tile>{{ label }}</VChip></VChipGroup></div>
    <div class="music-filter-row"><VLabel>封面</VLabel><VChipGroup v-model="cover" mandatory><VChip value="all" filter tile>全部</VChip><VChip value="with_cover" filter tile>有封面</VChip></VChipGroup></div>
  </div>
</template>
<style scoped>
.music-filter-row{display:flex;align-items:center;gap:.75rem;margin-block-end:.5rem;}
.music-filter-row>.v-label{flex:0 0 auto;}
.music-filter-row :deep(.v-slide-group){min-inline-size:0;flex:1;}
</style>
