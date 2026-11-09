<script setup>
import { ref, watch, onUnmounted } from 'vue';
import QRCode from 'qrcode';

const props = defineProps({ value: { type: String, default: '' }, size: { type: Number, default: 200 } });
const source = ref('');
const failed = ref(false);
let generation = 0;
watch(() => [props.value, props.size], async () => {
  const request = ++generation;
  source.value = '';
  failed.value = false;
  if (!props.value) return;
  try {
    const image = await QRCode.toDataURL(props.value, { width: props.size, margin: 2, errorCorrectionLevel: 'M' });
    if (request === generation) source.value = image;
  } catch {
    if (request === generation) failed.value = true;
  }
}, { immediate: true });
onUnmounted(() => { generation += 1; });
</script>

<template>
  <VAlert v-if="failed" type="error" variant="tonal">二维码生成失败，请重新获取。</VAlert>
  <VImg v-else-if="source" :src="source" :width="size" :height="size" alt="115 扫码授权二维码" class="settings-qr-code" />
  <VProgressCircular v-else indeterminate color="primary" />
</template>

<style scoped>
.settings-qr-code { margin-inline: auto; background: #fff; border-radius: 10px; }
</style>
