// Adapted from MoviePilot-Frontend v3 195b61d (MIT; see third-party/MoviePilot-LICENSE).
export const themeOptions = [
  { label: '浅色', key: 'light' },
  { label: '深色', key: 'dark' },
  { label: '跟随系统', key: 'system' },
];

export const themePalettes = {
  light: { primary: '#8D51F9', success: '#248A3D', info: '#007AFF', warning: '#FF9500', error: '#FF3B30', background: '#F2F2F7', surface: '#FFFFFF', mutedSurface: 'rgba(118,118,128,.08)', input: 'rgba(255,255,255,.8)', inputBorder: 'rgba(60,60,67,.22)', borderSoft: 'rgba(60,60,67,.1)', text: '#000000', muted: '#636366', border: 'rgba(60,60,67,.14)', popup: '#FFFFFF' },
  dark: { primary: '#8D51F9', success: '#69C59A', info: '#7C9CC4', warning: '#E4B863', error: '#FF7B7B', onSuccess: '#07150F', onWarning: '#171106', background: '#0B1322', surface: '#182235', mutedSurface: 'rgba(255,255,255,.06)', input: 'rgba(11,19,34,.52)', inputBorder: 'rgba(255,255,255,.2)', borderSoft: 'rgba(255,255,255,.1)', text: '#F2F5FA', muted: '#BBC3D0', border: 'rgba(255,255,255,.14)', popup: '#182235' },
};

export function resolveTheme(mode, prefersDark) {
  return mode === 'light' || mode === 'dark' ? mode : (prefersDark ? 'dark' : 'light');
}
export const resolveColorScheme = resolveTheme;

export function getThemePalette(mode, prefersDark = true, appearance = {}) {
  const palette = { ...themePalettes[resolveTheme(mode, prefersDark)], primary: appearance.primaryColor || '#8D51F9' };
  if (appearance.glassEnabled === false) palette.input = palette.surface;
  return palette;
}

export function textColorForBackground(color) {
  const rgb = color.slice(1).match(/../g).map(value => parseInt(value, 16) / 255);
  const brightness = rgb.reduce((sum, value, index) => sum + (value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4) * [.2126, .7152, .0722][index], 0);
  return brightness > .179 ? '#000000' : '#FFFFFF';
}

// Keep custom colors legible for text actions as well as filled buttons.
function readableAccent(color, dark) {
  const rgb = color.slice(1).match(/../g).map(value => parseInt(value, 16));
  const background = dark ? [90, 90, 90] : [204, 204, 204];
  const luminance = values => values.reduce((sum, value, index) => {
    const channel = value / 255;
    return sum + (channel <= .04045 ? channel / 12.92 : ((channel + .055) / 1.055) ** 2.4) * [.2126, .7152, .0722][index];
  }, 0);
  const base = luminance(background);
  for (let step = 0; step < 20; step++) {
    const ink = luminance(rgb);
    if ((Math.max(base, ink) + .05) / (Math.min(base, ink) + .05) >= 4.5) break;
    rgb.forEach((value, index) => { rgb[index] = Math.round(value * .9 + (dark ? 255 : 0) * .1); });
  }
  return '#' + rgb.map(value => value.toString(16).padStart(2, '0')).join('');
}

export function getThemeTextColors(mode, prefersDark = true, appearance = {}) {
  const palette = getThemePalette(mode, prefersDark, appearance);
  const dark = resolveTheme(mode, prefersDark) === 'dark';
  const colors = dark
    ? { muted: '#CDD3DD', success: '#69C59A', warning: '#E4B863', error: '#FF969E', info: '#9ABBE4' }
    : { muted: '#525255', success: '#1D7131', warning: '#815800', error: '#B9233A', info: '#00649C' };
  colors.primary = readableAccent(palette.primary, dark);
  for (const type of ['success', 'warning', 'error', 'info']) colors[type] = readableAccent(colors[type], dark);
  colors.primarySurface = palette.primary.toUpperCase() === '#8D51F9' ? '#884AF0' : palette.primary;
  for (const type of ['Primary', 'Success', 'Warning', 'Error', 'Info']) {
    colors[`on${type}`] = palette[`on${type}`] || textColorForBackground(type === 'Primary' ? colors.primarySurface : palette[type.toLowerCase()]);
  }
  return colors;
}

