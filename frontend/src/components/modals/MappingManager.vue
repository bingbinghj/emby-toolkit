<!-- Metadata mappings use the MoviePilot v3 Vuetify form and dialog components. -->
<template>
  <div class="mapping-manager">
    <VProgressLinear v-if="isLoading" indeterminate color="primary" aria-label="正在加载映射配置" class="mb-4" />
    <VAlert v-if="loadError" type="error" variant="tonal" title="映射配置加载失败" :text="loadError" class="mb-4">
      <template #append><VBtn variant="text" :loading="isLoading" @click="fetchData">重新加载</VBtn></template>
    </VAlert>
    <VTabs v-model="activeTab" :disabled="!isReady" show-arrows class="mapping-tabs">
      <VTab value="keywords" prepend-icon="mdi-tag-outline">关键词</VTab>
      <VTab value="studios" prepend-icon="mdi-domain">工作室</VTab>
      <VTab value="countries" prepend-icon="mdi-earth">国家/地区</VTab>
      <VTab value="languages" prepend-icon="mdi-translate">原语言</VTab>
      <VTab value="rating_labels" prepend-icon="mdi-label-outline">分级标签</VTab>
      <VTab value="ratings" prepend-icon="mdi-shield-account-outline">分级制度</VTab>
    </VTabs>

    <VWindow v-if="isReady" v-model="activeTab" class="mapping-content">
      <VWindowItem value="keywords" eager>
        <VAlert type="info" variant="tonal" class="mb-4">将中文标签映射到 TMDb 的英文关键词或 ID。</VAlert>
        <div ref="keywordListRef" class="sortable-list">
          <VCard v-for="(item, index) in keywordList" :key="item.id" variant="outlined" class="list-item">
            <div class="col-handle drag-handle" aria-label="拖动调整顺序"><VIcon icon="mdi-drag-vertical" /></div>
            <VTextField v-model="item.label" label="中文标签" placeholder="例如：丧尸" :disabled="isSaving" class="col-label" />
            <VTextField v-model="item.en" label="英文关键词" placeholder="zombie" :disabled="isSaving" class="col-en" />
            <VTextField v-model="item.ids" label="TMDb IDs" placeholder="12377" :disabled="isSaving" class="col-ids" />
            <VBtn icon="mdi-delete-outline" variant="text" color="error" :disabled="isSaving" aria-label="删除此映射" @click="removeItem(keywordList, index)" />
          </VCard>
        </div>
        <VBtn block variant="outlined" prepend-icon="mdi-plus" class="mt-4" :disabled="isSaving" @click="addItem(keywordList)">添加关键词</VBtn>
      </VWindowItem>

      <VWindowItem value="studios" eager>
        <VAlert type="info" variant="tonal" class="mb-4">配置出品公司或播出平台。请区分电影制作公司和电视播出平台 ID。</VAlert>
        <div ref="studioListRef" class="sortable-list">
          <VCard v-for="(item, index) in studioList" :key="item.id" variant="outlined" class="list-item studio-item">
            <div class="col-handle drag-handle" aria-label="拖动调整顺序"><VIcon icon="mdi-drag-vertical" /></div>
            <VTextField v-model="item.label" label="中文简称" placeholder="例如：爱奇艺" :disabled="isSaving" class="col-label" />
            <VTextField v-model="item.en" label="英文原名" placeholder="iQIYI" :disabled="isSaving" class="col-en" />
            <VTextField v-model="item.company_ids" label="制作公司 IDs（电影）" placeholder="Company ID" :disabled="isSaving" class="col-ids" />
            <VTextField v-model="item.network_ids" label="播出平台 IDs（电视）" placeholder="Network ID" :disabled="isSaving" class="col-ids" />
            <VBtn icon="mdi-delete-outline" variant="text" color="error" :disabled="isSaving" aria-label="删除此映射" @click="removeItem(studioList, index)" />
          </VCard>
        </div>
        <VBtn block variant="outlined" prepend-icon="mdi-plus" class="mt-4" :disabled="isSaving" @click="addItem(studioList, 'studio')">添加工作室</VBtn>
      </VWindowItem>

      <VWindowItem value="countries" eager>
        <VAlert type="info" variant="tonal" class="mb-4">配置国家/地区代码映射。ISO 代码用于 API 筛选，别名用于元数据翻译匹配。</VAlert>
        <div ref="countryListRef" class="sortable-list">
          <VCard v-for="(item, index) in countryList" :key="item.id" variant="outlined" class="list-item">
            <div class="col-handle drag-handle" aria-label="拖动调整顺序"><VIcon icon="mdi-drag-vertical" /></div>
            <VTextField v-model="item.label" label="中文名称" placeholder="例如：美国" :disabled="isSaving" class="col-label" />
            <VTextField v-model="item.value" label="ISO 代码" placeholder="US" :disabled="isSaving" class="col-en" />
            <VTextField v-model="item.aliases" label="别名（逗号分隔）" placeholder="USA, United States" :disabled="isSaving" class="col-extra" />
            <VBtn icon="mdi-delete-outline" variant="text" color="error" :disabled="isSaving" aria-label="删除此映射" @click="removeItem(countryList, index)" />
          </VCard>
        </div>
        <VBtn block variant="outlined" prepend-icon="mdi-plus" class="mt-4" :disabled="isSaving" @click="addItem(countryList, 'country')">添加国家/地区</VBtn>
      </VWindowItem>

      <VWindowItem value="languages" eager>
        <VAlert type="info" variant="tonal" class="mb-4">配置筛选器使用的语言代码。别名兼容历史语言代码和媒体信息标记，首个别名应填写三位语言代码。</VAlert>
        <div ref="languageListRef" class="sortable-list">
          <VCard v-for="(item, index) in languageList" :key="item.id" variant="outlined" class="list-item">
            <div class="col-handle drag-handle" aria-label="拖动调整顺序"><VIcon icon="mdi-drag-vertical" /></div>
            <VTextField v-model="item.label" label="中文名称" placeholder="例如：粤语" :disabled="isSaving" class="col-label" />
            <VTextField v-model="item.value" label="ISO 代码" placeholder="cn" :disabled="isSaving" class="col-en" />
            <VTextField v-model="item.aliases" label="别名（逗号分隔）" placeholder="yue, cmn" :disabled="isSaving" class="col-extra" />
            <VBtn icon="mdi-delete-outline" variant="text" color="error" :disabled="isSaving" aria-label="删除此映射" @click="removeItem(languageList, index)" />
          </VCard>
        </div>
        <VBtn block variant="outlined" prepend-icon="mdi-plus" class="mt-4" :disabled="isSaving" @click="addItem(languageList, 'language')">添加语言</VBtn>
      </VWindowItem>

      <VWindowItem value="rating_labels" eager>
        <VAlert type="info" variant="tonal" class="mb-4">定义可用的中文分级名称，如“全年龄”、“限制级”。这些标签用于分级制度映射和分级筛选器。</VAlert>
        <div ref="ratingLabelListRef" class="sortable-list">
          <VCard v-for="(item, index) in ratingLabelList" :key="item.id" variant="outlined" class="list-item">
            <div class="col-handle drag-handle" aria-label="拖动调整顺序"><VIcon icon="mdi-drag-vertical" /></div>
            <VTextField v-model="item.label" label="中文分级名称" placeholder="例如：儿童专区" :disabled="isSaving" class="simple-label" />
            <VBtn icon="mdi-delete-outline" variant="text" color="error" :disabled="isSaving" aria-label="删除此映射" @click="removeItem(ratingLabelList, index)" />
          </VCard>
        </div>
        <VBtn block variant="outlined" prepend-icon="mdi-plus" class="mt-4" :disabled="isSaving" @click="addItem(ratingLabelList, 'simple')">添加分级标签</VBtn>
      </VWindowItem>

      <VWindowItem value="ratings" eager>
        <VAlert type="info" variant="tonal" class="mb-4">配置各国分级的查找优先级及中文映射。Emby 等级值用于封面权限控制：G=1、PG=5、PG-13=8、R=9、NC-17=10、XXX=15。</VAlert>
        <VCard variant="outlined" class="mb-4">
          <VCardTitle class="text-subtitle-1">优先级策略</VCardTitle>
          <VCardSubtitle>拖拽调整查找顺序</VCardSubtitle>
          <VCardText>
            <div ref="ratingPriorityRef" class="priority-tags">
              <VChip v-for="(country, index) in ratingPriority" :key="country" closable variant="tonal" class="priority-tag drag-handle" :color="country === 'ORIGIN' ? 'success' : undefined" :disabled="isSaving" @click:close="removePriority(index)">
                {{ country === 'ORIGIN' ? '原产国（自动）' : getCountryName(country) }}
              </VChip>
              <VMenu>
                <template #activator="{ props: menuProps }">
                  <VBtn v-bind="menuProps" variant="outlined" size="small" prepend-icon="mdi-plus" :disabled="isSaving || !availablePriorityOptions.length">添加国家</VBtn>
                </template>
                <VList density="compact">
                  <VListItem v-for="option in availablePriorityOptions" :key="option.value" :title="option.label" @click="addPriority(option.value)" />
                </VList>
              </VMenu>
            </div>
          </VCardText>
        </VCard>

        <div class="rating-heading">
          <div class="text-subtitle-1">国家/地区分级表</div>
          <VBtn variant="text" size="small" prepend-icon="mdi-plus" :disabled="isSaving || !availablePriorityOptions.length" @click="addRatingCountry">添加国家</VBtn>
        </div>
        <VExpansionPanels v-model="expandedCountries" multiple class="rating-container">
          <VExpansionPanel v-for="countryCode in sortedMappingKeys" :key="countryCode" :value="countryCode">
            <div class="rating-panel-heading">
              <VExpansionPanelTitle>{{ getCountryName(countryCode) }}</VExpansionPanelTitle>
              <VBtn variant="text" color="error" size="small" prepend-icon="mdi-delete-outline" :disabled="isSaving" @click.stop="removeRatingCountry(countryCode)">删除整组</VBtn>
            </div>
            <VExpansionPanelText>
              <div v-for="(rule, index) in ratingMapping[countryCode]" :key="index" class="rating-rule">
                <VTextField v-model="rule.code" label="原始分级代码" placeholder="例如：R" :disabled="isSaving" class="rating-code" />
                <VNumberInput v-model="rule.emby_value" label="Emby 等级值" :min="0" :max="100" :precision="0" control-variant="stacked" :disabled="isSaving" class="rating-value" />
                <VIcon icon="mdi-arrow-right" size="20" class="rating-rule-arrow" />
                <VCombobox v-model="rule.label" :items="dynamicRatingOptions" item-title="label" item-value="value" :return-object="false" label="中文分级标签" placeholder="选择或输入标签" :disabled="isSaving" class="rating-label" />
                <VBtn icon="mdi-delete-outline" variant="text" color="error" :disabled="isSaving" aria-label="删除此分级规则" @click="removeRatingRule(countryCode, index)" />
              </div>
              <VBtn variant="outlined" prepend-icon="mdi-plus" :disabled="isSaving" class="mt-2" @click="addRatingRule(countryCode)">添加分级规则</VBtn>
            </VExpansionPanelText>
          </VExpansionPanel>
        </VExpansionPanels>
        <VAlert v-if="!sortedMappingKeys.length" type="info" variant="tonal" class="mt-3" title="尚未配置分级映射" text="添加国家后即可定义分级规则。" />
      </VWindowItem>
    </VWindow>

    <div class="footer-actions">
      <VBtn variant="outlined" color="warning" prepend-icon="mdi-restore" :disabled="isSaving || !isReady" @click="handleRestoreDefaults">恢复当前页默认</VBtn>
      <VBtn prepend-icon="mdi-content-save-outline" :loading="isSaving" :disabled="isLoading || !isReady" @click="handleSave">保存所有配置</VBtn>
    </div>

    <VDialog v-model="showCountryDialog" max-width="440">
      <VCard title="添加分级国家">
        <VCardText>
          <VAutocomplete v-model="newPriorityCountry" :items="availablePriorityOptions" item-title="label" item-value="value" :return-object="false" label="国家/地区" placeholder="搜索国家" autofocus @update:model-value="selectRatingCountry" />
        </VCardText>
        <VCardActions><VSpacer /><VBtn variant="text" @click="showCountryDialog = false">取消</VBtn></VCardActions>
      </VCard>
    </VDialog>
  </div>
