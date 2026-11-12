<template>
  <VContainer fluid class="mp-page cover-page pa-0">
    <h1 class="sr-only">封面生成</h1>
    <div class="d-flex flex-wrap align-center justify-end ga-4 mb-6">
      <VBtn prepend-icon="mdi-image-multiple-outline" :loading="isGenerating" :disabled="isLoading || !isConfigReady" @click="runGenerateAllTask">
        生成全部封面
      </VBtn>
    </div>

    <VAlert title="操作提示" type="info" variant="tonal" class="mb-6">
      开启监控新入库可实时生成原生媒体库封面。如需自定义图片，可以在【其他设置】里填写自定义路径，例如：/config/custom_images。<br />
      然后在这个目录下新建想要自定义图片的媒体库子目录，例如：/config/custom_images/漫威宇宙，在这个目录下放入以1.jpg、2.jpg...命名的图片。
    </VAlert>

    <VAlert v-if="configLoadError" type="error" variant="tonal" title="封面配置加载失败" :text="configLoadError" class="mb-4">
      <template #append><VBtn variant="tonal" prepend-icon="mdi-refresh" :loading="isLoading" @click="fetchConfig">重试加载配置</VBtn></template>
    </VAlert>
    <VProgressLinear v-if="isLoading" indeterminate color="primary" class="mb-4" aria-label="正在加载封面配置" />
    <div v-if="isConfigReady && !isLoading" class="cover-config-cards">
      <VCard>
        <VCardItem title="基础设置">
          <template #prepend>
            <VAvatar color="primary" variant="tonal" rounded><VIcon icon="mdi-tune-variant" /></VAvatar>
          </template>
          <template #append>
            <VBtn color="default" variant="text" prepend-icon="mdi-content-save-outline" :loading="isSaving" @click="saveConfig('basic')">保存</VBtn>
          </template>
        </VCardItem>
        <VCardText>
          <VRow>
            <VCol cols="12" sm="6" lg="4"><VSwitch v-model="configData.enabled" label="启用封面生成" /></VCol>
            <VCol cols="12" sm="6" lg="4"><VSwitch v-model="configData.transfer_monitor" label="监控新入库" hint="新媒体入库后自动更新所在库封面" persistent-hint /></VCol>
            <VCol cols="12" sm="6" lg="4"><VSwitch v-model="configData.show_item_count" label="在封面上显示角标" hint="媒体项总数或是榜单类型" persistent-hint /></VCol>
            <VCol cols="12" sm="6"><VSelect v-model="configData.sort_by" label="封面图片来源排序" :items="sortOptions" item-title="label" item-value="value" /></VCol>
            <VCol cols="12" sm="6"><VSelect v-model="configData.max_safe_rating" label="默认分级上限" :items="ratingLimitOptions" item-title="label" item-value="value" hint="最高用于生成封面的分级海报，分级规则不受此限制。" persistent-hint /></VCol>
          </VRow>
          <VDivider class="my-5" />
          <fieldset class="library-selection">
            <legend class="text-subtitle-2 mb-2">选择要忽略的媒体库</legend>
            <div class="d-flex flex-wrap ga-2">
              <VCheckbox v-for="lib in libraryOptions" :key="lib.value" v-model="configData.exclude_libraries" :value="lib.value" :label="lib.label" hide-details />
            </div>
            <p v-if="!libraryOptions.length" class="text-body-2 text-medium-emphasis mb-0">暂无可选媒体库</p>
          </fieldset>
          <template v-if="configData.show_item_count">
            <VDivider class="my-5" />
            <VRow>
              <VCol cols="12" md="6">
                <div class="text-subtitle-2 mb-3">数字样式</div>
                <VBtnToggle v-model="configData.badge_style" mandatory color="primary" variant="tonal" divided>
                  <VBtn value="badge">徽章</VBtn>
                  <VBtn value="ribbon">缎带</VBtn>
                </VBtnToggle>
              </VCol>
              <VCol cols="12" md="6">
                <VSlider v-model="configData.badge_size_ratio" label="数字大小" :step="0.01" :min="0.08" :max="0.20" thumb-label="always">
                  <template #thumb-label="{ modelValue }">{{ (modelValue * 100).toFixed(0) }}%</template>
                </VSlider>
              </VCol>
            </VRow>
          </template>
        </VCardText>
      </VCard>

      <VCard>
        <VCardItem title="封面样式与标题">
          <template #prepend>
            <VAvatar color="primary" variant="tonal" rounded><VIcon icon="mdi-image-edit-outline" /></VAvatar>
          </template>
          <template #append>
            <VBtn color="default" variant="text" prepend-icon="mdi-content-save-outline" :loading="isSaving" @click="saveConfig(configData.tab || 'style-tab')">保存</VBtn>
          </template>
        </VCardItem>
        <VTabs v-model="configData.tab" class="px-4" color="primary" show-arrows>
          <VTab value="style-tab" prepend-icon="mdi-image-multiple-outline">封面风格</VTab>
          <VTab value="title-tab" prepend-icon="mdi-format-title">封面标题</VTab>
          <VTab value="single-tab" prepend-icon="mdi-image-outline">单图风格设置</VTab>
          <VTab value="multi-1-tab" prepend-icon="mdi-view-grid-outline">多图风格设置</VTab>
          <VTab value="others-tab" prepend-icon="mdi-cog-outline">其他设置</VTab>
        </VTabs>
        <VDivider />
        <VCardText>
          <VWindow :model-value="configData.tab || 'style-tab'">
            <VWindowItem value="style-tab">
              <VProgressLinear v-if="isPreviewLoading" indeterminate color="primary" class="mb-4" aria-label="正在更新封面预览" />
              <VRadioGroup v-model="configData.cover_style" name="cover-style-group">
                <div class="cover-style-grid">
                  <VCard v-for="style in styles" :key="style.value" variant="outlined" class="style-card" :class="{ 'is-selected': configData.cover_style === style.value }" @click="configData.cover_style = style.value">
                    <VImg :src="stylePreviews[style.value]" :aspect-ratio="16 / 9" cover :alt="`${style.title}封面预览`">
                      <template #placeholder><div class="d-flex align-center justify-center fill-height"><VProgressCircular indeterminate color="primary" /></div></template>
                    </VImg>
                    <div class="style-card-label">
                      <VRadio :value="style.value" :label="style.title" hide-details />
                      <VChip v-if="configData.cover_style === style.value" color="primary" size="small" variant="tonal">已选择</VChip>
                    </div>
                  </VCard>
                </div>
              </VRadioGroup>
            </VWindowItem>

            <VWindowItem value="title-tab">
              <div class="d-flex flex-column ga-4">
                <div v-for="(item, index) in titleConfigs" :key="item.id" class="title-config-row">
                  <VTextField v-model="item.library" label="媒体库名称" placeholder="与媒体库名称完全一致" hide-details />
                  <VTextField v-model="item.zh" label="中文标题" placeholder="封面上显示的中文" hide-details />
                  <VTextField v-model="item.en" label="英文标题" placeholder="封面上显示的英文" hide-details />
                  <VBtn icon="mdi-delete-outline" color="error" variant="tonal" class="title-config-remove" aria-label="删除标题配置" @click="removeTitleConfig(index)" />
                </div>
                <p v-if="!titleConfigs.length" class="text-body-2 text-medium-emphasis mb-0">新增媒体库名称及其封面标题。</p>
                <VBtn variant="tonal" prepend-icon="mdi-plus" class="align-self-start" @click="addTitleConfig">新增配置</VBtn>
              </div>
            </VWindowItem>

            <VWindowItem value="single-tab">
              <VAlert type="info" variant="tonal" class="mb-5">可填写字体下载链接，或上传字体到服务器后填写本地路径；本地路径优先于下载链接。</VAlert>
              <VRow>
                <VCol v-for="field in singleFields" :key="field.key" cols="12" md="6">
                  <VTextField type="number" v-if="field.type === 'number'" v-model.number="configData[field.key]" :label="field.label" :placeholder="field.placeholder" :step="field.step || 1" :hint="field.hint" :persistent-hint="!!field.hint" />
                  <VTextField v-else v-model="configData[field.key]" :label="field.label" placeholder="留空使用预设字体" :hint="field.hint" :persistent-hint="!!field.hint" />
                </VCol>
                <VCol cols="12"><VSwitch v-model="configData.single_use_primary" label="优先使用海报图" hint="不启用则优先使用背景图" persistent-hint /></VCol>
              </VRow>
            </VWindowItem>

            <VWindowItem value="multi-1-tab">
              <VAlert type="info" variant="tonal" class="mb-5">此页为“多图风格1”的专属设置。</VAlert>
              <VRow>
                <VCol v-for="field in multiFields" :key="field.key" cols="12" md="6">
                  <VTextField type="number" v-if="field.type === 'number'" v-model.number="configData[field.key]" :label="field.label" :placeholder="field.placeholder" :step="field.step || 1" :disabled="field.blur && !configData.multi_1_blur" :hint="field.hint" :persistent-hint="!!field.hint" />
                  <VTextField v-else v-model="configData[field.key]" :label="field.label" placeholder="留空使用预设字体" :disabled="configData.multi_1_use_main_font" />
                </VCol>
                <VCol cols="12" md="4"><VSwitch v-model="configData.multi_1_blur" label="启用模糊背景" hint="不启用则使用纯色渐变背景" persistent-hint /></VCol>
                <VCol cols="12" md="4"><VSwitch v-model="configData.multi_1_use_main_font" label="使用单图风格字体" hint="启用后将忽略本页的字体路径和链接设置" persistent-hint /></VCol>
                <VCol cols="12" md="4"><VSwitch v-model="configData.multi_1_use_primary" label="优先使用海报图" hint="多图风格建议开启" persistent-hint /></VCol>
              </VRow>
            </VWindowItem>

            <VWindowItem value="others-tab">
              <VRow>
                <VCol cols="12" md="6"><VTextField v-model="configData.covers_input" label="自定义图片目录（可选）" placeholder="/path/to/custom/images" /></VCol>
                <VCol cols="12" md="6"><VTextField v-model="configData.covers_output" label="封面另存目录（可选）" placeholder="/path/to/save/covers" /></VCol>
              </VRow>
            </VWindowItem>
          </VWindow>
        </VCardText>
      </VCard>
    </div>
  </VContainer>
