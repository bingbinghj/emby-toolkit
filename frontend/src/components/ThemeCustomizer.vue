<script setup>
/*
 * Adapted from MoviePilot-Frontend v3 ThemeCustomizer.vue.
 * MIT License — Copyright (c) 2023 jxxghp
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { NAlert, NButton, NIcon, NInput, NInputGroup, NSelect, NSlider, NSwitch, NText } from 'naive-ui';
import { SunnyOutline, MoonOutline, DesktopOutline } from '@vicons/ionicons5';
import UiIcon from './UiIcon.vue';
import { appearanceDefaults, MAX_WALLPAPER_LENGTH, normalizeWallpaper, primaryColors, useAppearance } from '../composables/useAppearance';

const emit = defineEmits(['close']);
const { settings, updateSettings, resetSettings, setWallpaper } = useAppearance();
const customColorInput = ref(null);
const wallpaperFileInput = ref(null);
const errorMessage = ref('');
const wallpaperBusy = ref(false);
const wallpaperUrl = ref(settings.wallpaper.startsWith('http') ? settings.wallpaper : '');
const customized = computed(() => Object.keys(appearanceDefaults).some(key => settings[key] !== appearanceDefaults[key]));
const themeOptions = [{ label: '浅色', value: 'light', icon: SunnyOutline }, { label: '深色', value: 'dark', icon: MoonOutline }, { label: '系统', value: 'system', icon: DesktopOutline }];
const radiusOptions = [{ label: '紧凑', value: 'none', size: '8px' }, { label: '小', value: 'small', size: '12px' }, { label: '默认', value: 'default', size: '20px' }, { label: '大', value: 'large', size: '24px' }, { label: '超大', value: 'extra', size: '28px' }];
const materialOptions = [{ label: '清透', value: 'clear' }, { label: '色调', value: 'tinted' }, { label: '磨砂', value: 'frosted' }];
const styleOptions = [{ label: '自适应 · 增强阅读保护', value: 'adaptive' }, { label: '清透 · 保留背景细节', value: 'clear' }];
const wallpaperOptions = [{ label: '默认壁纸', value: 'default' }, { label: '自定义壁纸', value: 'custom' }, { label: '无壁纸', value: 'none' }];
let wallpaperRequest = 0;

watch(() => settings.wallpaper, value => { wallpaperUrl.value = value.startsWith('http') ? value : ''; });
onBeforeUnmount(() => { wallpaperRequest++; });

function save(action) {
  errorMessage.value = '';
  try { action(); } catch (error) { errorMessage.value = error.message || '保存外观设置失败，请重试。'; }
}

function savePatch(patch) { save(() => updateSettings(patch)); }

function cancelWallpaperRequest() {
  wallpaperRequest++;
  wallpaperBusy.value = false;
}

function changeWallpaperMode(value) {
  cancelWallpaperRequest();
  savePatch({ wallpaperMode: value });
}

function handleReset() {
  cancelWallpaperRequest();
  save(resetSettings);
}

function changeWallpaperUrl(value) {
  cancelWallpaperRequest();
  wallpaperUrl.value = value;
}

function loadImage(source) {
  return new Promise((resolve, reject) => {
    const image = new Image();
    const timer = window.setTimeout(() => finish(new Error('图片加载超时，请检查地址后重试。')), 15000);
    function finish(error) {
      window.clearTimeout(timer);
      image.onload = image.onerror = null;
      if (error) reject(error); else resolve(image);
    }
    image.onload = () => finish(image.naturalWidth && image.naturalHeight ? null : new Error('图片尺寸无效。'));
    image.onerror = () => finish(new Error('图片加载失败，请检查地址或选择其他图片。'));
    image.src = source;
  });
}

async function applyWallpaperUrl() {
  const request = ++wallpaperRequest;
  errorMessage.value = '';
  wallpaperBusy.value = false;
  const url = normalizeWallpaper(wallpaperUrl.value);
  if (!url || !/^https?:/i.test(url)) {
    errorMessage.value = '请输入有效的 HTTP / HTTPS 图片地址。';
    return;
  }
  wallpaperBusy.value = true;
  try {
    await loadImage(url);
    if (request === wallpaperRequest) setWallpaper(url);
  } catch (error) {
    if (request === wallpaperRequest) errorMessage.value = error.message;
  } finally {
    if (request === wallpaperRequest) wallpaperBusy.value = false;
  }
}

async function uploadWallpaper(event) {
  const file = event.target.files?.[0];
  event.target.value = '';
  if (!file) return;
  const request = ++wallpaperRequest;
  errorMessage.value = '';
  wallpaperBusy.value = true;
  let objectUrl;
  try {
    if (!['image/jpeg', 'image/png', 'image/webp'].includes(file.type)) throw new Error('请选择 JPEG、PNG 或 WebP 图片。');
    if (file.size > 20 * 1024 * 1024) throw new Error('图片不能超过 20 MB，请先缩小图片。');
    objectUrl = URL.createObjectURL(file);
    const image = await loadImage(objectUrl);
    if (request !== wallpaperRequest) return;
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
    if (!result.startsWith('data:image/jpeg;base64,') || result.length > MAX_WALLPAPER_LENGTH) throw new Error('图片压缩后仍然过大，请选择较小的图片。');
    if (request === wallpaperRequest) setWallpaper(result);
  } catch (error) {
    if (request === wallpaperRequest) errorMessage.value = error.message || '图片处理失败，请选择其他图片。';
  } finally {
    if (objectUrl) URL.revokeObjectURL(objectUrl);
    if (request === wallpaperRequest) wallpaperBusy.value = false;
  }
}
</script>

<template>
  <section class="theme-customizer-panel" aria-label="外观设置">
    <header class="theme-customizer-header">
      <div><h2>外观设置</h2><n-text depth="3">更改即时生效，保存在当前浏览器</n-text></div>
      <div class="theme-customizer-actions">
        <n-button quaternary circle :disabled="!customized" aria-label="重置外观设置" title="重置外观设置" @click="handleReset"><UiIcon name="refresh" /></n-button>
        <n-button quaternary circle aria-label="关闭外观设置" @click="emit('close')"><UiIcon name="close" /></n-button>
      </div>
    </header>
    <n-alert v-if="errorMessage" class="theme-customizer-error" type="error" closable @close="errorMessage = ''">{{ errorMessage }}</n-alert>
    <div class="theme-customizer-body">
      <h3>主色</h3>
      <div class="theme-customizer-color-grid">
        <button v-for="color in primaryColors" :key="color.value" type="button" class="theme-customizer-color-option" :class="{ 'is-active': settings.primaryColor === color.value }" :aria-label="`使用${color.name}主色`" :aria-pressed="settings.primaryColor === color.value" :title="color.name" @click="savePatch({ primaryColor: color.value })"><span :style="{ backgroundColor: color.value }" /></button>
        <button type="button" class="theme-customizer-color-option" :class="{ 'is-active': !primaryColors.some(color => color.value === settings.primaryColor) }" aria-label="选择自定义主色" title="自定义主色" @click="customColorInput.click()"><UiIcon name="theme" :size="25" /></button>
        <input ref="customColorInput" class="theme-customizer-native-input" type="color" tabindex="-1" aria-label="自定义主色" :value="settings.primaryColor" @input="savePatch({ primaryColor: $event.target.value })" />
      </div>

      <h3>主题</h3>
      <div class="theme-customizer-option-grid">
        <button v-for="option in themeOptions" :key="option.value" type="button" class="theme-customizer-card-option" :class="{ 'is-active': settings.theme === option.value }" :aria-pressed="settings.theme === option.value" @click="savePatch({ theme: option.value })"><n-icon class="theme-customizer-theme-symbol" :component="option.icon" :size="28" aria-hidden="true" /><span>{{ option.label }}</span></button>
      </div>

      <h3>圆角</h3>
      <div class="theme-customizer-radius-grid">
        <button v-for="option in radiusOptions" :key="option.value" type="button" class="theme-customizer-radius-option" :class="{ 'is-active': settings.radius === option.value }" :aria-label="`${option.label}圆角`" :aria-pressed="settings.radius === option.value" @click="savePatch({ radius: option.value })"><span class="theme-customizer-radius-preview"><i :style="{ borderRadius: option.size }" /></span><span>{{ option.label }}</span></button>
      </div>

      <div class="theme-customizer-glass-toggle"><h3>玻璃效果</h3><n-switch :value="settings.glassEnabled" aria-label="启用玻璃效果" @update:value="savePatch({ glassEnabled: $event })" /></div>
      <label class="theme-customizer-field-label">玻璃材质</label>
      <div class="theme-customizer-option-grid">
        <button v-for="option in materialOptions" :key="option.value" type="button" class="theme-customizer-card-option theme-customizer-material-option" :class="[{ 'is-active': settings.glassAppearance === option.value }, `material-${option.value}`]" :aria-pressed="settings.glassAppearance === option.value" :disabled="!settings.glassEnabled" @click="savePatch({ glassAppearance: option.value })"><span class="theme-customizer-material-preview" aria-hidden="true"><i /></span><span>{{ option.label }}</span></button>
      </div>
      <label class="theme-customizer-field-label" for="glass-ui-style">阅读风格</label>
      <n-select id="glass-ui-style" :value="settings.glassUIStyle" :options="styleOptions" :disabled="!settings.glassEnabled" aria-label="阅读风格" @update:value="savePatch({ glassUIStyle: $event })" />
      <div class="theme-customizer-slider-label"><label for="glass-reflection">反射强度</label><span>{{ settings.glassReflectionStrength }}%</span></div>
      <n-slider id="glass-reflection" :value="settings.glassReflectionStrength" :min="0" :max="100" :step="1" :disabled="!settings.glassEnabled" aria-label="反射强度" @update:value="savePatch({ glassReflectionStrength: $event })" />
      <div class="theme-customizer-slider-label"><label for="glass-transparency">透明强度</label><span>{{ settings.glassTransparencyStrength }}%</span></div>
      <n-slider id="glass-transparency" :value="settings.glassTransparencyStrength" :min="0" :max="100" :step="1" :disabled="!settings.glassEnabled" aria-label="透明强度" @update:value="savePatch({ glassTransparencyStrength: $event })" />

      <h3>壁纸</h3>
      <n-select :value="settings.wallpaperMode" :options="wallpaperOptions" aria-label="壁纸模式" @update:value="changeWallpaperMode" />
      <template v-if="settings.wallpaperMode === 'custom'">
        <label class="theme-customizer-field-label" for="wallpaper-url">图片地址</label>
        <n-input-group><n-input id="wallpaper-url" :value="wallpaperUrl" placeholder="https://example.com/wallpaper.jpg" aria-label="壁纸图片地址" @update:value="changeWallpaperUrl" @keyup.enter="applyWallpaperUrl" /><n-button :loading="wallpaperBusy" aria-label="应用壁纸" @click="applyWallpaperUrl">应用</n-button></n-input-group>
        <n-button class="theme-customizer-upload" block :loading="wallpaperBusy" @click="wallpaperFileInput.click()"><template #icon><UiIcon name="upload" /></template>上传图片</n-button>
        <input ref="wallpaperFileInput" class="theme-customizer-native-input" type="file" accept="image/jpeg,image/png,image/webp" tabindex="-1" aria-label="上传壁纸图片" @change="uploadWallpaper" />
        <n-text class="theme-customizer-wallpaper-note" depth="3">支持 JPEG、PNG、WebP，图片自动缩小并保存在当前浏览器。</n-text>
        <img v-if="settings.wallpaper" class="theme-customizer-wallpaper-preview" :src="settings.wallpaper" alt="当前自定义壁纸预览" />
      </template>
    </div>
  </section>
</template>

<style scoped>
.theme-customizer-panel { display: flex; flex-direction: column; height: 100%; min-height: 0; color: var(--ui-text); }
.theme-customizer-header { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 22px 24px; border-bottom: 1px solid var(--ui-border); }
.theme-customizer-header h2 { margin: 0 0 6px; font-size: 21px; font-weight: 600; }
.theme-customizer-header :deep(.n-text) { font-size: 12px; }
.theme-customizer-actions { display: flex; gap: 2px; flex-shrink: 0; }
.theme-customizer-error { margin: 14px 24px 0; }
.theme-customizer-body { flex: 1; min-height: 0; overflow-y: auto; overscroll-behavior: contain; padding: 0 24px 30px; }
.theme-customizer-body h3 { margin: 26px 0 16px; font-size: 16px; font-weight: 600; }
.theme-customizer-color-grid { display: grid; grid-template-columns: repeat(auto-fill, 44px); gap: 10px; }
.theme-customizer-color-option { display: flex; align-items: center; justify-content: center; width: 44px; height: 44px; border: 1px solid var(--ui-border); border-radius: 10px; background: transparent; color: inherit; cursor: pointer; }
.theme-customizer-color-option > span { display: block; width: 28px; height: 28px; border-radius: 7px; }
.theme-customizer-native-input { position: absolute; width: 1px; height: 1px; opacity: 0; pointer-events: none; }
.theme-customizer-option-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 12px; }
.theme-customizer-card-option { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 10px; min-height: 92px; padding: 12px 6px; border: 1px solid var(--ui-border); border-radius: 10px; background: transparent; color: inherit; font: inherit; cursor: pointer; }
.theme-customizer-theme-symbol { font-size: 30px; line-height: 1; }
.theme-customizer-panel button.is-active { border-color: var(--accent-color); background: var(--accent-glow-color); box-shadow: 0 0 0 2px var(--accent-glow-color); }
.theme-customizer-panel button:focus-visible { outline: 2px solid var(--accent-color); outline-offset: 3px; }
.theme-customizer-panel button:disabled { cursor: default; opacity: 0.45; }
.theme-customizer-radius-grid { display: grid; grid-template-columns: repeat(5, minmax(0, 1fr)); gap: 8px; }
.theme-customizer-radius-option { display: flex; flex-direction: column; align-items: center; gap: 8px; min-width: 0; padding: 5px; border: 1px solid transparent; border-radius: 10px; background: transparent; color: inherit; font: inherit; font-size: 12px; cursor: pointer; }
.theme-customizer-radius-preview { display: grid; place-items: center; width: 100%; height: 48px; border-radius: 7px; background: var(--ui-surface-muted); }
.theme-customizer-radius-preview i { display: block; width: 32px; height: 32px; border: 1px solid var(--accent-color); background: var(--accent-glow-color); }
.theme-customizer-glass-toggle { display: flex; align-items: center; justify-content: space-between; margin-top: 28px; padding-top: 20px; border-top: 1px solid var(--ui-border); }
.theme-customizer-glass-toggle h3 { margin: 0; }
.theme-customizer-field-label { display: block; margin: 20px 0 10px; font-size: 13px; }
.theme-customizer-material-option { min-height: 98px; }
.theme-customizer-material-preview { display: grid; place-items: center; width: 65px; height: 38px; border-radius: 6px; background: linear-gradient(130deg, #768de1, #a687cf 48%, #f1b395); }
.theme-customizer-material-preview i { width: 43px; height: 24px; border: 1px solid rgba(255, 255, 255, .55); border-radius: 5px; background: rgba(255, 255, 255, .18); box-shadow: 0 2px 6px rgba(0, 0, 0, .1); }
.material-tinted .theme-customizer-material-preview i { background: color-mix(in srgb, var(--accent-color) 48%, transparent); }
.material-frosted .theme-customizer-material-preview i { backdrop-filter: blur(6px); background: rgba(255, 255, 255, .55); }
.theme-customizer-slider-label { display: flex; justify-content: space-between; gap: 8px; margin: 22px 0 9px; font-size: 13px; }
.theme-customizer-slider-label span { color: var(--ui-text-muted); }
.theme-customizer-upload { margin-top: 12px; }
.theme-customizer-wallpaper-note { display: block; margin-top: 10px; font-size: 12px; line-height: 1.6; }
.theme-customizer-wallpaper-preview { display: block; width: 100%; height: 130px; margin-top: 14px; border-radius: 10px; object-fit: cover; }
@media (max-width: 380px) { .theme-customizer-header { padding: 18px 16px; } .theme-customizer-body { padding-inline: 16px; } .theme-customizer-error { margin-inline: 16px; } .theme-customizer-option-grid { gap: 8px; } }
</style>
