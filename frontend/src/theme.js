// Palette and spacing based on MoviePilot-Frontend v3 (src/plugins/vuetify/theme.ts).
export const themeOptions = [
  { label: '浅色', key: 'light' },
  { label: '深色', key: 'dark' },
  { label: '跟随系统', key: 'system' },
  { label: '幻紫', key: 'purple' },
  { label: '透明', key: 'transparent' },
  { label: '玻璃', key: 'glass' },
];

const standardSemantic = { success: '#56CA00', info: '#16B1FF', warning: '#FFB400', error: '#FF4C51' };
export const themePalettes = {
  light: { ...standardSemantic, primary: '#8D51F9', background: '#F4F5FA', surface: '#FFFFFF', mutedSurface: '#F9FAFC', input: '#FFFFFF', inputBorder: 'rgba(58,53,65,.22)', borderSoft: 'rgba(58,53,65,.06)', text: '#3A3541', muted: 'rgba(58,53,65,.68)', border: 'rgba(58,53,65,.12)', popup: '#FFFFFF' },
  dark: { ...standardSemantic, primary: '#6E66ED', background: '#0E1116', surface: '#14161F', mutedSurface: '#14161F', input: '#14161F', inputBorder: 'rgba(231,227,252,.22)', borderSoft: 'rgba(231,227,252,.1)', text: '#E7E3FC', muted: 'rgba(231,227,252,.68)', border: 'rgba(231,227,252,.12)', popup: '#14161F' },
  purple: { ...standardSemantic, primary: '#8D51F9', background: '#28243D', surface: '#312D4B', mutedSurface: '#3D3759', input: '#312D4B', inputBorder: 'rgba(231,227,252,.22)', borderSoft: 'rgba(231,227,252,.1)', text: '#E7E3FC', muted: 'rgba(231,227,252,.68)', border: 'rgba(231,227,252,.12)', popup: '#312D4B' },
  transparent: { primary: '#A370F7', success: '#66BB6A', info: '#42A5F5', warning: '#FFA726', error: '#EF5350', background: '#1C1C1C', surface: 'rgba(30,30,30,.3)', mutedSurface: 'rgba(30,30,30,.2)', input: 'rgba(30,30,30,.3)', inputBorder: 'rgba(231,227,252,.22)', borderSoft: 'rgba(231,227,252,.05)', text: '#E7E3FC', muted: 'rgba(255,255,255,.65)', border: 'rgba(231,227,252,.18)', popup: '#1C1C1C' },
  glass: { primary: '#8D51F9', success: '#69C59A', info: '#7C9CC4', warning: '#E4B863', error: '#FF7B7B', onSuccess: '#07150F', onWarning: '#171106', background: '#0B1322', surface: '#182235', mutedSurface: 'rgba(255,255,255,.06)', input: 'rgba(11,19,34,.52)', inputBorder: 'rgba(255,255,255,.14)', borderSoft: 'rgba(255,255,255,.1)', text: '#F2F5FA', muted: 'rgba(242,245,250,.72)', border: 'rgba(255,255,255,.14)', popup: '#182235' },
};

export function resolveTheme(mode, prefersDark) {
  return mode === 'system' ? (prefersDark ? 'dark' : 'light') : (themePalettes[mode] ? mode : 'light');
}

const lightMaterialPalettes = {
  transparent: { ...themePalettes.light, muted: 'rgba(58,53,65,.72)', surface: 'rgba(255,255,255,.3)', mutedSurface: 'rgba(255,255,255,.2)', input: 'rgba(255,255,255,.65)' },
  glass: { ...themePalettes.light, muted: 'rgba(58,53,65,.72)', surface: 'rgba(255,255,255,.65)', mutedSurface: 'rgba(58,53,65,.04)', input: 'rgba(255,255,255,.65)' },
};

export function resolveColorScheme(mode, prefersDark) {
  return resolveTheme(mode === 'transparent' || mode === 'glass' ? 'system' : mode, prefersDark) === 'light' ? 'light' : 'dark';
}

export function getThemePalette(mode, prefersDark = true) {
  const resolved = resolveTheme(mode, prefersDark);
  return (!prefersDark && lightMaterialPalettes[resolved]) || themePalettes[resolved];
}

