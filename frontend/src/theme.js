// Palette and spacing based on MoviePilot-Frontend v3 (src/plugins/vuetify/theme.ts).
export const themeOptions = [
  { label: '浅色', key: 'light' },
  { label: '深色', key: 'dark' },
  { label: '跟随系统', key: 'system' },
  { label: '幻紫', key: 'purple' },
  { label: '透明', key: 'transparent' },
  { label: '玻璃', key: 'glass' },
];

export const themePalettes = {
  light: { primary: '#8448E6', accent: '#793AD2', background: '#F4F5FA', surface: '#FFFFFF', mutedSurface: '#F7F6FB', input: '#FCFBFF', inputBorder: '#898396', text: '#3A3541', muted: '#665F70', border: '#C6BED0', popup: '#FFFFFF' },
  dark: { primary: '#6E66ED', accent: '#ADA6FF', background: '#0E1116', surface: '#14161F', mutedSurface: '#202231', input: '#181A25', inputBorder: '#91899F', text: '#F0EDFA', muted: '#C0BBCE', border: '#575264', popup: '#202231' },
  purple: { primary: '#8448E6', accent: '#C4A2FF', background: '#28243D', surface: '#312D4B', mutedSurface: '#3D3759', input: '#302A47', inputBorder: '#A499B4', text: '#F0EBFC', muted: '#C8BEDD', border: '#766B8B', popup: '#373452' },
  transparent: { primary: '#8448E6', accent: '#CEAEFF', background: '#1C1C1C', surface: 'rgba(30, 30, 30, 0.3)', mutedSurface: 'rgba(22, 22, 30, 0.6)', input: 'rgba(22, 22, 30, 0.85)', inputBorder: '#A8A0B5', text: '#F3EDFC', muted: '#DED6EB', border: '#81768F', popup: '#292632' },
  glass: { primary: '#8448E6', accent: '#C9B6FF', background: '#0B1322', surface: 'rgba(23, 27, 32, 0.244)', mutedSurface: 'rgba(15, 24, 39, 0.6)', input: 'rgba(15, 24, 39, 0.8)', inputBorder: '#A2AFC6', text: '#F2F5FA', muted: '#D2DCE9', border: '#7D8A9F', popup: '#182235' },
};

export function resolveTheme(mode, prefersDark) {
  return mode === 'system' ? (prefersDark ? 'dark' : 'light') : (themePalettes[mode] ? mode : 'light');
}

export function moviePilotTheme(mode) {
  const { primary, accent, surface, background, text, muted, border, input, inputBorder, popup, mutedSurface } = themePalettes[mode] || themePalettes.light;
  const translucent = mode === 'transparent' || mode === 'glass';
  const semantic = mode === 'light'
    ? { Success: '#327817', Warning: '#865900', Error: '#B52240', Info: '#066C9F' }
    : { Success: '#A3DD70', Warning: '#FFD477', Error: '#FF9BA3', Info: '#83D2FF' };
  const buttonColors = { Success: '#327817', Warning: '#865900', Error: '#B52240', Info: '#066C9F' };
  const semanticButtons = Object.fromEntries(Object.entries(buttonColors).flatMap(([type, color]) => [
    [`color${type}`, color], [`colorHover${type}`, color], [`colorPressed${type}`, color],
    [`border${type}`, `1px solid ${color}`], [`borderHover${type}`, `1px solid ${color}`],
    [`borderPressed${type}`, `1px solid ${color}`], [`textColor${type}`, '#FFFFFF'],
    [`colorFocus${type}`, color], [`borderFocus${type}`, `1px solid ${color}`],
    [`textColorHover${type}`, '#FFFFFF'], [`textColorPressed${type}`, '#FFFFFF'], [`textColorFocus${type}`, '#FFFFFF'],
  ]));
  return {
    common: {
      primaryColor: accent, primaryColorHover: accent,
      primaryColorPressed: accent, primaryColorSuppl: accent,
      successColor: semantic.Success, warningColor: semantic.Warning, errorColor: semantic.Error, infoColor: semantic.Info,
      bodyColor: background, cardColor: surface, modalColor: popup, popoverColor: popup,
      textColorBase: text, textColor1: text, textColor2: muted, textColor3: muted,
      placeholderColor: muted,
      borderColor: border, dividerColor: border, borderRadius: '8px',
      fontFamily: 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif',
    },
    Layout: { color: translucent ? 'transparent' : background, headerColor: surface, siderColor: surface },
    Card: { color: surface, borderColor: border, borderRadius: '12px', paddingMedium: '24px', titleFontWeight: '600' },
    Button: { ...semanticButtons, borderRadiusMedium: '8px', borderRadiusSmall: '6px', fontWeight: '500', colorPrimary: primary, colorHoverPrimary: primary, colorPressedPrimary: primary, colorFocusPrimary: primary, borderPrimary: `1px solid ${primary}`, borderHoverPrimary: `1px solid ${primary}`, borderPressedPrimary: `1px solid ${primary}`, borderFocusPrimary: `1px solid ${primary}`, border: `1px solid ${inputBorder}`, textColorPrimary: '#FFFFFF', textColorHoverPrimary: '#FFFFFF', textColorPressedPrimary: '#FFFFFF', textColorFocusPrimary: '#FFFFFF' },
    Input: { color: input, colorFocus: input, textColor: text, placeholderColor: muted, border: `1px solid ${inputBorder}`, borderHover: `1px solid ${accent}`, borderFocus: `1px solid ${accent}`, boxShadowFocus: '0 0 0 3px var(--accent-glow-color)', borderRadius: '8px' },
    Select: { peers: { InternalSelection: { color: input, colorActive: input, textColor: text, placeholderColor: muted, border: `1px solid ${inputBorder}`, borderHover: `1px solid ${accent}`, borderActive: `1px solid ${accent}`, borderFocus: `1px solid ${accent}` } } },
    Checkbox: { color: input, border: `1px solid ${inputBorder}` },
    Radio: { color: input, border: `1px solid ${inputBorder}` },
    Form: { labelTextColor: text, feedbackTextColor: muted },
    DataTable: { thColor: mutedSurface, thTextColor: text, tdTextColor: text, tdColor: translucent ? 'transparent' : surface, tdColorStriped: mutedSurface, borderColor: border, thFontWeight: '600' },
    Descriptions: { thColor: mutedSurface, tdColor: translucent ? 'transparent' : surface, borderColor: border, thTextColor: muted, tdTextColor: text },
    Menu: {
      color: 'transparent', itemHeight: '44px', borderRadius: '8px',
      itemTextColor: muted, itemIconColor: muted, itemTextColorHover: text,
      itemTextColorActive: accent, itemIconColorActive: accent,
      itemTextColorActiveHover: accent, itemIconColorActiveHover: accent,
      itemColorActive: 'var(--accent-glow-color)',
      itemColorActiveHover: 'var(--accent-glow-color)',
    },
  };
}