</template>

<script setup>
import { ref, onMounted, onBeforeUnmount, watch } from 'vue';
import axios from 'axios';
import { useMessage } from '@/composables/useFeedback';
import * as yaml from 'js-yaml'; // ★ 新增：导入 js-yaml

// 导入静态图片数据
import { single_1, single_2, multi_1 } from '../assets/cover_styles/images.js';
const stylePreviews = ref({
  single_1: single_1,
  single_2: single_2,
  multi_1: multi_1,
});

const styles = [
  { title: "单图 1", value: "single_1" },
  { title: "单图 2", value: "single_2" },
  { title: "多图 1", value: "multi_1" }
];

const singleFields = [
  { key: 'zh_font_path_local', label: '中文字体（本地路径）', hint: '本地路径优先于下载链接' },
  { key: 'en_font_path_local', label: '英文字体（本地路径）' },
  { key: 'zh_font_url', label: '中文字体（下载链接）' },
  { key: 'en_font_url', label: '英文字体（下载链接）' },
  { key: 'zh_font_size', label: '中文字体大小比例', type: 'number', step: 0.1, placeholder: '1.0', hint: '相对于预设尺寸的比例，1为原始大小' },
  { key: 'en_font_size', label: '英文字体大小比例', type: 'number', step: 0.1, placeholder: '1.0' },
  { key: 'blur_size', label: '背景模糊程度', type: 'number', placeholder: '50', hint: '数字越大越模糊，默认 50' },
  { key: 'color_ratio', label: '背景颜色混合占比', type: 'number', step: 0.1, placeholder: '0.8', hint: '颜色所占的比例，0-1，默认 0.8' },
];
const multiFields = [
  { key: 'zh_font_path_multi_1_local', label: '中文字体（本地路径）' },
  { key: 'en_font_path_multi_1_local', label: '英文字体（本地路径）' },
  { key: 'zh_font_url_multi_1', label: '中文字体（下载链接）' },
  { key: 'en_font_url_multi_1', label: '英文字体（下载链接）' },
  { key: 'zh_font_size_multi_1', label: '中文字体大小比例', type: 'number', step: 0.1, placeholder: '1.0' },
  { key: 'en_font_size_multi_1', label: '英文字体大小比例', type: 'number', step: 0.1, placeholder: '1.0' },
  { key: 'blur_size_multi_1', label: '背景模糊程度', type: 'number', placeholder: '50', blur: true, hint: '需启用模糊背景' },
  { key: 'color_ratio_multi_1', label: '背景颜色混合占比', type: 'number', step: 0.1, placeholder: '0.8', blur: true, hint: '需启用模糊背景' },
];

