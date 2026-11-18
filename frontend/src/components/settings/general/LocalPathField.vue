<script setup>
import { ref, watch, onBeforeUnmount, useAttrs } from 'vue';
import axios from 'axios';

defineOptions({ inheritAttrs: false });
const props = defineProps({ modelValue: { type: [String, Array], default: '' }, multiple: Boolean });
const emit = defineEmits(['update:modelValue']);
const attrs = useAttrs();
const menuOpen = ref(false), nodes = ref([]), opened = ref([]), failure = ref(null);
const nodeFor = (path, name) => ({ path, name, children: [] });
let generation = 0;
const pending = new Map();

async function readChildren(node) {
  if (node.loaded) return true;
  if (pending.has(node.path)) return pending.get(node.path);
  const version = generation;
  const request = (async () => {
    try {
      const { data } = await axios.get('/api/p115/system/directories', { params: { path: node.path } });
      if (version !== generation || !menuOpen.value) return false;
      if (data?.code !== 200 || !Array.isArray(data.data) || typeof data.current_path !== 'string' || !data.current_path.startsWith('/') ||
          data.data.some(child => typeof child?.path !== 'string' || !child.path.startsWith('/') || typeof child.name !== 'string')) {
        throw new Error(data?.message || '目录响应无效');
      }
      node.children = data.data.filter(child => !child.is_parent).map(child => nodeFor(child.path, child.name));
      node.loaded = true;
      if (failure.value?.node.path === node.path) failure.value = null;
      return true;
    } catch (error) {
      if (version === generation) failure.value = { node, message: error.response?.data?.error || error.response?.data?.message || error.message };
      return false;
    } finally {
      if (version === generation) pending.delete(node.path);
    }
  })();
  pending.set(node.path, request);
  return request;
}

async function choose(node) {
  const version = generation;
  if (!menuOpen.value || !await readChildren(node) || version !== generation || !menuOpen.value) return;
  const path = node.path;
  emit('update:modelValue', props.multiple ? [...new Set([...(props.modelValue || []), path])] : path);
  menuOpen.value = false;
}

watch(menuOpen, async open => {
  generation++;
  pending.clear();
  failure.value = null;
  if (!open) return;
  const value = props.multiple ? props.modelValue?.at(-1) : props.modelValue;
  const segments = String(value || '/').split('/').filter(Boolean);
  nodes.value = [nodeFor('/', '/')];
  opened.value = ['/'];
  let node = nodes.value[0];
  for (const segment of segments) {
    if (!await readChildren(node)) return;
    node = node.children.find(child => child.name === segment);
    if (!node) return;
    opened.value.push(node.path);
  }
  await readChildren(node);
});
onBeforeUnmount(() => { generation++; });
</script>

<template>
  <component :is="multiple ? 'VCombobox' : 'VTextField'" v-bind="attrs" :model-value="modelValue"
    :multiple="multiple" :chips="multiple" :closable-chips="multiple" :return-object="false" :menu-icon="multiple ? '' : undefined"
    @update:model-value="emit('update:modelValue', $event)">
    <template #append-inner>
      <VMenu v-model="menuOpen" :close-on-content-click="false" location="bottom end" max-width="min(30rem, 95vw)">
        <template #activator="{ props: menuProps }">
          <VBtn v-bind="menuProps" icon="mdi-folder-open-outline" variant="text" size="small" :disabled="attrs.disabled || attrs.readonly" :aria-label="attrs.label ? `选择${attrs.label}` : '选择目录'" @click.stop />
        </template>
        <VCard class="local-path-menu">
          <VAlert v-if="failure" type="error" variant="tonal" density="compact">{{ failure.message }}<VBtn variant="text" @click="readChildren(failure.node)">重试读取目录</VBtn></VAlert>
          <VTreeview v-model:opened="opened" :items="nodes" :load-children="readChildren" item-value="path" item-title="name"
            density="compact" max-height="20rem" expand-icon="mdi-folder-outline" collapse-icon="mdi-folder-open-outline">
            <template #title="{ item }"><button type="button" class="directory-name" :aria-label="`选择目录 ${item.path}`" @click.stop="choose(item)">{{ item.name }}</button></template>
          </VTreeview>
        </VCard>
      </VMenu>
    </template>
  </component>
</template>

<style scoped>
.local-path-menu { min-inline-size: min(18rem, 85vw); }
.directory-name { width: 100%; min-height: 36px; text-align: start; white-space: normal; overflow-wrap: anywhere; }
</style>