</template>

<script setup>
import { ref, onMounted, nextTick, watch, onUnmounted, computed } from 'vue';
import axios from 'axios';
import Sortable from 'sortablejs';
import { useMessage, useDialog } from '@/composables/useFeedback';

const message = useMessage();
const dialog = useDialog();

const activeTab = ref('keywords');

// 数据列表
const keywordList = ref([]);
const studioList = ref([]);
const countryList = ref([]);
const languageList = ref([]);
const ratingLabelList = ref([]); // 新增：分级标签列表

// 分级映射数据
const ratingMapping = ref({}); // 结构: { "US": [{code: 'R', label: '限制级'}] }
const ratingPriority = ref([]); // 结构: ['ORIGIN', 'US']
const newPriorityCountry = ref(null);
const showCountryDialog = ref(false);
const expandedCountries = ref(['US']);
const ratingPriorityRef = ref(null);

const isSaving = ref(false);
const isLoading = ref(false);
const isReady = ref(false);
const loadError = ref('');

// 拖拽 DOM 引用
const keywordListRef = ref(null);
const studioListRef = ref(null);
const countryListRef = ref(null);
const languageListRef = ref(null);
const ratingLabelListRef = ref(null);

let sortables = [];

const generateId = () => '_' + Math.random().toString(36).substr(2, 9);

