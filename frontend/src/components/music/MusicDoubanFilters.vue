<!-- Music-only filters from MoviePilot-Frontend v3 DoubanView (MIT); see ../../../third-party/MoviePilot-LICENSE. -->
<script setup>
import {ref,watch,computed} from 'vue';
import AppFilterBar from '@/components/filter/AppFilterBar.vue';
const activeCount=computed(()=>Number(!!region.value)+Number(sort.value!=='U')+Number(cover.value!=='all'));
const emit=defineEmits(['change']);
const category=ref('流行'), region=ref(''), sort=ref('U'), cover=ref('all');
const categories=['流行','摇滚','民谣','电子','爵士','古典','原声','独立音乐','纯音乐','R&B','hip-hop'];
const regions=['华语','欧美','日本','韩国','内地','香港','台湾','美国','英国','粤语'];
const sorts={U:'综合排序',S:'评分排序',R:'发行时间',O:'标记人数'};
watch([category,region,sort,cover],()=>emit('change',{tags:[category.value,region.value].filter(Boolean).join(','),douban_sort:sort.value,with_cover:cover.value==='with_cover'}));
</script>
<template>
  <AppFilterBar class="music-douban-filters" :active-count="activeCount">
    <template #primary>
    <div class="app-filter-row"><VLabel class="app-filter-label">风格</VLabel><VChipGroup class="app-filter-chips" v-model="category" mandatory><VChip v-for="value in categories" :key="value" :value="value">{{ value==='hip-hop' ? 'Hip-Hop' : value }}</VChip></VChipGroup></div>
    </template>
    <div class="app-filter-row"><VLabel class="app-filter-label">地区</VLabel><VChipGroup class="app-filter-chips" v-model="region"><VChip v-for="value in regions" :key="value" :value="value">{{ value }}</VChip></VChipGroup></div>
    <div class="app-filter-row"><VLabel class="app-filter-label">排序</VLabel><VChipGroup class="app-filter-chips" v-model="sort" mandatory><VChip v-for="(label,value) in sorts" :key="value" :value="value">{{ label }}</VChip></VChipGroup></div>
    <div class="app-filter-row"><VLabel class="app-filter-label">封面</VLabel><VChipGroup class="app-filter-chips" v-model="cover" mandatory><VChip value="all">全部</VChip><VChip value="with_cover">有封面</VChip></VChipGroup></div>
  </AppFilterBar>
</template>