const message = useMessage();
const isLoading = ref(true);
const isConfigReady = ref(false);
const configLoadError = ref('');
const isSaving = ref(false);
const isGenerating = ref(false);
const configData = ref({});
let persistedConfig = {};
const sectionFields = {
  basic: ['enabled', 'transfer_monitor', 'show_item_count', 'sort_by', 'max_safe_rating', 'exclude_libraries', 'badge_style', 'badge_size_ratio'],
  'style-tab': ['cover_style'],
  'title-tab': ['title_config'],
  'single-tab': ['zh_font_path_local', 'en_font_path_local', 'zh_font_url', 'en_font_url', 'zh_font_size', 'en_font_size', 'blur_size', 'color_ratio', 'single_use_primary'],
  'multi-1-tab': ['zh_font_path_multi_1_local', 'en_font_path_multi_1_local', 'zh_font_url_multi_1', 'en_font_url_multi_1', 'zh_font_size_multi_1', 'en_font_size_multi_1', 'blur_size_multi_1', 'color_ratio_multi_1', 'multi_1_blur', 'multi_1_use_main_font', 'multi_1_use_primary'],
  'others-tab': ['covers_input', 'covers_output'],
};
const ratingLimitOptions = ref([]);
// ★ 新增：用于封面标题UI的结构化数据
const titleConfigs = ref([]);

