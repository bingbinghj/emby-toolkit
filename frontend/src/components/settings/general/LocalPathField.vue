<script setup>
import { ref, watch, onBeforeUnmount, useAttrs } from 'vue';
import axios from 'axios';

defineOptions({ inheritAttrs: false });
const props = defineProps({ modelValue: { type: String, default: '' } });
const emit = defineEmits(['update:modelValue']);
const attrs = useAttrs();
const menuOpen = ref(false), loading = ref(false), error = ref('');
const directories = ref([]), currentPath = ref(''), requestedPath = ref('/');
let requestVersion = 0;

const loadDirectory = async (path) => {
  const version = ++requestVersion;
  requestedPath.value = path || '/';
  currentPath.value = '';
  directories.value = [];
  error.value = '';
  loading.value = true;
  try {
    const { data } = await axios.get('/api/p115/system/directories', { params: { path: requestedPath.value } });
    if (version !== requestVersion || !menuOpen.value) return;
    if (data?.code !== 200 || !Array.isArray(data.data) || typeof data.current_path !== 'string' || !data.current_path.startsWith('/')
      || data.data.some(item => typeof item?.path !== 'string' || !item.path.startsWith('/') || typeof item.name !== 'string')) {
      throw new Error(data?.message || '目录响应无效，请重试');
    }
    currentPath.value = data.current_path;
    directories.value = data.data;
  } catch (failed) {
    if (version === requestVersion && menuOpen.value) error.value = failed.response?.data?.error || failed.response?.data?.message || failed.message || '目录读取失败';
  } finally {
    if (version === requestVersion) loading.value = false;
  }
};

const chooseDirectory = () => {
  if (!menuOpen.value || loading.value || error.value || !currentPath.value) return;
  emit('update:modelValue', currentPath.value);
  menuOpen.value = false;
};

watch(menuOpen, open => {
  if (open) loadDirectory(String(props.modelValue || '').trim() || '/');
  else { requestVersion += 1; loading.value = false; }
});
onBeforeUnmount(() => { requestVersion += 1; });
</script>

<template>
  <VTextField v-bind="attrs" :model-value="modelValue" @update:model-value="emit('update:modelValue', $event)">
    <template #append-inner>
      <VMenu v-model="menuOpen" :close-on-content-click="false" :max-width="480">
        <template #activator="{ props: menuProps }">
          <VBtn v-bind="menuProps" icon="mdi-folder-open-outline" variant="text" size="small" :disabled="attrs.disabled || attrs.readonly" :aria-label="attrs.label ? `选择${attrs.label}` : '选择目录'" @click.stop />
        </template>
        <VCard class="local-path-menu" width="480" max-width="calc(100vw - 24px)">
          <VCardTitle class="text-subtitle-1">选择目录</VCardTitle>
          <VCardText class="pb-0">
            <div class="text-body-2 text-break mb-3">{{ currentPath || requestedPath }}</div>
            <VProgressLinear v-if="loading" indeterminate class="mb-3" />
            <VAlert v-if="error" type="error" variant="tonal" density="compact" class="mb-3">{{ error }}</VAlert>
            <div class="d-flex flex-wrap ga-2 mb-2">
              <VBtn size="small" variant="tonal" prepend-icon="mdi-home-outline" @click="loadDirectory('/')">根目录</VBtn>
              <VBtn v-if="error" size="small" variant="text" :loading="loading" @click="loadDirectory(requestedPath)">重试读取目录</VBtn>
            </div>
            <VList v-if="directories.length" class="local-path-directories" density="compact">
              <VListItem v-for="item in directories" :key="item.path" tag="button" type="button" class="w-100 text-start"
                :title="item.is_parent ? '上一级' : item.name" :prepend-icon="item.is_parent ? 'mdi-arrow-up' : 'mdi-folder-outline'"
                :disabled="loading" :aria-label="item.is_parent ? '返回上一级目录' : `打开目录 ${item.name}`" @click="loadDirectory(item.path)" />
            </VList>
            <p v-else-if="!loading && !error" class="text-body-2 text-medium-emphasis py-3 mb-0">此目录没有子文件夹。</p>
          </VCardText>
          <VCardActions>
            <VSpacer /><VBtn variant="text" @click="menuOpen = false">取消</VBtn>
            <VBtn variant="flat" :disabled="loading || Boolean(error) || !currentPath" @click="chooseDirectory">选择此目录</VBtn>
          </VCardActions>
        </VCard>
      </VMenu>
    </template>
  </VTextField>
</template>

<style scoped>
.local-path-menu { min-width: 0; }
.local-path-directories { max-height: 20rem; overflow-y: auto; }
</style>
