<script setup>
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { useMessage } from '@/composables/useFeedback';
import { MAX_WALLPAPER_LENGTH, normalizeWallpaperImage, preloadWallpaperImage, useWallpaper, wallpaperRotationIntervals } from '@/composables/useWallpaper';

const { settings, presets, sources, activeImage, sourceLoading, sourceError, refreshSource, updateSettings, setCustomImage } = useWallpaper();
const message = useMessage();
const url = ref(settings.customImage.startsWith('http') ? settings.customImage : '');
const apiUrl = ref(settings.customApi);
const busy = ref(false), fileInput = ref(null), previewFailed = ref(false);
const intervals = computed(() => wallpaperRotationIntervals.some(item => item.value === settings.rotationInterval)
  ? wallpaperRotationIntervals : [...wallpaperRotationIntervals, { title: `${settings.rotationInterval} 秒（已保存）`, value: settings.rotationInterval }]);
const preview = computed(() => settings.mode === 'custom' ? settings.customImage : settings.mode === 'preset' ? presets.find(preset => preset.id === settings.presetId)?.image : settings.mode === 'remote' ? activeImage.value : '');
let generation = 0;

function cancelPending() { generation++; busy.value = false; }
function change(patch) {
  cancelPending();
  try { updateSettings(patch); } catch (error) { message.error(error.message); }
}
watch(() => settings.customImage, image => { url.value = image.startsWith('http') ? image : ''; });
watch(() => settings.customApi, value => { apiUrl.value = value; });
watch(() => [settings.mode, settings.presetId, settings.customImage, settings.rotationInterval], cancelPending);
watch(preview, () => { previewFailed.value = false; });
onBeforeUnmount(cancelPending);

async function applyUrl() {
  const revision = ++generation;
  busy.value = false;
  const image = normalizeWallpaperImage(url.value);
  if (!image || !/^https?:/i.test(image)) { message.warning('请输入有效的 HTTP / HTTPS 图片地址。'); return; }
  busy.value = true;
  try {
    await preloadWallpaperImage(image);
    if (revision === generation) setCustomImage(image);
  } catch (error) {
    if (revision === generation) message.error(error.message);
  } finally { if (revision === generation) busy.value = false; }
}

async function upload(event) {
  const file = event.target.files?.[0];
  event.target.value = '';
  if (!file) return;
  const revision = ++generation;
  busy.value = true;
  let objectUrl;
  try {
    if (!['image/jpeg', 'image/png', 'image/webp'].includes(file.type)) throw new Error('请选择 JPEG、PNG 或 WebP 图片。');
    if (file.size > 20 * 1024 * 1024) throw new Error('图片不能超过 20 MB，请先缩小图片。');
    objectUrl = URL.createObjectURL(file);
    const image = await preloadWallpaperImage(objectUrl);
    if (revision !== generation) return;
    const scale = Math.min(1, 1920 / image.naturalWidth, 1080 / image.naturalHeight);
    const canvas = document.createElement('canvas');
    canvas.width = Math.max(1, Math.round(image.naturalWidth * scale));
    canvas.height = Math.max(1, Math.round(image.naturalHeight * scale));
    const context = canvas.getContext('2d');
    if (!context) throw new Error('当前浏览器无法处理图片。');
    context.fillStyle = '#ffffff';
    context.fillRect(0, 0, canvas.width, canvas.height);
    context.drawImage(image, 0, 0, canvas.width, canvas.height);
    let result;
    for (const quality of [0.88, 0.75, 0.6, 0.45, 0.3]) {
      result = canvas.toDataURL('image/jpeg', quality);
      if (result.length <= MAX_WALLPAPER_LENGTH) break;
    }
    if (result.length > MAX_WALLPAPER_LENGTH || !normalizeWallpaperImage(result)) throw new Error('图片压缩后仍然过大，请选择较小的图片。');
    if (revision === generation) setCustomImage(result);
  } catch (error) {
    if (revision === generation) message.error(error.message || '图片处理失败，请选择其他图片。');
  } finally {
    if (objectUrl) URL.revokeObjectURL(objectUrl);
    if (revision === generation) busy.value = false;
  }
}
</script>