export function moviePilotTheme(mode, prefersDark = true, appearance = {}) {
  const palette = getThemePalette(mode, prefersDark, appearance);
  const ink = getThemeTextColors(mode, prefersDark, appearance);
  const { primary, surface, background, text, border, borderSoft, input, inputBorder, popup, mutedSurface } = palette;
  const muted = ink.muted;
  const translucent = appearance.glassEnabled !== false;
  const radii = themeRadii[appearance.radius] || themeRadii.default;
  const colors = Object.fromEntries(['primary', 'success', 'warning', 'error', 'info'].flatMap(key =>
    ['Color', 'ColorHover', 'ColorPressed', 'ColorSuppl'].map(state => [`${key}${state}`, palette[key]])
  ));
  const buttonText = Object.fromEntries(['Primary', 'Success', 'Warning', 'Error', 'Info'].flatMap(type =>
    ['', 'Hover', 'Pressed', 'Focus'].map(state => [`textColor${state}${type}`, ink[`on${type}`]])
  ));
  const semanticText = Object.fromEntries(['Primary', 'Success', 'Warning', 'Error', 'Info'].map(type => [`textColor${type}`, ink[type.toLowerCase()]]));
  const actionText = Object.fromEntries(['Primary', 'Success', 'Warning', 'Error', 'Info'].flatMap(type =>
    ['Ghost', 'Text'].flatMap(style => ['', 'Hover', 'Pressed', 'Focus'].map(state => [`textColor${style}${state}${type}`, ink[type.toLowerCase()]]))
  ));
  const defaultActionText = Object.fromEntries(['', 'Ghost', 'Text'].flatMap(style =>
    ['Hover', 'Pressed', 'Focus'].map(state => [`textColor${style}${state}`, ink.primary])
  ));
  return {
    common: {
      ...colors,
      bodyColor: background, cardColor: surface, modalColor: popup, popoverColor: popup,
      textColorBase: text, textColor1: text, textColor2: muted, textColor3: muted,
      placeholderColor: muted,
      borderColor: border, dividerColor: border, borderRadius: `${radii.control}px`,
      fontFamily: 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif',
    },
    Layout: { color: translucent ? 'transparent' : background, headerColor: surface, siderColor: surface },
    Dialog: { textColor: text },
    Popover: { textColor: text },
    Empty: { textColor: muted },
    Typography: { ...semanticText, textColor: text, aTextColor: ink.primary },
    Tag: { ...semanticText },
    Tabs: { tabTextColorActiveLine: ink.primary, tabTextColorActiveBar: ink.primary, tabTextColorActiveCard: ink.primary },
    Pagination: { itemTextColorActive: ink.primary },
    Switch: { textColor: ink.onPrimary, railColorActive: ink.primarySurface },
    Card: { color: surface, borderColor: borderSoft, borderRadius: `${radii.surface}px`, paddingMedium: '24px', titleFontWeight: '600' },
    Button: { ...buttonText, ...actionText, ...defaultActionText, colorPrimary: ink.primarySurface, colorHoverPrimary: ink.primarySurface, colorPressedPrimary: ink.primarySurface, colorFocusPrimary: ink.primarySurface, borderRadiusMedium: `${radii.control}px`, borderRadiusSmall: `${radii.control}px`, fontWeight: '500', color: 'transparent', colorHover: mutedSurface, colorPressed: mutedSurface, textColor: text },
    Input: { color: input, colorFocus: input, textColor: text, placeholderColor: muted, border: `1px solid ${inputBorder}`, borderHover: `1px solid ${primary}`, borderFocus: `1px solid ${primary}`, boxShadowFocus: '0 0 0 3px var(--accent-glow-color)', borderRadius: `${radii.control}px` },
    Select: { peers: { InternalSelection: { color: input, colorActive: input, textColor: text, placeholderColor: muted, border: `1px solid ${inputBorder}`, borderHover: `1px solid ${primary}`, borderActive: `1px solid ${primary}`, borderFocus: `1px solid ${primary}` } } },
    Checkbox: { color: input, border: `1px solid ${inputBorder}`, colorChecked: ink.primarySurface, borderChecked: `1px solid ${ink.primarySurface}`, checkMarkColor: ink.onPrimary },
    Radio: { color: input, border: `1px solid ${inputBorder}`, buttonColorActive: ink.primarySurface, buttonTextColorActive: ink.onPrimary },
    Form: { labelTextColor: text, feedbackTextColor: muted },
    DataTable: { thColor: mutedSurface, thTextColor: text, tdTextColor: text, tdColor: translucent ? 'transparent' : surface, tdColorStriped: mutedSurface, borderColor: borderSoft, thFontWeight: '600' },
    Descriptions: { thColor: mutedSurface, tdColor: translucent ? 'transparent' : surface, borderColor: border, thTextColor: muted, tdTextColor: text },
    Menu: {
      color: 'transparent', itemHeight: '44px', borderRadius: `${radii.control}px`,
      itemTextColor: muted, itemIconColor: muted, itemTextColorHover: text,
      itemTextColorActive: ink.onPrimary, itemIconColorActive: ink.onPrimary,
      itemTextColorActiveHover: ink.onPrimary, itemIconColorActiveHover: ink.onPrimary,
      itemColorActive: ink.primarySurface,
      itemColorActiveCollapsed: ink.primarySurface,
      itemColorActiveHover: ink.primarySurface,
    },
  };
}

