<!-- 本次音乐选项沿用 MP ReorganizeDialog，编辑与刮削共用。 -->
<script setup>
defineProps({ entity: String, disabled: Boolean });
const trackMatch=defineModel('trackMatch',{default:'strict'});
const cueEnable=defineModel('cueEnable',{default:true});
</script>

<template>
  <div>
    <VSwitch v-model="cueEnable" label="识别 CUE" color="primary" :disabled="disabled" hint="仅影响本次资料读取、预览和保存；已分轨专辑附带错误 CUE 时可关闭，整轨专辑请保持开启。" persistent-hint class="mb-4" />
    <VSelect v-if="entity==='album'" v-model="trackMatch" label="曲目对齐方式" prepend-inner-icon="mdi-playlist-check" :disabled="disabled" :items="[{title:'严格校验',value:'strict'},{title:'忽略时长',value:'ignore_duration'},{title:'跳过对齐验证',value:'sequential'}]" hint="用于手选专辑匹配；忽略时长仍须唯一匹配，按顺序套用要求曲目数一致。" persistent-hint />
    <VAlert v-if="entity==='album' && trackMatch==='sequential'" type="warning" variant="tonal" class="mt-3">按文件自然顺序对应发行版曲目，跳过曲名、曲序和时长核验。请核对预览；单首歌曲不能直接套用多首曲目的专辑。</VAlert>
  </div>
</template>