// 动态生成分级下拉框选项
const dynamicRatingOptions = computed(() => {
  return ratingLabelList.value
    .filter(item => item.label && item.label.trim())
    .map(item => ({
      label: item.label,
      value: item.label
    }));
});

// 通用数据处理：后端 -> 前端
const processBackendData = (data, type) => {
  let list = Array.isArray(data) ? data : [];

  if (list.length > 0 && typeof list[0] === 'string') {
    list = list.map(s => ({ label: s }));
  }

  return list.map(item => {
    const base = { id: generateId(), label: item.label || '' };

    if (type === 'country' || type === 'language') {
      base.value = item.value || '';
      base.aliases = Array.isArray(item.aliases) ? item.aliases.join(', ') : (item.aliases || '');
    } else if (type === 'simple') {
      // simple logic
    } else if (type === 'studio') {
      // ★★★ 修改：工作室特殊处理，读取分离的 IDs ★★★
      base.en = Array.isArray(item.en) ? item.en.join(', ') : (item.en || '');

      // 读取 company_ids
      base.company_ids = Array.isArray(item.company_ids)
        ? item.company_ids.join(', ')
        : (item.company_ids || '');

      // 读取 network_ids
      base.network_ids = Array.isArray(item.network_ids)
        ? item.network_ids.join(', ')
        : (item.network_ids || '');

      // 兼容旧数据：如果只有 ids 且上面两个都空，暂时填入 company_ids (或者你可以选择留空让用户自己分)
      if (!base.company_ids && !base.network_ids && item.ids) {
         const oldIds = Array.isArray(item.ids) ? item.ids.join(', ') : item.ids;
         // 这里为了安全，可以不自动填充，或者默认填入 company_ids
         // base.company_ids = oldIds;
      }

    } else {
      // 关键词等其他类型保持原样
      base.en = Array.isArray(item.en) ? item.en.join(', ') : (item.en || '');
      base.ids = Array.isArray(item.ids) ? item.ids.join(', ') : (item.ids || '');
    }
    return base;
  });
};