export function textColorForBackground(color) {
  const rgb = color.slice(1).match(/../g).map(value => parseInt(value, 16) / 255);
  const brightness = rgb.reduce((sum, value, index) => sum + (value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4) * [.2126, .7152, .0722][index], 0);
  return brightness > .179 ? '#000000' : '#FFFFFF';
}

export function getThemeTextColors(mode, prefersDark = true) {
  const palette = getThemePalette(mode, prefersDark);
  const colors = resolveColorScheme(mode, prefersDark) === 'light'
    ? { muted: '#696471', primary: '#7133C9', success: '#267000', warning: '#815800', error: '#B9233A', info: '#00649C' }
    : { muted: '#C5C0D3', primary: '#BDA5FF', success: '#93D775', warning: '#FFC56D', error: '#FF969E', info: '#7BCAFA' };
  // Keep white primary actions legible while retaining the original purple hue.
  colors.primarySurface = ({ '#8D51F9': '#884AF0', '#6E66ED': '#625AE0', '#A370F7': '#9157DD' })[palette.primary.toUpperCase()] || palette.primary;
  for (const type of ['Primary', 'Success', 'Warning', 'Error', 'Info']) {
    colors[`on${type}`] = type === 'Primary' ? '#FFFFFF' : palette[`on${type}`] || textColorForBackground(palette[type.toLowerCase()]);
  }
  return colors;
}

export function moviePilotTheme(mode, prefersDark = true) {
  const palette = getThemePalette(mode, prefersDark);
  const ink = getThemeTextColors(mode, prefersDark);
  const { primary, surface, background, text, border, borderSoft, input, inputBorder, popup, mutedSurface } = palette;
  const muted = ink.muted;
  const translucent = mode === 'transparent' || mode === 'glass';
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
      borderColor: border, dividerColor: border, borderRadius: '8px',
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
    Card: { color: surface, borderColor: borderSoft, borderRadius: '12px', paddingMedium: '24px', titleFontWeight: '600' },
    Button: { ...buttonText, ...actionText, ...defaultActionText, colorPrimary: ink.primarySurface, colorHoverPrimary: ink.primarySurface, colorPressedPrimary: ink.primarySurface, colorFocusPrimary: ink.primarySurface, borderRadiusMedium: '8px', borderRadiusSmall: '6px', fontWeight: '500', color: 'transparent', colorHover: mutedSurface, colorPressed: mutedSurface, textColor: text },
    Input: { color: input, colorFocus: input, textColor: text, placeholderColor: muted, border: `1px solid ${inputBorder}`, borderHover: `1px solid ${primary}`, borderFocus: `1px solid ${primary}`, boxShadowFocus: '0 0 0 3px var(--accent-glow-color)', borderRadius: '8px' },
    Select: { peers: { InternalSelection: { color: input, colorActive: input, textColor: text, placeholderColor: muted, border: `1px solid ${inputBorder}`, borderHover: `1px solid ${primary}`, borderActive: `1px solid ${primary}`, borderFocus: `1px solid ${primary}` } } },
    Checkbox: { color: input, border: `1px solid ${inputBorder}` },
    Radio: { color: input, border: `1px solid ${inputBorder}`, buttonTextColorActive: ink.primary },
    Form: { labelTextColor: text, feedbackTextColor: muted },
    DataTable: { thColor: mutedSurface, thTextColor: text, tdTextColor: text, tdColor: translucent ? 'transparent' : surface, tdColorStriped: mutedSurface, borderColor: borderSoft, thFontWeight: '600' },
    Descriptions: { thColor: mutedSurface, tdColor: translucent ? 'transparent' : surface, borderColor: border, thTextColor: muted, tdTextColor: text },
    Menu: {
      color: 'transparent', itemHeight: '44px', borderRadius: '8px',
      itemTextColor: muted, itemIconColor: muted, itemTextColorHover: text,
      itemTextColorActive: '#FFFFFF', itemIconColorActive: '#FFFFFF',
      itemTextColorActiveHover: '#FFFFFF', itemIconColorActiveHover: '#FFFFFF',
      itemColorActive: ink.primarySurface,
      itemColorActiveCollapsed: ink.primarySurface,
      itemColorActiveHover: ink.primarySurface,
    },
  };
}
