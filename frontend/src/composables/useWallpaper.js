import { computed, reactive, readonly, ref } from 'vue';

const STORAGE_KEY = 'etk-wallpaper-settings';
export const MAX_WALLPAPER_LENGTH = 2 * 1024 * 1024;
export const wallpaperPresets = Object.freeze([
  { id: 'dusk', title: '山间暮色', image: '/wallpapers/dusk.svg' },
  { id: 'lake', title: '湖畔晨光', image: '/wallpapers/lake.svg' },
  { id: 'night', title: '静谧星空', image: '/wallpapers/night.svg' },
]);
const defaults = { mode: 'preset', presetId: 'dusk', customImage: '', rotationInterval: 0 };

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

function normalize(input = {}) {
  return {
    mode: ['preset', 'custom', 'none'].includes(input.mode) ? input.mode : defaults.mode,
    presetId: wallpaperPresets.some(preset => preset.id === input.presetId) ? input.presetId : defaults.presetId,
    customImage: normalizeWallpaperImage(input.customImage),
    rotationInterval: Number.isFinite(input.rotationInterval)
      ? Math.round(Math.max(0, Math.min(3600, input.rotationInterval))) : defaults.rotationInterval,
  };
}

function readSettings() {
  if (typeof window === 'undefined') return { ...defaults };
  try {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    if (stored) {
      const value = JSON.parse(stored);
      if (value && typeof value === 'object') return normalize(value);
    }
  } catch { /* Invalid or unavailable storage falls back to the previous preferences. */ }
  try {
    const old = JSON.parse(window.localStorage.getItem('moviepilot-theme-customizer') || '{}');
    const customImage = normalizeWallpaperImage(old.wallpaperImage || old.wallpaper);
    const migrated = normalize({
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
const images = computed(() => {
  if (settings.mode === 'none') return [];
  if (settings.mode === 'custom') return settings.customImage ? [settings.customImage] : [];
  const first = wallpaperPresets.findIndex(preset => preset.id === settings.presetId);
  return [...wallpaperPresets.slice(first), ...wallpaperPresets.slice(0, first)].map(preset => preset.image);
});
const activeImage = ref(''), previousImage = ref(''), crossfading = ref(false);
const transitionStartedAt = ref(0), ready = ref(false);
const presentation = { activeImage, previousImage, crossfading, transitionStartedAt, ready };

function updateSettings(patch) {
  const next = normalize({ ...settings, ...patch });
  try {
    if (typeof window !== 'undefined') window.localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch (error) {
    throw new Error('无法保存壁纸设置，浏览器存储空间可能已满或被禁用。', { cause: error });
  }
  Object.assign(settings, next);
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
    settings: readonly(settings), presets: wallpaperPresets, images,
    updateSettings, setCustomImage, resetSettings: () => updateSettings(defaults),
    activeImage: readonly(activeImage), previousImage: readonly(previousImage),
    crossfading: readonly(crossfading), transitionStartedAt: readonly(transitionStartedAt), ready: readonly(ready),
    setPresentation,
  };
}
