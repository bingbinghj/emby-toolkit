<template>
  <div class="mp-page media-organize-page">
    <header class="d-flex flex-wrap align-center justify-space-between ga-3 mb-6">
      <div>
        <h1 class="text-h4 mb-1">媒体整理</h1>
        <p class="text-body-2 text-medium-emphasis mb-0">选择在库媒体，重新执行 ETK 媒体信息与元数据处理流程。</p>
      </div>
      <VBtn color="primary" prepend-icon="mdi-refresh" :disabled="!selectedItems.length || processing" :loading="processing" data-testid="organize-selected" @click="openConfirmation">重新整理 {{ selectedItems.length || '' }}</VBtn>
    </header>

    <VAlert v-if="librariesError" type="error" variant="tonal" class="mb-4">
      {{ librariesError }}<VBtn variant="text" :loading="loadingLibraries" @click="loadLibraries">重新读取媒体库</VBtn>
    </VAlert>
    <VCard class="mb-4">
      <VCardText>
        <VRow align="center">
          <VCol cols="12" md="4"><VSelect v-model="libraryId" :items="libraries" item-title="name" item-value="id" label="媒体库" :loading="loadingLibraries" :disabled="loadingLibraries || processing" :hide-details="true" data-testid="organize-library" /></VCol>
          <VCol cols="12" md="5"><VTextField v-model="search" label="搜索媒体" placeholder="按名称查找" prepend-inner-icon="mdi-magnify" clearable :hide-details="true" :disabled="!libraryId || processing" @keyup.enter="searchItems" /></VCol>
          <VCol cols="12" md="3"><VBtn block variant="tonal" prepend-icon="mdi-magnify" :loading="loadingItems" :disabled="!libraryId || processing" @click="searchItems">搜索</VBtn></VCol>
        </VRow>
      </VCardText>
    </VCard>

    <VAlert v-if="selectedItems.length" type="info" variant="tonal" class="mb-4" data-testid="organize-selection">
      <div class="d-flex flex-wrap align-center ga-3"><span>已选择 {{ selectedItems.length }} 项，跨分页和媒体库保留选择。</span><VBtn size="small" variant="text" :disabled="processing" @click="selectedItems = []">清空选择</VBtn></div>
      <div class="d-flex flex-wrap ga-2 mt-2"><VChip v-for="item in selectedItems" :key="item.Id" size="small" closable :disabled="processing" @click:close="removeSelection(item.Id)">{{ item.LibraryName }} · {{ item.Name }}</VChip></div>
    </VAlert>

    <VAlert v-if="itemsError" type="error" variant="tonal" class="mb-4" data-testid="organize-load-error">{{ itemsError }}<VBtn variant="text" :loading="loadingItems" @click="loadItems">重新读取</VBtn></VAlert>
    <VAlert v-if="feedback" :type="feedback.type" :title="feedback.message" variant="tonal" class="mb-4" data-testid="organize-result">
      <div v-for="(detail, index) in feedback.details" :key="index" class="text-wrap">{{ detail }}</div>
    </VAlert>

    <VCard>
      <VCardItem :title="loadedLibrary?.name || '在库媒体'" subtitle="电影包括关联版本；电视剧按 ETK 现有在库季集范围处理。" />
      <VDataTable :headers="headers" :items="items" item-value="Id" show-select :model-value="selectedIds" :items-per-page="pageSize" :loading="loadingItems" :item-selectable="canSelectItems" hide-default-footer data-testid="organize-items" @update:model-value="updateSelection">
        <template #item.Name="{ item }">
          <div class="d-flex align-center ga-3 py-2"><VAvatar rounded size="42"><VImg v-if="poster(item)" :src="poster(item)" :alt="item.Name" cover /><VIcon v-else icon="mdi-movie-open-outline" /></VAvatar><VBtn variant="text" class="media-title px-0" :to="`/edit-media/${encodeURIComponent(item.Id)}`">{{ item.Name }}</VBtn></div>
        </template>
        <template #item.Type="{ item }"><VChip size="small" variant="tonal" :color="item.Type === 'Movie' ? 'primary' : 'success'">{{ item.Type === 'Movie' ? '电影' : '电视剧' }}</VChip></template>
        <template #no-data><div class="py-8 text-medium-emphasis">{{ libraryId ? '当前条件下没有媒体' : '请选择媒体库' }}</div></template>
      </VDataTable>
      <VCardActions class="d-flex flex-wrap justify-space-between"><span class="text-body-2 text-medium-emphasis">{{ total === null ? '总数未确认' : `共 ${total} 项` }}</span><VPagination v-if="pages > 1" v-model="page" :length="pages" :total-visible="5" :disabled="loadingItems || processing" @update:model-value="loadItems" /></VCardActions>
    </VCard>

    <VDialog v-model="confirmationOpen" max-width="640" :persistent="processing">
      <VCard>
        <VCardTitle>确认重新整理 {{ confirmationItems.length }} 项</VCardTitle>
        <VCardText>
          <p>将重新执行媒体信息重建、回读验收和元数据增强。电影包含关联版本，电视剧包含当前在库季集。</p>
          <VList bg-color="transparent" density="compact"><VListItem v-for="item in confirmationItems" :key="item.Id" :title="item.Name" :subtitle="`${item.LibraryName} · ${item.Type === 'Movie' ? '电影' : '电视剧'} · ${item.Id}`" /></VList>
        </VCardText>
        <VCardActions><VSpacer /><VBtn variant="text" :disabled="processing" @click="confirmationOpen = false">取消</VBtn><VBtn color="warning" :loading="processing" data-testid="organize-confirm" @click="reorganize">开始重新整理</VBtn></VCardActions>
      </VCard>
    </VDialog>
  </div>