const libraryOptions = ref([]);

const sortOptions = [
  { label: "最新添加", value: "Latest" },
  { label: "随机", value: "Random" },
];

// ★ 新增：将YAML字符串解析为结构化数组
const parseYamlToData = (yamlString) => {
  try {
    if (!yamlString || yamlString.trim() === '') {
      titleConfigs.value = [];
      return;
    }
    const data = yaml.load(yamlString);
    if (!data) {
      titleConfigs.value = [];
      return;
    }
    if (typeof data !== 'object' || Array.isArray(data)) {
      message.warning('封面标题配置需使用“媒体库名称: 标题列表”的格式。');
      titleConfigs.value = [];
      return;
    }
    titleConfigs.value = Object.entries(data).map(([library, titles], index) => {
      const normalizedTitles = Array.isArray(titles) ? titles : [titles || '', ''];
      return {
      id: Date.now() + index, // 使用时间戳+索引确保key的唯一性
      library: library,
      zh: normalizedTitles[0] || '',
      en: normalizedTitles[1] || ''
      };
    });
  } catch (e) {
    message.error('封面标题配置 (YAML) 格式解析失败，请检查。');
    console.error("YAML Parse Error:", e);
    titleConfigs.value = []; // 解析失败则清空，避免UI出错
  }
};

// ★ 新增：将结构化数组转换回YAML字符串
const convertDataToYaml = () => {
  try {
    const dataObject = titleConfigs.value.reduce((acc, item) => {
      // 过滤掉媒体库名称为空的无效配置
      if (item.library && item.library.trim() !== '') {
        acc[item.library.trim()] = [item.zh || '', item.en || ''];
      }
      return acc;
    }, {});

    if (Object.keys(dataObject).length === 0) {
      return ''; // 如果没有有效配置，返回空字符串
    }

    return yaml.dump(dataObject);
  } catch (e) {
    message.error('生成封面标题配置失败。');
    console.error("YAML Dump Error:", e);
    return configData.value.title_config; // 转换失败则返回原始值，防止数据丢失
  }
};

// ★ 新增：添加一行新的标题配置
const addTitleConfig = () => {
  titleConfigs.value.push({
    id: Date.now(),
    library: '',
    zh: '',
    en: ''
  });
};

// ★ 新增：移除指定索引的标题配置
const removeTitleConfig = (index) => {
  titleConfigs.value.splice(index, 1);
};