export const themeRadii = {
  none: { surface: 8, field: 8, control: 8, overlay: 10 },
  small: { surface: 12, field: 12, control: 10, overlay: 14 },
  default: { surface: 20, field: 16, control: 14, overlay: 22 },
  large: { surface: 24, field: 20, control: 16, overlay: 26 },
  extra: { surface: 28, field: 24, control: 18, overlay: 30 },
};

// MoviePilot's standard glass response curves; no continuous renderer is needed.
function glassResponse(value, anchors) {
  const stops = [0, 20, 50, 70, 85, 100];
  const upper = stops.findIndex(stop => value <= stop);
  if (upper <= 0) return anchors[0];
  const progress = (value - stops[upper - 1]) / (stops[upper] - stops[upper - 1]);
  return anchors[upper - 1] + (anchors[upper] - anchors[upper - 1]) * progress * progress * (3 - 2 * progress);
}

export function applyDocumentTheme(appearance, prefersDark) {
  const scheme = resolveTheme(appearance.theme, prefersDark);
  const palette = getThemePalette(scheme, prefersDark, appearance);
  const ink = getThemeTextColors(scheme, prefersDark, appearance);
  const root = document.documentElement;
  const radius = themeRadii[appearance.radius] || themeRadii.default;
  const value = appearance.glassTransparencyStrength;
  const densities = {
    clear: [1, .88, .62, .42, .26, .18],
    tinted: [1, .92, .72, .52, .39, .3],
    frosted: [1, .9, .7, .52, .36, .22],
  };
  root.dataset.theme = appearance.glassEnabled ? 'glass' : 'plain';
  root.dataset.glassColorScheme = scheme;
  root.dataset.themePreference = appearance.theme;
  root.dataset.themeRadius = appearance.radius;
  root.dataset.glassAppearance = appearance.glassAppearance;
  root.dataset.glassUiStyle = appearance.glassUIStyle;
  root.dataset.wallpaperMode = appearance.wallpaperMode;
  root.classList.toggle('dark', scheme === 'dark');
  root.classList.toggle('light', scheme === 'light');
  root.style.colorScheme = scheme;
  const variables = {
    '--ui-page-bg': palette.background, '--ui-surface': palette.surface, '--ui-surface-muted': palette.mutedSurface,
    '--ui-text': palette.text, '--ui-text-muted': ink.muted, '--ui-border': palette.border,
    '--ui-border-soft': palette.borderSoft, '--ui-border-strong': palette.inputBorder, '--ui-input-border': palette.inputBorder,
    '--ui-input-bg': palette.input, '--accent-color': palette.primary, '--accent-glow-color': `color-mix(in srgb, ${palette.primary} 14%, transparent)`,
    '--modal-solid-bg-color': palette.popup,
    '--ui-error-color': palette.error, '--ui-warning-color': palette.warning, '--ui-info-color': palette.info, '--ui-success-color': palette.success,
    '--ui-primary-text': ink.primary, '--ui-success-text': ink.success, '--ui-warning-text': ink.warning,
    '--ui-error-text': ink.error, '--ui-info-text': ink.info,
    '--ui-primary-solid': ink.primarySurface, '--ui-on-primary': ink.onPrimary,
    '--ui-on-success': ink.onSuccess, '--ui-on-warning': ink.onWarning, '--ui-on-error': ink.onError, '--ui-on-info': ink.onInfo,
    '--ui-radius': `${radius.surface}px`, '--ui-radius-sm': `${radius.control}px`,
    '--ui-field-radius': `${radius.field}px`, '--ui-overlay-radius': `${radius.overlay}px`,
    '--glass-reflection': String(appearance.glassReflectionStrength / 100),
    '--glass-surface-density': String(glassResponse(value, densities[appearance.glassAppearance])),
    '--glass-tint-density': String(glassResponse(value, [1, .9, .65, .48, .36, .28])),
    '--glass-blur-surface': `${glassResponse(value, [64, 52, 36, 24, 15, 8])}px`,
    '--glass-blur-raised': `${glassResponse(value, [84, 70, 50, 35, 24, 16])}px`,
  };
  Object.entries(variables).forEach(([key, value]) => root.style.setProperty(key, value));
  document.querySelector('meta[name="theme-color"]')?.setAttribute('content', palette.background);
  return scheme;
}
