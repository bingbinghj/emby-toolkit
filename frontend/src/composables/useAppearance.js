/*
 * Adapted from MoviePilot-Frontend v3 useThemeCustomizer.ts.
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
import { computed, reactive } from 'vue';

const STORAGE_KEY = 'moviepilot-theme-customizer';
export const MAX_WALLPAPER_LENGTH = 2 * 1024 * 1024;

export const primaryColors = [
  { name: '紫色', value: '#8D51F9' },
  { name: '靛蓝', value: '#3F51B5' },
  { name: '蓝色', value: '#1976D2' },
  { name: '青色', value: '#00BCD4' },
  { name: '蓝绿', value: '#009688' },
  { name: '绿色', value: '#4CAF50' },
  { name: '琥珀', value: '#FFB400' },
  { name: '橙色', value: '#FF9800' },
  { name: '珊瑚', value: '#FF4C51' },
  { name: '粉色', value: '#E91E63' },
  { name: '天蓝', value: '#16B1FF' },
  { name: '灰蓝', value: '#607D8B' },
];

export const appearanceDefaults = Object.freeze({
  theme: 'system',
  primaryColor: primaryColors[0].value,
  radius: 'default',
  glassEnabled: true,
  glassAppearance: 'clear',
  glassUIStyle: 'adaptive',
  glassReflectionStrength: 42,
  glassTransparencyStrength: 52,
  wallpaperMode: 'default',
  wallpaper: '',
});

export function normalizeThemeMode(input) {
  if (input === 'auto' || input === 'glass') return 'system';
  if (input === 'default') return 'light';
  if (input === 'purple' || input === 'transparent') return 'dark';
  return ['light', 'dark', 'system'].includes(input) ? input : 'system';
}

export function normalizeWallpaper(input) {
  if (typeof input !== 'string') return '';
  const value = input.trim();
  if (value.length > MAX_WALLPAPER_LENGTH) return '';
  if (/^data:image\/(?:jpeg|png|webp);base64,[A-Za-z0-9+/]+={0,2}$/i.test(value)) return value;
  try {
    const url = new URL(value);
    return ['http:', 'https:'].includes(url.protocol) ? url.href : '';
  } catch {
    return '';
  }
}

export function normalizeAppearance(input) {
  const value = input && typeof input === 'object' && !Array.isArray(input) ? input : {};
  const choice = (key, options) => options.includes(value[key]) ? value[key] : appearanceDefaults[key];
  const strength = key => typeof value[key] === 'number' && Number.isFinite(value[key])
    ? Math.round(Math.max(0, Math.min(100, value[key]))) : appearanceDefaults[key];
  return {
    theme: normalizeThemeMode(value.theme),
    primaryColor: typeof value.primaryColor === 'string' && /^#[\da-f]{6}$/i.test(value.primaryColor)
      ? value.primaryColor.toUpperCase() : appearanceDefaults.primaryColor,
    radius: value.radius === 'huge' ? 'extra' : choice('radius', ['none', 'small', 'default', 'large', 'extra']),
    glassEnabled: typeof value.glassEnabled === 'boolean' ? value.glassEnabled : appearanceDefaults.glassEnabled,
    glassAppearance: choice('glassAppearance', ['clear', 'tinted', 'frosted']),
    glassUIStyle: choice('glassUIStyle', ['adaptive', 'clear']),
    glassReflectionStrength: strength('glassReflectionStrength'),
    glassTransparencyStrength: strength('glassTransparencyStrength'),
    wallpaperMode: choice('wallpaperMode', ['default', 'custom', 'none']),
    wallpaper: normalizeWallpaper(value.wallpaper),
  };
}

function readAppearance() {
  if (typeof window === 'undefined') return { ...appearanceDefaults };
  try {
    const storage = window.localStorage;
    const legacyTheme = storage.getItem('moviepilot-theme');
    const legacyDark = storage.getItem('isDark');
    const legacy = {
      theme: legacyTheme || (legacyDark === null ? 'system' : legacyDark === 'true' ? 'dark' : 'light'),
      ...(legacyTheme === 'glass' ? { glassAppearance: 'frosted' } : {}),
      ...(legacyTheme === 'transparent' ? { glassAppearance: 'clear', glassUIStyle: 'clear' } : {}),
    };
    const stored = storage.getItem(STORAGE_KEY);
    if (!stored) return normalizeAppearance(legacy);
    try {
      const parsed = JSON.parse(stored);
      return normalizeAppearance({ ...legacy, ...(parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : {}) });
    } catch {
      return normalizeAppearance(legacy);
    }
  } catch {
    return { ...appearanceDefaults };
  }
}

const settings = reactive(readAppearance());
const wallpaper = computed(() => settings.wallpaper);

function updateSettings(patch) {
  const next = normalizeAppearance({ ...settings, ...patch });
  try {
    if (typeof window !== 'undefined') window.localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch (error) {
    throw new Error('无法保存外观设置，浏览器存储空间可能已满或被禁用。', { cause: error });
  }
  // 保存成功后才改变共享状态，失败时保留原来已经保存的外观。
  Object.assign(settings, next);
  return next;
}

function setWallpaper(value) {
  const normalized = normalizeWallpaper(value);
  if (!normalized) throw new Error('请选择有效图片，或输入 HTTP / HTTPS 图片地址。');
  return updateSettings({ wallpaper: normalized, wallpaperMode: 'custom' });
}

function resetSettings() {
  return updateSettings(appearanceDefaults);
}

export function useAppearance() {
  return { settings, updateSettings, resetSettings, wallpaper, setWallpaper };
}