// 通用数据处理：前端 -> 后端
const processFrontendData = (list, type) => {
  return list.map(item => {
    if (!item.label || !item.label.trim()) return null;
    const base = { label: item.label.trim() };

    if (type === 'country' || type === 'language') {
      base.value = item.value ? item.value.trim() : '';
      base.aliases = item.aliases ? item.aliases.split(',').map(s => s.trim()).filter(s => s) : [];
    } else if (type === 'simple') {
      // simple logic
    } else if (type === 'studio') {
      // ★★★ 修改：工作室特殊处理，保存分离的 IDs ★★★
      base.en = item.en ? item.en.split(',').map(s => s.trim()).filter(s => s) : [];

      // 处理 Company IDs
      base.company_ids = item.company_ids
        ? item.company_ids.toString().split(',').map(s => s.trim()).filter(s => s).map(Number)
        : [];

      // 处理 Network IDs
      base.network_ids = item.network_ids
        ? item.network_ids.toString().split(',').map(s => s.trim()).filter(s => s).map(Number)
        : [];

      // 不再保存通用的 'ids' 字段，强制分离

    } else {
      // 关键词等其他类型
      base.en = item.en ? item.en.split(',').map(s => s.trim()).filter(s => s) : [];
      base.ids = item.ids ? item.ids.toString().split(',').map(s => s.trim()).filter(s => s).map(Number) : [];
    }
    return base;
  }).filter(item => item !== null);
};