const fetchConfig = async () => {
  isLoading.value = true;
  isConfigReady.value = false;
  configLoadError.value = '';
  ++previewGeneration;
  clearTimeout(previewUpdateTimeout);
  isPreviewLoading.value = false;
  try {
    const response = await axios.get('/api/config/cover_generator');
    if (!response.data || typeof response.data !== 'object' || Array.isArray(response.data)) {
      throw new Error('封面配置返回的数据无效，请重试。');
    }
    configData.value = response.data;
    persistedConfig = structuredClone(response.data);
    configData.value.tab ||= 'style-tab';
    // ★ 修改：获取配置后，立即解析YAML
    parseYamlToData(configData.value.title_config);
    isConfigReady.value = true;
  } catch (error) {
    configLoadError.value = error.response?.data?.error || error.response?.data?.message || error.message || '加载封面生成器配置失败。';
  } finally {
    isLoading.value = false;
  }
};

const fetchLibraryOptions = async () => {
  try {
    const response = await axios.get('/api/config/cover_generator/libraries');
    libraryOptions.value = response.data;
  } catch (error) {
    message.error('获取媒体库列表失败，请检查后端。');
  }
};

const saveConfig = async (section) => {
  if (isSaving.value) return;
  if (isLoading.value || !isConfigReady.value) {
    message.warning('请先成功加载封面配置，再保存。');
    return;
  }
  isSaving.value = true;
  if (section === 'title-tab') configData.value.title_config = convertDataToYaml();
  const payload = { ...persistedConfig, ...Object.fromEntries(sectionFields[section].map(key => [key, configData.value[key]])) };
  try {
    await axios.post('/api/config/cover_generator', payload);
    persistedConfig = JSON.parse(JSON.stringify(payload));
    message.success('配置已成功保存！');
  } catch (error) {
    message.error(error.response?.data?.error || error.response?.data?.message || '保存配置失败。');
  } finally {
    isSaving.value = false;
  }
};

const runGenerateAllTask = async () => {
  if (isGenerating.value) return;
  if (isLoading.value || !isConfigReady.value) {
    message.warning('请先成功加载封面配置，再生成封面。');
    return;
  }
  isGenerating.value = true;
  try {
    const response = await axios.post('/api/tasks/run', { task_name: 'generate-all-covers' });
    const result = response.data || {};
    const details = [...(result.warning_details || []), ...(result.failures || [])]
      .map(detail => typeof detail === 'string' ? detail : detail.reason || JSON.stringify(detail));
    if (result.success === false || result.failed > 0 || result.failures?.length) {
      message.error([result.message || '部分媒体库封面生成失败', ...details].join('；'), { duration: 0 });
    } else if (response.status === 207 || result.warnings > 0 || details.length) {
      message.warning([result.message || '封面生成完成，有项目需要检查', ...details].join('；'), { duration: 0 });
    } else {
      message.success(result.message || '媒体库封面生成完成。');
    }
  } catch (error) {
    message.error(error.response?.data?.error || error.response?.data?.message || error.message || '触发任务失败，请检查后端日志。', { duration: 0 });
  } finally {
    isGenerating.value = false;
  }
};

// --- 实时预览部分保持不变 ---
let previewUpdateTimeout = null;
let previewGeneration = 0;
const isPreviewLoading = ref(false);

function debounceUpdatePreview() {
  const generation = ++previewGeneration;
  isPreviewLoading.value = true;
  if (previewUpdateTimeout) {
    clearTimeout(previewUpdateTimeout);
  }
  previewUpdateTimeout = setTimeout(() => updateAllPreviews(generation), 500);
}

