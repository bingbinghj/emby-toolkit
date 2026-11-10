import { computed, reactive, readonly, ref } from 'vue';

const STORAGE_KEY = 'etk-wallpaper-settings';
const LAST_IMAGE_KEY = 'etk-wallpaper-last-image';
export const MAX_WALLPAPER_LENGTH = 2 * 1024 * 1024;
export const wallpaperSources = Object.freeze([
  { title: 'TMDb 电影背景', value: 'tmdb' },
  { title: 'Bing 每日壁纸', value: 'bing' },
  { title: 'Emby 媒体背景', value: 'emby' },
  { title: '自定义壁纸 API', value: 'api' },
]);
export const wallpaperRotationIntervals = Object.freeze([
  { title: '不轮换', value: 0 }, { title: '15 秒', value: 15 },
  { title: '1 分钟', value: 60 }, { title: '5 分钟', value: 300 },
  { title: '10 分钟', value: 600 }, { title: '30 分钟', value: 1800 },
  { title: '1 小时', value: 3600 }, { title: '6 小时', value: 21600 },
  { title: '12 小时', value: 43200 }, { title: '1 天', value: 86400 },
]);
export const wallpaperPresets = Object.freeze([
  { id: 'dusk', title: '山间暮色', image: '/wallpapers/dusk.svg' },
  { id: 'lake', title: '湖畔晨光', image: '/wallpapers/lake.svg' },
  { id: 'night', title: '静谧星空', image: '/wallpapers/night.svg' },
]);
const defaults = { mode: 'preset', presetId: 'dusk', customImage: '', source: 'tmdb', customApi: '', rotationInterval: 0 };

export function normalizeWallpaperImage(input) {
  if (typeof input !== 'string') return '';
  const value = input.trim();
  if (value.length > MAX_WALLPAPER_LENGTH) return '';
  if (/^data:image\/(?:jpeg|png|webp);base64,[A-Za-z0-9+/]+={0,2}$/i.test(value)) return value;
  try {
    const url = new URL(value);
    return ['http:', 'https:'].includes(url.protocol) ? url.href : '';
  } catch { return ''; }
}

export function normalizeWallpaperSettings(input = {}) {
  return {
    mode: ['preset', 'custom', 'remote', 'none'].includes(input.mode) ? input.mode : defaults.mode,
    presetId: wallpaperPresets.some(preset => preset.id === input.presetId) ? input.presetId : defaults.presetId,
    customImage: normalizeWallpaperImage(input.customImage),
    source: wallpaperSources.some(source => source.value === input.source) ? input.source : defaults.source,
    customApi: normalizeWallpaperImage(input.customApi).startsWith('http') ? normalizeWallpaperImage(input.customApi) : '',
    rotationInterval: Number.isFinite(input.rotationInterval)
      ? Math.round(Math.max(0, Math.min(86400, input.rotationInterval))) : defaults.rotationInterval,
  };
}

function readSettings() {
  if (typeof window === 'undefined') return { ...defaults };
  try {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    if (stored) {
      const value = JSON.parse(stored);
      if (value && typeof value === 'object') return normalizeWallpaperSettings(value);
    }
  } catch { /* Invalid or unavailable storage falls back to the previous preferences. */ }
  try {
    const old = JSON.parse(window.localStorage.getItem('moviepilot-theme-customizer') || '{}');
    const customImage = normalizeWallpaperImage(old.wallpaperImage || old.wallpaper);
    const migrated = normalizeWallpaperSettings({
      mode: old.wallpaperMode === 'none' ? 'none' : old.wallpaperMode === 'custom' && customImage ? 'custom' : 'preset',
      customImage,
    });
    if (['wallpaperMode', 'wallpaperImage', 'wallpaper'].some(key => Object.hasOwn(old, key))) {
      try { window.localStorage.setItem(STORAGE_KEY, JSON.stringify(migrated)); }
      catch { /* Keep the migrated preferences in memory when storage is unavailable. */ }
    }
    return migrated;
  } catch { return { ...defaults }; }
}

const settings = reactive(readSettings());
const sourceImages = ref([]), sourceLoading = ref(false), sourceError = ref('');
let sourceRevision = 0, sourceController;
const images = computed(() => {
  if (settings.mode === 'none') return [];
  if (settings.mode === 'custom') return settings.customImage ? [settings.customImage] : [];
  if (settings.mode === 'remote') return sourceImages.value;
  const first = wallpaperPresets.findIndex(preset => preset.id === settings.presetId);
  return [...wallpaperPresets.slice(first), ...wallpaperPresets.slice(0, first)].map(preset => preset.image);
});
function lastImage() {
  try { return normalizeWallpaperCandidates(window.localStorage.getItem(LAST_IMAGE_KEY))[0] || ''; }
  catch { return ''; }
}
const activeImage = ref(lastImage()), previousImage = ref(''), crossfading = ref(false);
// MP's tone helper uses 0.72 until a decoded image supplies its bounded exposure.
const activeExposure = ref(0.72), previousExposure = ref(0.72);
const transitionStartedAt = ref(0), ready = ref(false);
const presentation = { activeImage, previousImage, activeExposure, previousExposure, crossfading, transitionStartedAt, ready };