// 初始化 Sortable
const initSortable = (el, listRef) => {
  if (!el) return null;
  const s = Sortable.create(el, {
    handle: '.drag-handle',
    animation: 150,
    ghostClass: 'sortable-ghost',
    onEnd: (evt) => {
      const { oldIndex, newIndex } = evt;
      if (oldIndex === newIndex) return;
      const item = listRef.value.splice(oldIndex, 1)[0];
      listRef.value.splice(newIndex, 0, item);
    }
  });
  sortables.push(s);
  return s;
};

// 分级优先级拖拽初始化
const setupRatingSortable = () => {
  if (ratingPriorityRef.value) {
    const s = Sortable.create(ratingPriorityRef.value, {
      animation: 150,
      ghostClass: 'sortable-ghost',
      draggable: '.priority-tag',
      onEnd: (evt) => {
        const item = ratingPriority.value.splice(evt.oldIndex, 1)[0];
        ratingPriority.value.splice(evt.newIndex, 0, item);
      }
    });
    sortables.push(s);
  }
};

// 监听 DOM 变化初始化拖拽
const setupSortables = () => {
  sortables.forEach(s => s?.destroy());
  sortables = [];

  nextTick(() => {
    if (activeTab.value === 'ratings') {
      setupRatingSortable();
    } else {
      if (keywordListRef.value) initSortable(keywordListRef.value, keywordList);
      if (studioListRef.value) initSortable(studioListRef.value, studioList);
      if (countryListRef.value) initSortable(countryListRef.value, countryList);
      if (languageListRef.value) initSortable(languageListRef.value, languageList);
      if (ratingLabelListRef.value) initSortable(ratingLabelListRef.value, ratingLabelList);
    }
  });
};

watch(activeTab, () => {
  setupSortables();
});

// 初始化数据
const fetchData = async () => {
  if (isLoading.value) return;
  isLoading.value = true;
  isReady.value = false;
  loadError.value = '';
  try {
    const [kwRes, stRes, cnRes, lgRes, rMapRes, rPrioRes, rLabelRes] = await Promise.all([
      axios.get('/api/mappings/config/keyword_mapping'),
      axios.get('/api/mappings/config/studio_mapping'),
      axios.get('/api/mappings/config/country_mapping'),
      axios.get('/api/mappings/config/language_mapping'),
      axios.get('/api/mappings/config/rating_mapping'),
      axios.get('/api/mappings/config/rating_priority'),
      axios.get('/api/mappings/config/unified_ratings')
    ]);

    if (![kwRes, stRes, cnRes, lgRes, rLabelRes, rPrioRes].every(response => Array.isArray(response.data))
      || !rMapRes.data || typeof rMapRes.data !== 'object' || Array.isArray(rMapRes.data)) {
      throw new Error('映射配置返回格式不完整，请重新加载。');
    }
    const loaded = {
      keywords: processBackendData(kwRes.data, 'keyword'),
      studios: processBackendData(stRes.data, 'studio'),
      countries: processBackendData(cnRes.data, 'country'),
      languages: processBackendData(lgRes.data, 'language'),
      labels: processBackendData(rLabelRes.data, 'simple'),
    };
    keywordList.value = loaded.keywords;
    studioList.value = loaded.studios;
    countryList.value = loaded.countries;
    languageList.value = loaded.languages;
    ratingLabelList.value = loaded.labels;
    ratingMapping.value = rMapRes.data;
    ratingPriority.value = rPrioRes.data;
    isReady.value = true;
    setupSortables();
  } catch (e) {
    console.error(e);
    loadError.value = e.response?.data?.error || e.response?.data?.message || e.message || '无法完整加载映射配置，请重新加载。';
    message.error('加载配置失败');
  } finally {
    isLoading.value = false;
  }
};