// ★★★ 修改 2: 新增获取动态分级选项的函数 ★★★
const fetchRatingOptions = async () => {
  try {
    // 调用之前在 MappingManager 中用过的 API
    const response = await axios.get('/api/mappings/config/rating_mapping');
    const mapping = response.data || {};

    // 使用 Map 来去重 (Key: emby_value, Value: label)
    // 目的：多个国家可能有相同的分级值(比如8)，我们只需要显示一个代表性的中文标签
    const valueMap = new Map();

    // 遍历所有国家的配置
    Object.values(mapping).forEach(rules => {
      if (Array.isArray(rules)) {
        rules.forEach(rule => {
          // 只有当 emby_value 有效且有中文标签时才收集
          if (rule.emby_value !== null && rule.emby_value !== undefined && rule.label) {
            // 如果这个分级值还没被收录，或者当前是"US"的规则(通常US最标准)，则更新
            // 这里简化逻辑：只要 Map 里没有这个值，就存进去。
            // 这样通常会保留优先级靠前的国家的标签（取决于后端返回顺序）
            if (!valueMap.has(rule.emby_value)) {
              valueMap.set(rule.emby_value, rule.label);
            }
          }
        });
      }
    });

    // 将 Map 转为数组，并按分级值从小到大排序
    const dynamicOptions = Array.from(valueMap.entries())
      .sort((a, b) => a[0] - b[0])
      .map(([val, label]) => ({
        label: `${label} (等级 ${val})`, // 显示格式：成人 (等级 9)
        value: val
      }));

    // ★★★ 始终在最后追加“无限制”选项 ★★★
    dynamicOptions.push({ label: '无限制 (全视之眼)', value: 999 });

    ratingLimitOptions.value = dynamicOptions;

  } catch (error) {
    console.error("获取分级映射失败", error);
    // 兜底：如果获取失败，使用最基础的默认值
    ratingLimitOptions.value = [
      { label: '青少年 (等级 8) - 默认', value: 8 },
      { label: '无限制 (全视之眼)', value: 999 }
    ];
  }
};

async function updateAllPreviews(generation = ++previewGeneration) {
  if (!isConfigReady.value) return;
  if (!configData.value.show_item_count) {
    stylePreviews.value.single_1 = single_1;
    stylePreviews.value.single_2 = single_2;
    stylePreviews.value.multi_1 = multi_1;
    isPreviewLoading.value = false;
    return;
  }

  try {
    const previewsToUpdate = [
      { key: 'single_1', base_image: single_1 },
      { key: 'single_2', base_image: single_2 },
      { key: 'multi_1', base_image: multi_1 },
    ];

    const promises = previewsToUpdate.map(p =>
      axios.post('/api/config/cover_generator/preview', {
        base_image: p.base_image,
        badge_style: configData.value.badge_style,
        badge_size_ratio: configData.value.badge_size_ratio,
      })
    );

    const results = await Promise.all(promises);
    if (generation !== previewGeneration) return;

    stylePreviews.value.single_1 = results[0].data.image;
    stylePreviews.value.single_2 = results[1].data.image;
    stylePreviews.value.multi_1 = results[2].data.image;

  } catch (error) {
    if (generation === previewGeneration) message.error("实时预览失败");
  } finally {
    if (generation === previewGeneration) isPreviewLoading.value = false;
  }
}

watch(
  () => [
    configData.value.show_item_count,
    configData.value.badge_style,
    configData.value.badge_size_ratio
  ],
  () => {
    if (isConfigReady.value) debounceUpdatePreview();
  },
  { deep: true } // 建议对复杂对象监听开启deep
);

onMounted(() => {
  fetchConfig();
  fetchLibraryOptions();
  fetchRatingOptions();
});
onBeforeUnmount(() => {
  ++previewGeneration;
  clearTimeout(previewUpdateTimeout);
});
</script>

<style scoped>
.cover-config-cards { display: grid; gap: 24px; min-width: 0; }
.cover-page :deep(.v-card-title), .cover-page :deep(.v-card-subtitle) { white-space: normal; overflow-wrap: anywhere; }
.library-selection { min-width: 0; border: 0; padding: 0; }
.cover-style-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 280px), 1fr)); gap: 16px; }
.style-card { min-width: 0; cursor: pointer; }
.style-card.is-selected { border-color: rgb(var(--v-theme-primary)); box-shadow: 0 0 0 1px rgb(var(--v-theme-primary)); }
.style-card-label { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 12px; padding: 12px 16px; }
.title-config-row { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)) 44px; align-items: start; gap: 12px; }
.title-config-remove { justify-self: end; }
@media (max-width: 680px) {
  .title-config-row { grid-template-columns: minmax(0, 1fr); border: 1px solid rgba(var(--v-border-color), var(--v-border-opacity)); padding: 16px; border-radius: 12px; }
  .cover-page :deep(.v-card-item) { grid-template-areas: 'prepend content' 'append append'; grid-template-columns: max-content minmax(0, 1fr); row-gap: 12px; }
  .cover-page :deep(.v-card-item__append) { padding-inline-start: 0; justify-self: start; }
}
</style>
