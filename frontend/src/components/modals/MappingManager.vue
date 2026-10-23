<!-- src/components/modals/MappingManager.vue -->
<template>
  <div class="mapping-manager">
    <n-tabs type="line" animated v-model:value="activeTab">

      <!-- Tab 1: 关键词 -->
      <n-tab-pane name="keywords"><template #tab><span class="mp-title-with-icon"><UiIcon name="settings" :size="16" />关键词</span></template>
        <n-alert type="info" :bordered="false" class="mb-4">
          将中文标签映射到 TMDb 的英文关键词或 ID。
        </n-alert>
        <div class="list-header">
          <div class="col-handle"></div>
          <div class="col-label">中文标签</div>
          <div class="col-en">英文关键词</div>
          <div class="col-ids">TMDb IDs</div>
          <div class="col-action">操作</div>
        </div>
        <div ref="keywordListRef" class="sortable-list">
          <div v-for="(item, index) in keywordList" :key="item.id" class="list-item">
            <div class="col-handle drag-handle"><n-icon :component="DragIcon" /></div>
            <div class="col-label" data-label="中文名称"><n-input v-model:value="item.label" placeholder="例如：丧尸" /></div>
            <div class="col-en" data-label="英文名称"><n-input v-model:value="item.en" placeholder="zombie" /></div>
            <div class="col-ids" data-label="TMDb IDs"><n-input v-model:value="item.ids" placeholder="12377" /></div>
            <div class="col-action">
              <n-button circle text type="error" aria-label="删除此映射" title="删除此映射" @click="removeItem(keywordList, index)"><n-icon :component="DeleteIcon" /></n-button>
            </div>
          </div>
        </div>
        <n-button dashed block class="mt-4" @click="addItem(keywordList)"><template #icon><UiIcon name="add" /></template>添加关键词</n-button>
      </n-tab-pane>

      <!-- Tab 2: 工作室 -->
      <n-tab-pane name="studios"><template #tab><span class="mp-title-with-icon"><UiIcon name="settings" :size="16" />工作室</span></template>
        <n-alert type="info" :bordered="false" class="mb-4">
          配置出品公司或播出平台。<b>请区分电影制作公司和电视播出平台 ID。</b>
        </n-alert>
        <div class="list-header">
          <div class="col-handle"></div>
          <div class="col-label">中文简称</div>
          <div class="col-en">英文原名</div>
          <!-- ★★★ 修改：拆分为两列 ★★★ -->
          <div class="col-ids">制作公司 IDs (Movie)</div>
          <div class="col-ids">播出平台 IDs (TV)</div>
          <div class="col-action">操作</div>
        </div>
        <div ref="studioListRef" class="sortable-list">
          <div v-for="(item, index) in studioList" :key="item.id" class="list-item">
            <div class="col-handle drag-handle"><n-icon :component="DragIcon" /></div>
            <div class="col-label" data-label="中文名称"><n-input v-model:value="item.label" placeholder="例如：爱奇艺" /></div>
            <div class="col-en" data-label="英文名称"><n-input v-model:value="item.en" placeholder="iQIYI" /></div>

            <!-- ★★★ 修改：两个输入框 ★★★ -->
            <div class="col-ids" data-label="制作公司 IDs（电影）">
              <n-input v-model:value="item.company_ids" placeholder="Company ID" />
            </div>
            <div class="col-ids" data-label="播出平台 IDs（电视）">
              <n-input v-model:value="item.network_ids" placeholder="Network ID" />
            </div>

            <div class="col-action">
              <n-button circle text type="error" aria-label="删除此映射" title="删除此映射" @click="removeItem(studioList, index)"><n-icon :component="DeleteIcon" /></n-button>
            </div>
          </div>
        </div>
        <n-button dashed block class="mt-4" @click="addItem(studioList)"><template #icon><UiIcon name="add" /></template>添加工作室</n-button>
      </n-tab-pane>

      <!-- Tab 3: 国家/地区 -->
      <n-tab-pane name="countries"><template #tab><span class="mp-title-with-icon"><UiIcon name="settings" :size="16" />国家/地区</span></template>
        <n-alert type="info" :bordered="false" class="mb-4">
          配置国家/地区代码映射。<b>ISO代码</b>用于API筛选，<b>别名</b>用于元数据翻译匹配。
        </n-alert>
        <div class="list-header">
          <div class="col-handle"></div>
          <div class="col-label">中文名称</div>
          <div class="col-en">ISO 代码 (如 US)</div>
          <div class="col-extra">别名 (逗号分隔)</div>
          <div class="col-action">操作</div>
        </div>
        <div ref="countryListRef" class="sortable-list">
          <div v-for="(item, index) in countryList" :key="item.id" class="list-item">
            <div class="col-handle drag-handle"><n-icon :component="DragIcon" /></div>
            <div class="col-label" data-label="中文名称"><n-input v-model:value="item.label" placeholder="例如：美国" /></div>
            <div class="col-en" data-label="ISO 代码"><n-input v-model:value="item.value" placeholder="US" /></div>
            <div class="col-extra" data-label="别名（逗号分隔）"><n-input v-model:value="item.aliases" placeholder="USA, United States" /></div>
            <div class="col-action">
              <n-button circle text type="error" aria-label="删除此映射" title="删除此映射" @click="removeItem(countryList, index)"><n-icon :component="DeleteIcon" /></n-button>
            </div>
          </div>
        </div>
        <n-button dashed block class="mt-4" @click="addItem(countryList, 'country')"><template #icon><UiIcon name="add" /></template>添加国家/地区</n-button>
      </n-tab-pane>

      <!-- Tab 4: 原语言 -->
      <n-tab-pane name="languages"><template #tab><span class="mp-title-with-icon"><UiIcon name="settings" :size="16" />原语言</span></template>
        <n-alert type="info" :bordered="false" class="mb-4">
          配置语言代码映射。用于筛选器的语言选项。<b>别名</b>可用于兼容TMDb的历史遗留代码（如 cn 和 yue），以及压制组各种不规范的参数用于格式化媒体信息，注意别名第一个必须用三位语言代码。
        </n-alert>
        <div class="list-header">
          <div class="col-handle"></div>
          <div class="col-label">中文名称</div>
          <div class="col-en">ISO 代码 (如 en)</div>
          <!-- ★★★ 核心修改：把 col-empty 换成别名输入框 ★★★ -->
          <div class="col-extra">别名 (逗号分隔)</div>
          <div class="col-action">操作</div>
        </div>
        <div ref="languageListRef" class="sortable-list">
          <div v-for="(item, index) in languageList" :key="item.id" class="list-item">
            <div class="col-handle drag-handle"><n-icon :component="DragIcon" /></div>
            <div class="col-label" data-label="中文名称"><n-input v-model:value="item.label" placeholder="例如：粤语" /></div>
            <div class="col-en" data-label="ISO 代码"><n-input v-model:value="item.value" placeholder="cn" /></div>
            <!-- ★★★ 核心修改：绑定 item.aliases ★★★ -->
            <div class="col-extra" data-label="别名（逗号分隔）"><n-input v-model:value="item.aliases" placeholder="yue, cmn" /></div>
            <div class="col-action">
              <n-button circle text type="error" aria-label="删除此映射" title="删除此映射" @click="removeItem(languageList, index)"><n-icon :component="DeleteIcon" /></n-button>
            </div>
          </div>
        </div>
        <n-button dashed block class="mt-4" @click="addItem(languageList, 'language')"><template #icon><UiIcon name="add" /></template>添加语言</n-button>
      </n-tab-pane>

      <!-- Tab 5: 分级标签 -->
      <n-tab-pane name="rating_labels"><template #tab><span class="mp-title-with-icon"><UiIcon name="settings" :size="16" />分级标签</span></template>
        <n-alert type="info" :bordered="false" class="mb-4">
          定义系统中可用的<b>中文分级名称</b>（如“全年龄”、“限制级”）。<br/>
          这些标签将用于下方的“分级制度”映射以及分级筛选器。
        </n-alert>
        <div class="list-header">
          <div class="col-handle"></div>
          <div class="col-label" style="flex: 1">中文分级名称</div>
          <div class="col-action">操作</div>
        </div>
        <div ref="ratingLabelListRef" class="sortable-list">
          <div v-for="(item, index) in ratingLabelList" :key="item.id" class="list-item">
            <div class="col-handle drag-handle"><n-icon :component="DragIcon" /></div>
            <div class="col-label" data-label="中文分级名称">
              <n-input v-model:value="item.label" placeholder="例如：儿童专区" />
            </div>
            <div class="col-action">
              <n-button circle text type="error" aria-label="删除此映射" title="删除此映射" @click="removeItem(ratingLabelList, index)">
                <n-icon :component="DeleteIcon" />
              </n-button>
            </div>
          </div>
        </div>
        <n-button dashed block class="mt-4" @click="addItem(ratingLabelList, 'simple')"><template #icon><UiIcon name="add" /></template>添加分级标签</n-button>
      </n-tab-pane>

      <!-- Tab 6: 分级制度 (映射) -->
      <n-tab-pane name="ratings"><template #tab><span class="mp-title-with-icon"><UiIcon name="settings" :size="16" />分级制度</span></template>
        <n-alert type="info" :bordered="false" class="mb-4">
          TMDb 返回各国分级数据。在此定义<b>优先级</b>和<b>中文映射</b>。<br/>
          <b>Emby等级值</b>：用于封面生成时的权限控制 (G=1, PG=5, PG-13=8, R=9, NC-17=10, XXX=15)。
        </n-alert>

        <!-- 1. 优先级策略 -->
        <n-card size="small" title="优先级策略" class="mb-4" embedded>
          <template #header-extra>
            <n-text depth="3" style="font-size: 12px">拖拽调整查找顺序</n-text>
          </template>
          <div ref="ratingPriorityRef" class="priority-tags">
            <n-tag
              v-for="(country, index) in ratingPriority"
              :key="country"
              closable
              @close="removePriority(index)"
              class="priority-tag drag-handle"
              :type="country === 'ORIGIN' ? 'success' : 'default'"
            >
              {{ country === 'ORIGIN' ? '原产国 (自动)' : getCountryName(country) }}
            </n-tag>

            <!-- 添加优先级的下拉框 -->
            <n-popselect
              v-model:value="newPriorityCountry"
              :options="availablePriorityOptions"
              trigger="click"
              @update:value="addPriority"
              scrollable
            >
              <n-button dashed size="small" type="primary">
                <template #icon><n-icon :component="AddIcon" /></template>
                添加国家
              </n-button>
            </n-popselect>
          </div>
        </n-card>

        <!-- 2. 分级映射表 -->
        <div class="list-header rating-list-header">
          <div class="col-label" style="flex: 1">国家/地区分级表 (跟随优先级排序)</div>
          <div class="col-action">
            <n-button size="tiny" dashed @click="addRatingCountry"><template #icon><UiIcon name="add" /></template>添加国家</n-button>
          </div>
        </div>

        <div class="rating-container">
          <n-collapse display-directive="show" :default-expanded-names="['US']">

            <n-collapse-item
              v-for="countryCode in sortedMappingKeys"
              :key="countryCode"
              :title="getCountryName(countryCode)"
              :name="countryCode"
            >
              <template #header-extra>
                <n-button size="tiny" type="error" text @click.stop="removeRatingCountry(countryCode)"><template #icon><UiIcon name="delete" /></template>删除整组</n-button>
              </template>

              <!-- 具体分级规则表格 -->
              <!-- ★★★ 修改：初始化时增加 emby_value 字段 ★★★ -->
              <n-dynamic-input
                v-model:value="ratingMapping[countryCode]"
                :on-create="() => ({ code: '', label: '全年龄', emby_value: null })"
              >
                <template #default="{ value }">
                  <div class="rating-rule">
                    <!-- 1. 原始分级代码 -->
                    <n-input v-model:value="value.code" placeholder="原始分级 (如 R)" :input-props="{ 'aria-label': '原始分级代码' }" class="rating-code" />

                    <!-- 2. Emby 等级值 (新增) -->
                    <n-tooltip trigger="focus">
                      <template #trigger>
                        <n-input-number
                          v-model:value="value.emby_value"
                          placeholder="等级值"
                          :min="0"
                          :max="100"
                          class="rating-value" :input-props="{ 'aria-label': 'Emby 等级值' }"
                          :show-button="false"
                        >
                          <template #suffix>级</template>
                        </n-input-number>
                      </template>
                      Emby 内部限制等级 (MaxParentalRating)<br/>
                      参考值：G=1, PG=5, PG-13=8, R=9, NC-17=10, XXX=15
                    </n-tooltip>

                    <div class="rating-rule-arrow" aria-hidden="true">➜</div>

                    <!-- 3. 中文标签 -->
                    <n-select
                      v-model:value="value.label"
                      :options="dynamicRatingOptions"
                      placeholder="选择中文标签"
                      class="rating-label" aria-label="中文分级标签"
                      filterable
                      tag
                    />
                  </div>
                </template>
              </n-dynamic-input>
            </n-collapse-item>

            <!-- 空状态提示 -->
            <div v-if="sortedMappingKeys.length === 0" class="rating-empty">
              请在上方添加国家以配置分级映射
            </div>

          </n-collapse>
        </div>
      </n-tab-pane>

    </n-tabs>

    <div class="footer-actions">
      <n-button ghost type="warning" @click="handleRestoreDefaults">
        <template #icon><n-icon :component="RefreshIcon" /></template>
        恢复当前页默认
      </n-button>
      <n-button type="primary" :loading="isSaving" @click="handleSave"><template #icon><UiIcon name="save" /></template>
        保存所有配置
      </n-button>
    </div>
  </div>