</template>

<script setup>
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import axios from 'axios';
import { useMessage } from '@/composables/useFeedback';

const message = useMessage();
const libraries = ref([]), libraryId = ref(''), loadedLibrary = ref(null);
const loadingLibraries = ref(false), librariesError = ref('');
const items = ref([]), search = ref(''), appliedSearch = ref(''), page = ref(1), total = ref(null);
const pageSize = 24;
const selectedItems = ref([]), confirmationItems = ref([]), confirmationOpen = ref(false);
const loadingItems = ref(false), itemsError = ref(''), processing = ref(false), feedback = ref(null);
let active = true, librarySequence = 0, itemSequence = 0;
const selectedIds = computed(() => selectedItems.value.map(item => item.Id));
const pages = computed(() => Math.max(1, Math.ceil((total.value || 0) / pageSize)));
const canSelectItems = () => !processing.value && !loadingItems.value && !itemsError.value && loadedLibrary.value?.id === libraryId.value;
const headers = [{ title: '媒体', key: 'Name', sortable: false }, { title: '类型', key: 'Type', sortable: false }, { title: '年份', key: 'ProductionYear', sortable: false }];
const poster = item => item.ImageTags?.Primary ? `/image_proxy/Items/${encodeURIComponent(item.Id)}/Images/Primary?tag=${encodeURIComponent(item.ImageTags.Primary)}&quality=90` : '';

const loadLibraries = async () => {
  const sequence = ++librarySequence;
  loadingLibraries.value = true;
  try {
    const { data } = await axios.get('/api/media_center/libraries');
    if (!active || sequence !== librarySequence) return;
    if (!Array.isArray(data?.libraries)) throw new Error('媒体库响应无效');
    libraries.value = data.libraries.filter(item => item.id && !['music', 'playlists', 'boxsets'].includes(String(item.collection_type || '').toLowerCase()));
    librariesError.value = '';
    if (!libraryId.value && libraries.value.length) libraryId.value = libraries.value[0].id;
  } catch (error) {
    if (active && sequence === librarySequence) librariesError.value = error.response?.data?.error || error.message || '读取媒体库失败';
  } finally {
    if (active && sequence === librarySequence) loadingLibraries.value = false;
  }
};