<template>
  <section class="wallpaper-settings">
    <h3 class="text-subtitle-1 mb-3">背景壁纸</h3>
    <VBtnToggle :model-value="settings.mode" mandatory divided density="comfortable" color="primary" class="mb-4" @update:model-value="change({ mode: $event })">
      <VBtn value="preset">预设</VBtn><VBtn value="remote">来源</VBtn><VBtn value="custom">自定义</VBtn><VBtn value="none">关闭</VBtn>
    </VBtnToggle>
    <template v-if="settings.mode === 'preset'">
      <div class="wallpaper-presets mb-4">
        <button v-for="preset in presets" :key="preset.id" type="button" class="wallpaper-preset" :class="{ selected: settings.presetId === preset.id }" :aria-pressed="settings.presetId === preset.id" @click="change({ presetId: preset.id })">
          <img :src="preset.image" alt="" /><span>{{ preset.title }}</span>
        </button>
      </div>
    </template>
    <template v-if="settings.mode === 'remote'">
      <VSelect :model-value="settings.source" :items="sources" label="壁纸来源" aria-label="壁纸来源" class="mb-3" hide-details @update:model-value="change({ source: $event })" />
      <template v-if="settings.source === 'api'">
        <VTextField v-model="apiUrl" label="壁纸 API 地址" aria-label="壁纸 API 地址" placeholder="https://example.com/wallpapers" hide-details @keyup.enter="change({ customApi: apiUrl })" />
        <VBtn class="my-3" variant="tonal" :loading="sourceLoading" @click="change({ customApi: apiUrl })">应用 API</VBtn>
        <p class="text-caption text-medium-emphasis mb-3">API 需允许浏览器跨域读取，并返回图片地址数组或 { images: [...] }。</p>
      </template>
      <VAlert v-if="sourceError" type="warning" variant="tonal" density="compact" class="mb-3">{{ sourceError }} 上一次成功背景仍保留。</VAlert>
      <VBtn variant="text" prepend-icon="mdi-refresh" :loading="sourceLoading" class="mb-3" @click="refreshSource">重新读取壁纸</VBtn>
    </template>
    <template v-if="settings.mode === 'preset' || settings.mode === 'remote'">
      <VSelect :model-value="settings.rotationInterval" :items="intervals" label="轮换间隔" aria-label="轮换间隔" hide-details @update:model-value="change({ rotationInterval: $event })" />
      <p class="text-caption text-medium-emphasis mt-2">跟随系统的减少动态效果设置；开启后停止自动轮换。</p>
    </template>
    <template v-if="settings.mode === 'custom'">
      <VTextField :model-value="url" label="图片地址" placeholder="https://example.com/wallpaper.jpg" hide-details @update:model-value="cancelPending(); url = $event" @keyup.enter="applyUrl" />
      <div class="d-flex ga-2 my-3">
        <VBtn :loading="busy" variant="tonal" @click="applyUrl">应用地址</VBtn>
        <VBtn :loading="busy" variant="tonal" prepend-icon="mdi-upload" @click="fileInput.click()">上传图片</VBtn>
      </div>
      <input ref="fileInput" class="wallpaper-file-input" type="file" accept="image/jpeg,image/png,image/webp" aria-label="上传壁纸图片" tabindex="-1" @change="upload" />
      <p class="text-caption text-medium-emphasis">支持 JPEG、PNG、WebP，图片保存在当前浏览器。</p>
    </template>
    <img v-if="preview && !previewFailed" :key="preview" class="wallpaper-preview mt-3" :src="preview" alt="当前壁纸预览" @error="previewFailed = true" />
    <VAlert v-else-if="previewFailed" type="warning" variant="tonal" density="compact" class="mt-3">预览加载失败，请重新选择图片。</VAlert>
  </section>
</template>

<style scoped>
.wallpaper-presets { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; }
.wallpaper-preset { padding: 0; overflow: hidden; border: 2px solid transparent; border-radius: 10px; background: rgba(var(--v-theme-on-surface), .05); color: inherit; cursor: pointer; }
.wallpaper-preset.selected { border-color: rgb(var(--v-theme-primary)); }
.wallpaper-preset:focus-visible { outline: 2px solid rgb(var(--v-theme-primary)); outline-offset: 2px; }
.wallpaper-preset img { display: block; width: 100%; height: 60px; object-fit: cover; }
.wallpaper-preset span { display: block; padding: 6px 2px; font-size: 12px; }
.wallpaper-preview { display: block; width: 100%; height: 130px; border-radius: 12px; object-fit: cover; }
.wallpaper-file-input { position: absolute; width: 1px; height: 1px; opacity: 0; pointer-events: none; }
</style>