</template>

<script setup>
import { ref, onMounted, nextTick, watch, onUnmounted, computed, h } from 'vue';
import axios from 'axios';
import Sortable from 'sortablejs';
import { useMessage, useDialog, NSelect } from 'naive-ui';
import {
  AddOutline as AddIcon,
  TrashOutline as DeleteIcon,
  ReorderFourOutline as DragIcon,
  RefreshOutline as RefreshIcon
} from '@vicons/ionicons5';

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
const ratingPriorityRef = ref(null);

const isSaving = ref(false);

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

    keywordList.value = processBackendData(kwRes.data, 'keyword');
    studioList.value = processBackendData(stRes.data, 'studio');
    countryList.value = processBackendData(cnRes.data, 'country');
    languageList.value = processBackendData(lgRes.data, 'language');
    ratingLabelList.value = processBackendData(rLabelRes.data, 'simple');

    ratingMapping.value = rMapRes.data || {};
    ratingPriority.value = rPrioRes.data || [];

    setupSortables();
  } catch (e) {
    console.error(e);
    message.error('加载配置失败');
  }
};

const addItem = (list, type = 'normal') => {
  const item = { id: generateId(), label: '' };
  if (type === 'country' || type === 'language') {
    item.value = ''; item.aliases = '';
  } else if (type === 'simple') {
    // 仅需要 label
  } else if (list === studioList) {
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
  dialog.create({
    title: '添加分级国家',
    content: () => h(NSelect, {
      options: availablePriorityOptions.value, // 复用优先级的可选列表
      filterable: true,
      placeholder: '搜索国家...',
      onUpdateValue: (v) => {
        addPriority(v); // 直接调用上面的联动添加函数
        dialog.destroyAll();
      }
    })
  });
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
.mapping-manager { padding:0; min-width:0; container-type:inline-size; }
.mb-4 { margin-bottom: 16px; }
.mt-4 { margin-top: 16px; }

.list-header {
  display: flex;
  gap: 12px;
  padding: 8px 12px;
  background:var(--ui-surface-muted);
  font-weight: bold;
  color:var(--ui-text-muted);
  border-bottom:1px solid var(--ui-border);
}

.list-item {
  display: flex;
  gap: 12px;
  padding: 8px 12px;
  align-items: center;
  background:var(--ui-surface);
  border-bottom:1px solid var(--ui-border);
  transition: background 0.2s;
}
.list-item:hover { background:var(--ui-surface-muted); }

.sortable-ghost { opacity:0.5; background:var(--accent-glow-color); }

.sortable-list {
  max-height:60dvh;
  overflow-y: auto;
  padding-right:4px;
  display: flex;
  flex-direction: column;
}
.sortable-list::-webkit-scrollbar { width: 6px; }
.sortable-list::-webkit-scrollbar-thumb { background-color:var(--ui-border-strong); border-radius:3px; }

/* 列宽定义 */
.col-handle { width:24px; flex-shrink:0; display:flex; align-items:center; cursor:grab; color:var(--ui-text-muted); }
.col-handle:active { cursor: grabbing; }
.col-label { width:120px; min-width:0; }
.col-en { flex:1.5; min-width:0; }
.col-ids { flex:1; min-width:0; }
.col-extra { flex:2; min-width:0; } /* 用于国家别名 */
.col-empty { flex: 2; } /* 用于语言占位 */
.col-action { width:40px; flex-shrink:0; display:flex; justify-content:center; }
.rating-list-header .col-action { width:auto; }

.footer-actions {
  margin-top: 24px;
  display: flex;
  flex-wrap:wrap;
  gap:12px;
  justify-content: space-between;
  padding-top: 16px;
  border-top:1px solid var(--ui-border);
}

/* 优先级标签样式 */
.priority-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  padding: 8px;
  background:var(--ui-surface-muted);
  border-radius: 4px;
}
.priority-tag { cursor: move; }
.rating-container {
  margin-top: 10px;
  border:1px solid var(--ui-border);
  border-radius: 4px;
  overflow: hidden;
}
.rating-rule { display:flex; align-items:center; width:100%; min-width:0; gap:10px; }
.rating-code,.rating-label { flex:1.5; min-width:0; }
.rating-value { width:110px; }
.rating-rule-arrow { flex:0 0 20px; text-align:center; color:var(--ui-text-muted); }
.rating-empty { padding:20px; text-align:center; color:var(--ui-text-muted); }
@container (max-width:660px) {
  .list-header:not(.rating-list-header) { display:none; }
  .sortable-list { gap:12px; }
  .list-item { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:12px; padding:12px; border:1px solid var(--ui-border); border-radius:10px; }
  .list-item > .col-handle { grid-column:1; grid-row:1; width:auto; min-height:28px; }
  .list-item > .col-action { grid-column:2; grid-row:1; width:auto; justify-content:flex-end; }
  .list-item > [data-label] { width:auto; }
  .list-item > [data-label]::before { content:attr(data-label); display:block; margin-bottom:6px; color:var(--ui-text-muted); font-size:12px; }
  .list-item > .col-extra { grid-column:1 / -1; }
  .rating-rule { flex-wrap:wrap; gap:8px; }
  .rating-code { flex:1 1 120px; }
  .rating-value { width:90px; }
  .rating-label { flex-basis:100%; }
  .rating-rule-arrow { display:none; }
  :deep(.n-dynamic-input-item__action) { margin-left:8px; }
  .footer-actions > .n-button { flex:1 1 160px; }
}
@container (max-width:360px) { .list-item { grid-template-columns:minmax(0,1fr) auto; } .list-item > [data-label] { grid-column:1 / -1; } }
</style>