function updateSettings(patch) {
  const next = normalizeWallpaperSettings({ ...settings, ...patch });
  try {
    if (typeof window !== 'undefined') window.localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch (error) {
    throw new Error('无法保存壁纸设置，浏览器存储空间可能已满或被禁用。', { cause: error });
  }
  Object.assign(settings, next);
}

export function normalizeWallpaperCandidates(value) {
  const candidates = Array.isArray(value) ? value : Array.isArray(value?.images) ? value.images : [value?.url || value];
  return [...new Set(candidates.map(candidate => typeof candidate === 'string' && /^\/image_proxy\/Items\/[A-Za-z0-9_-]+\/Images\/Backdrop\/0\?/.test(candidate)
    ? candidate : normalizeWallpaperImage(candidate)).filter(Boolean))].slice(0, 50);
}

function cancelSourceRequest() {
  sourceRevision++;
  sourceController?.abort();
  sourceController = undefined;
  sourceLoading.value = false;
}

async function refreshSource() {
  cancelSourceRequest();
  if (settings.mode !== 'remote') return false;
  const revision = sourceRevision;
  const controller = new AbortController();
  sourceController = controller;
  sourceLoading.value = true;
  sourceError.value = '';
  const timeout = window.setTimeout(() => controller.abort(), 20000);
  try {
    const custom = settings.source === 'api';
    if (custom && !settings.customApi) throw new Error('请填写有效的 HTTP / HTTPS 壁纸 API 地址。');
    const response = await fetch(custom ? settings.customApi : `/api/wallpaper/${settings.source}`, {
      signal: controller.signal, cache: 'no-store', credentials: custom ? 'omit' : 'same-origin',
    });
    const data = await response.json();
    if (!response.ok) throw new Error(data?.error || '无法读取壁纸来源，请稍后重试。');
    const next = normalizeWallpaperCandidates(data);
    if (!next.length) throw new Error('壁纸来源未返回可用图片，请检查后重试。');
    if (revision !== sourceRevision) return false;
    sourceImages.value = next;
    return true;
  } catch (error) {
    if (revision === sourceRevision) sourceError.value = error.name === 'AbortError'
      ? '壁纸来源请求超时，请稍后重试。' : error.message || '壁纸来源加载失败，当前背景已保留。';
    return false;
  } finally {
    window.clearTimeout(timeout);
    if (revision === sourceRevision) {
      sourceController = undefined;
      sourceLoading.value = false;
    }
  }
}

function setCustomImage(input) {
  const image = normalizeWallpaperImage(input);
  if (!image) throw new Error('请选择 JPEG、PNG、WebP 图片，或输入有效的 HTTP / HTTPS 图片地址。');
  updateSettings({ mode: 'custom', customImage: image });
}

function setPresentation(patch) {
  for (const key of Object.keys(presentation)) {
    if (Object.hasOwn(patch, key)) presentation[key].value = patch[key];
  }
  if (patch.activeImage) {
    try { window.localStorage.setItem(LAST_IMAGE_KEY, patch.activeImage); }
    catch { /* Rendering still works when the optional last-image cache is unavailable. */ }
  }
}

export function preloadWallpaperImage(source) {
  return new Promise((resolve, reject) => {
    const image = new Image();
    const timeout = window.setTimeout(() => finish(new Error('图片加载超时，请检查地址后重试。')), 15000);
    function finish(error) {
      window.clearTimeout(timeout);
      image.onload = image.onerror = null;
      if (error) reject(error); else resolve(image);
    }
    image.onload = () => finish(image.naturalWidth && image.naturalHeight ? null : new Error('图片尺寸无效。'));
    image.onerror = () => finish(new Error('图片加载失败，请检查地址或选择其他图片。'));
    image.src = source;
  });
}

export function useWallpaper() {
  return {
    settings: readonly(settings), presets: wallpaperPresets, sources: wallpaperSources, images,
    sourceLoading: readonly(sourceLoading), sourceError: readonly(sourceError), refreshSource, cancelSourceRequest,
    updateSettings, setCustomImage, resetSettings: () => updateSettings(defaults),
    activeImage: readonly(activeImage), previousImage: readonly(previousImage),
    activeExposure: readonly(activeExposure), previousExposure: readonly(previousExposure),
    crossfading: readonly(crossfading), transitionStartedAt: readonly(transitionStartedAt), ready: readonly(ready),
    setPresentation,
  };
}