const addItem = (list, type = 'normal') => {
  const item = { id: generateId(), label: '' };
  if (type === 'country' || type === 'language') {
    item.value = ''; item.aliases = '';
  } else if (type === 'simple') {
    // 仅需要 label
  } else if (type === 'studio' || list === studioList.value) {
    // 注意：这里判断 list === studioList 或者传 type='studio' 都可以
    item.en = '';
    item.company_ids = '';
    item.network_ids = '';
  } else {
    item.en = ''; item.ids = '';
  }
  list.push(item);
};

const removeItem = (list, index) => {
  list.splice(index, 1);
};

// 分级相关辅助函数
const getCountryName = (code) => {
  const found = countryList.value.find(c => c.value === code);
  return found ? `${found.label} (${code})` : code;
};

const availablePriorityOptions = computed(() => {
  const opts = countryList.value.map(c => ({ label: c.label, value: c.value }));
  return opts.filter(o => !ratingPriority.value.includes(o.value));
});

const sortedMappingKeys = computed(() => {
  return ratingPriority.value.filter(code => code !== 'ORIGIN');
});

const addPriority = (val) => {
  if (val && !ratingPriority.value.includes(val)) {
    // 1. 加入优先级列表 (上)
    ratingPriority.value.push(val);

    // 2. 自动初始化映射表 (下)
    if (!ratingMapping.value[val]) {
      ratingMapping.value[val] = [{ code: '', label: '全年龄' }];
    }

    newPriorityCountry.value = null;
  }
};

const removePriority = (index) => {
  const codeToRemove = ratingPriority.value[index];

  ratingPriority.value.splice(index, 1);

  if (codeToRemove !== 'ORIGIN') {
    delete ratingMapping.value[codeToRemove];
  }
};

const removeRatingCountry = (code) => {
  const index = ratingPriority.value.indexOf(code);
  if (index !== -1) {
    removePriority(index); // 直接调用上面的联动删除函数
  }
};

const addRatingCountry = () => {
  newPriorityCountry.value = null;
  showCountryDialog.value = true;
};

const selectRatingCountry = (value) => {
  if (!value) return;
  addPriority(value);
  showCountryDialog.value = false;
};

const addRatingRule = (countryCode) => {
  if (!Array.isArray(ratingMapping.value[countryCode])) ratingMapping.value[countryCode] = [];
  ratingMapping.value[countryCode].push({ code: '', label: '全年龄', emby_value: null });
};

const removeRatingRule = (countryCode, index) => {
  ratingMapping.value[countryCode].splice(index, 1);
};

const handleRestoreDefaults = () => {
  // 特殊处理 ratings Tab
  if (activeTab.value === 'ratings') {
    dialog.warning({
      title: '恢复默认预设',
      content: `确定要恢复【分级制度】的默认预设吗？当前未保存的修改将丢失。`,
      positiveText: '确定',
      negativeText: '取消',
      onPositiveClick: async () => {
        try {
          const [rMapRes, rPrioRes] = await Promise.all([
            axios.get('/api/mappings/config/rating_mapping/defaults'),
            axios.get('/api/mappings/config/rating_priority/defaults')
          ]);
          ratingMapping.value = rMapRes.data;
          ratingPriority.value = rPrioRes.data;
          message.success('已加载默认分级预设，请点击保存以生效');
          setupSortables();
        } catch (e) {
          message.error('获取默认预设失败');
        }
      }
    });
    return;
  }

  const typeMap = {
    'keywords': { url: 'keyword_mapping', list: keywordList, type: 'keyword' },
    'studios': { url: 'studio_mapping', list: studioList, type: 'studio' },
    'countries': { url: 'country_mapping', list: countryList, type: 'country' },
    'languages': { url: 'language_mapping', list: languageList, type: 'language' },
    'rating_labels': { url: 'unified_ratings', list: ratingLabelList, type: 'simple' }
  };

  const current = typeMap[activeTab.value];
  if (!current) return;

  dialog.warning({
    title: '恢复默认预设',
    content: `确定要恢复【${activeTab.value}】的默认预设吗？当前未保存的修改将丢失。`,
    positiveText: '确定',
    negativeText: '取消',
    onPositiveClick: async () => {
      try {
        const res = await axios.get(`/api/mappings/config/${current.url}/defaults`);
        current.list.value = processBackendData(res.data, current.type);
        message.success('已加载默认预设，请点击保存以生效');
        setupSortables();
      } catch (e) {
        message.error('获取默认预设失败');
      }
    }
  });
};