const loadItems = async () => {
  if (!libraryId.value) return;
  const sequence = ++itemSequence, target = libraryId.value;
  loadingItems.value = true;
  try {
    const { data } = await axios.get(`/api/media_center/libraries/${encodeURIComponent(target)}/items`, {
      params: { page: page.value, page_size: pageSize, start_index: (page.value - 1) * pageSize, search: appliedSearch.value },
    });
    if (!active || sequence !== itemSequence || target !== libraryId.value) return;
    if (!Array.isArray(data?.items) || typeof data?.total !== 'number' || data.total < 0) throw new Error('媒体列表或分页总数无效');
    const library = libraries.value.find(item => item.id === target);
    loadedLibrary.value = { id: target, name: data.library?.name || library?.name || target };
    items.value = data.items.filter(item => item.Id && ['Movie', 'Series'].includes(item.Type));
    total.value = data.total;
    itemsError.value = '';
  } catch (error) {
    if (active && sequence === itemSequence) itemsError.value = error.response?.data?.error || error.message || '读取媒体失败，已保留此前选择';
  } finally {
    if (active && sequence === itemSequence) loadingItems.value = false;
  }
};

const searchItems = () => { appliedSearch.value = search.value?.trim() || ''; page.value = 1; loadItems(); };
const removeSelection = id => { if (!processing.value) selectedItems.value = selectedItems.value.filter(item => item.Id !== id); };
const updateSelection = ids => {
  if (!canSelectItems()) return;
  const current = new Set(items.value.map(item => item.Id)), next = new Set(ids);
  selectedItems.value = selectedItems.value.filter(item => !current.has(item.Id) || next.has(item.Id));
  for (const item of items.value) {
    if (next.has(item.Id) && !selectedItems.value.some(selected => selected.Id === item.Id)) selectedItems.value.push({ ...item, LibraryName: loadedLibrary.value.name });
  }
};
const openConfirmation = () => {
  if (processing.value || !selectedItems.value.length) return;
  confirmationItems.value = selectedItems.value.map(item => ({ ...item }));
  confirmationOpen.value = true;
};
const reorganize = async () => {
  if (processing.value || !confirmationItems.value.length) return;
  const ids = confirmationItems.value.map(item => item.Id);
  processing.value = true;
  try {
    const response = await axios.post('/api/actions/reorganize', { item_ids: ids });
    if (!active) return;
    const result = response.data || {};
    const failures = Array.isArray(result.failures) ? result.failures : [];
    const warnings = Array.isArray(result.warning_details) ? result.warning_details : [];
    const details = [...failures, ...warnings].map(detail => typeof detail === 'string' ? detail : JSON.stringify(detail));
    const failed = failures.length > 0 || Number(result.failed) > 0 || result.success === false || Boolean(result.error);
    const verified = result.selected_count === ids.length && Array.isArray(result.items)
      && result.items.length === ids.length && new Set(result.items.map(item => item.item_id)).size === ids.length
      && result.items.every(item => ids.includes(item.item_id) && ['completed', 'failed'].includes(item.status));
    if (result.error) details.unshift(result.error);
    if (!verified) details.push('尚未取得完整任务结果，请在通知中心检查处理状态。');
    const type = failed ? 'error' : response.status === 207 || Number(result.warnings) > 0 || details.length ? 'warning' : 'success';
    feedback.value = { type, message: verified || failed ? result.message || result.error || '媒体重新整理完成' : '媒体整理结果尚未确认', details };
    message[type](feedback.value.message);
    confirmationOpen.value = false;
  } catch (error) {
    if (!active) return;
    const result = error.response?.data || {};
    feedback.value = { type: 'error', message: result.error || result.message || error.message || '重新整理失败', details: [...(Array.isArray(result.failures) ? result.failures : []), ...(Array.isArray(result.warning_details) ? result.warning_details : [])] };
    message.error(feedback.value.message);
    confirmationOpen.value = false;
  } finally {
    processing.value = false;
  }
};

watch(libraryId, () => { page.value = 1; search.value = ''; appliedSearch.value = ''; loadItems(); });
onMounted(loadLibraries);
onBeforeUnmount(() => { active = false; ++librarySequence; ++itemSequence; });
</script>

<style scoped>
.media-organize-page { width: 100%; min-width: 0; }
.media-title { max-width: 100%; height: auto; min-height: 36px; white-space: normal; text-align: start; }
.media-title :deep(.v-btn__content) { white-space: normal; }
</style>
