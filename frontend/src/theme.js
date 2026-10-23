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
  light: { primary: '#8D51F9', accent: '#8D51F9', background: '#F4F5FA', surface: '#FFFFFF', mutedSurface: '#F9FAFC', input: '#FFFFFF', inputBorder: '#C9C6CF', borderSoft: '#EEEFF4', text: '#3A3541', muted: '#716D78', border: '#E3E2E8', popup: '#FFFFFF' },
  dark: { primary: '#6E66ED', accent: '#6E66ED', background: '#0E1116', surface: '#14161F', mutedSurface: '#1C1F2B', input: '#191C27', inputBorder: '#454958', borderSoft: '#232631', text: '#E7E3FC', muted: '#AAA6BD', border: '#2E313E', popup: '#1C1F2B' },
  purple: { primary: '#8D51F9', accent: '#8D51F9', background: '#28243D', surface: '#312D4B', mutedSurface: '#3D3759', input: '#352F50', inputBorder: '#615A75', borderSoft: '#3C3855', text: '#E7E3FC', muted: '#B4AEC7', border: '#49435F', popup: '#373452' },
  transparent: { primary: '#A370F7', accent: '#A370F7', background: '#1C1C1C', surface: 'rgba(30,30,30,.3)', mutedSurface: 'rgba(30,30,30,.45)', input: 'rgba(30,30,30,.6)', inputBorder: 'rgba(255,255,255,.24)', borderSoft: 'rgba(255,255,255,.08)', text: '#E7E3FC', muted: '#BAB6C7', border: 'rgba(255,255,255,.12)', popup: '#24242D' },
  glass: { primary: '#8D51F9', accent: '#8D51F9', background: '#0B1322', surface: 'rgba(24,34,53,.72)', mutedSurface: 'rgba(255,255,255,.04)', input: 'rgba(11,19,34,.6)', inputBorder: 'rgba(255,255,255,.24)', borderSoft: 'rgba(255,255,255,.08)', text: '#F2F5FA', muted: '#B3BECE', border: 'rgba(255,255,255,.12)', popup: '#182235' },
};

export function resolveTheme(mode, prefersDark) {
  return mode === 'system' ? (prefersDark ? 'dark' : 'light') : (themePalettes[mode] ? mode : 'light');
}

export function moviePilotTheme(mode) {
  const { primary, accent, surface, background, text, muted, border, borderSoft, input, inputBorder, popup, mutedSurface } = themePalettes[mode] || themePalettes.light;
  const translucent = mode === 'transparent' || mode === 'glass';
  const semantic = mode === 'light'
    ? { Success: '#408F22', Warning: '#AB7600', Error: '#D93243', Info: '#1480B8' }
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
    Card: { color: surface, borderColor: borderSoft, borderRadius: '12px', paddingMedium: '24px', titleFontWeight: '600' },
    Button: { ...semanticButtons, borderRadiusMedium: '8px', borderRadiusSmall: '6px', fontWeight: '500', color: 'transparent', colorHover: mutedSurface, colorPressed: mutedSurface, textColor: text, colorPrimary: primary, colorHoverPrimary: primary, colorPressedPrimary: primary, colorFocusPrimary: primary, borderPrimary: `1px solid ${primary}`, borderHoverPrimary: `1px solid ${primary}`, borderPressedPrimary: `1px solid ${primary}`, borderFocusPrimary: `1px solid ${primary}`, border: `1px solid ${border}`, textColorPrimary: '#FFFFFF', textColorHoverPrimary: '#FFFFFF', textColorPressedPrimary: '#FFFFFF', textColorFocusPrimary: '#FFFFFF' },
    Input: { color: input, colorFocus: input, textColor: text, placeholderColor: muted, border: `1px solid ${inputBorder}`, borderHover: `1px solid ${accent}`, borderFocus: `1px solid ${accent}`, boxShadowFocus: '0 0 0 3px var(--accent-glow-color)', borderRadius: '8px' },
    Select: { peers: { InternalSelection: { color: input, colorActive: input, textColor: text, placeholderColor: muted, border: `1px solid ${inputBorder}`, borderHover: `1px solid ${accent}`, borderActive: `1px solid ${accent}`, borderFocus: `1px solid ${accent}` } } },
    Checkbox: { color: input, border: `1px solid ${inputBorder}` },
    Radio: { color: input, border: `1px solid ${inputBorder}` },
    Form: { labelTextColor: text, feedbackTextColor: muted },
    DataTable: { thColor: mutedSurface, thTextColor: text, tdTextColor: text, tdColor: translucent ? 'transparent' : surface, tdColorStriped: mutedSurface, borderColor: borderSoft, thFontWeight: '600' },
    Descriptions: { thColor: mutedSurface, tdColor: translucent ? 'transparent' : surface, borderColor: border, thTextColor: muted, tdTextColor: text },
    Menu: {
      color: 'transparent', itemHeight: '44px', borderRadius: '8px',
      itemTextColor: muted, itemIconColor: muted, itemTextColorHover: text,
      itemTextColorActive: '#FFFFFF', itemIconColorActive: '#FFFFFF',
      itemTextColorActiveHover: '#FFFFFF', itemIconColorActiveHover: '#FFFFFF',
      itemColorActive: primary,
      itemColorActiveHover: primary,
    },
  };
}