const handleSave = async () => {
  if (isSaving.value) return;
  if (!isReady.value || isLoading.value || loadError.value) {
    message.error('请先完整加载映射配置，再保存。');
    return;
  }
  isSaving.value = true;
  try {
    await Promise.all([
      axios.post('/api/mappings/config/keyword_mapping', processFrontendData(keywordList.value, 'keyword')),
      axios.post('/api/mappings/config/studio_mapping', processFrontendData(studioList.value, 'studio')),
      axios.post('/api/mappings/config/country_mapping', processFrontendData(countryList.value, 'country')),
      axios.post('/api/mappings/config/language_mapping', processFrontendData(languageList.value, 'language')),
      axios.post('/api/mappings/config/rating_mapping', ratingMapping.value),
      axios.post('/api/mappings/config/rating_priority', ratingPriority.value),
      axios.post('/api/mappings/config/unified_ratings', processFrontendData(ratingLabelList.value, 'simple'))
    ]);
    message.success('所有映射配置已保存');
    await fetchData();
  } catch (e) {
    console.error(e);
    message.error('保存失败');
  } finally {
    isSaving.value = false;
  }
};

onMounted(() => {
  fetchData();
});

onUnmounted(() => {
  sortables.forEach(s => s?.destroy());
});
</script>

<style scoped>
.mapping-manager { min-inline-size: 0; container-type: inline-size; }
.mapping-tabs { border-block-end: 1px solid rgba(var(--v-border-color), var(--v-border-opacity)); }
.mapping-content { padding-block-start: 20px; }
.sortable-list { display: flex; flex-direction: column; gap: 10px; max-block-size: 60dvh; overflow-y: auto; padding-inline-end: 4px; }
.list-item { display: flex; align-items: center; gap: 12px; padding: 14px 12px; }
.col-handle { inline-size: 24px; flex: 0 0 24px; display: flex; align-items: center; cursor: grab; color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity)); }
.col-handle:active { cursor: grabbing; }
.col-label { flex: 1; min-inline-size: 120px; }
.col-en, .col-ids { flex: 1.2; min-inline-size: 0; }
.col-extra { flex: 1.8; min-inline-size: 0; }
.simple-label { flex: 1; min-inline-size: 0; }
.sortable-ghost { opacity: 0.5; }
.priority-tags { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
.priority-tag { cursor: grab; }
.rating-heading { display: flex; gap: 12px; justify-content: space-between; align-items: center; margin-block-end: 12px; }
.rating-panel-heading { display: flex; align-items: center; padding-inline-end: 12px; }
.rating-panel-heading > .v-expansion-panel-title { flex: 1; inline-size: auto; min-inline-size: 0; }
.rating-rule { display: flex; align-items: center; flex-wrap: wrap; inline-size: 100%; gap: 12px; margin-block-end: 12px; }
.rating-code, .rating-label { flex: 1 1 150px; min-inline-size: 0; }
.rating-value { flex: 0 0 140px; min-inline-size: 0; }
.rating-rule-arrow { color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity)); }
.footer-actions { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 12px; margin-block-start: 24px; padding-block-start: 16px; border-block-start: 1px solid rgba(var(--v-border-color), var(--v-border-opacity)); }
@container (max-width: 760px) {
  .list-item { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 14px; }
  .list-item > .col-handle { grid-column: 1; grid-row: 1; }
  .list-item > .v-btn { grid-column: 2; grid-row: 1; }
  .list-item > .v-input, .list-item > :deep(.app-responsive-input) { grid-column: 1 / -1; min-inline-size: 0; }
  .rating-rule { align-items: start; }
  .rating-code { flex: 1 1 140px; }
  .rating-value { flex-basis: 140px; }
  .rating-label { flex-basis: calc(100% - 52px); }
  .rating-rule-arrow { display: none; }
  .footer-actions > .v-btn { flex: 1 1 160px; }
}
</style>
