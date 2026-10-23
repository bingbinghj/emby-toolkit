const isExplicitZero = (value) => {
  if (typeof value === 'number') {
    return Number.isFinite(value) && value === 0;
  }
  if (typeof value !== 'string' || !value.trim()) {
    return false;
  }
  const numericValue = Number(value);
  return Number.isFinite(numericValue) && numericValue === 0;
};

export const isInvalidAudioProbeStream = (stream) => {
  if (stream?.Type !== 'Audio') return false;
  if (String(stream.Language ?? '').trim()) return false;
  if (String(stream.Title ?? '').trim()) return false;
  return isExplicitZero(stream.Channels) && isExplicitZero(stream.SampleRate);
};

export const updateMediaInfoStreamLanguage = (stream, language) => {
  if (!stream) return;
  stream.Language = language ?? null;
};

export const normalizeMediaInfoCodeToken = (value) => {
  const token = String(value || '').trim().toLowerCase().replaceAll('_', '-');
  // 与后端媒体兼容规则一致，让已保存的旧泰语映射也能匹配 tai。
  return ['tai', 'tai languages'].includes(token) ? 'tha' : token;
};

export const getMediaInfoLanguageTokens = (value) => {
  const token = normalizeMediaInfoCodeToken(value);
  if (!token) return [];
  const base = token.includes('-') ? token.split('-', 1)[0] : token;
  return [...new Set([token, base])];
};

export const getMediaInfoLanguageAliases = (item) => {
  const aliases = Array.isArray(item?.aliases)
    ? item.aliases
    : String(item?.aliases || '').split(',');
  return [item?.value, ...aliases, item?.label]
    .flatMap(getMediaInfoLanguageTokens)
    .filter(Boolean);
};

export const filterMediaInfoLanguageOption = (pattern, option) => {
  const query = normalizeMediaInfoCodeToken(pattern)
    .replaceAll('繁體', '繁体')
    .replaceAll('簡體', '简体');
  if (!query) return true;

  return [option?.label, option?.value, ...(option?._languageTokens || [])]
    .some(value => normalizeMediaInfoCodeToken(value)
      .replaceAll('繁體', '繁体')
      .replaceAll('簡體', '简体')
      .includes(query));
};

const CHINESE_LANGUAGE_SEMANTICS = {
  mandarin: {
    labels: new Set(['国语', '普通话', '中文']),
    label: '国语',
    value: 'chi',
    tokens: ['chi', 'zho', 'zh', 'cmn', 'zh-cmn', 'mandarin', 'guo', 'guoyu', '国语', '普通话', '中文'],
  },
  cantonese: {
    labels: new Set(['粤语', '广东话']),
    label: '粤语',
    value: 'yue',
    tokens: ['yue', 'zh-yue', 'yue-hk', 'yue-hant-hk', 'cantonese', '粤语', '广东话', '粤文', '粤配', '港配', '粤语配音'],
  },
  simplified: {
    labels: new Set(['简体', '简中', '简体中文']),
    label: '简体',
    value: 'chs',
    tokens: ['chs', 'zhs', 'sc', 'gb', 'zh-cn', 'zh-sg', 'zh-hans', 'zh-hans-cn', 'simplified', '简体', '简中', '简体中文'],
  },
  traditional: {
    labels: new Set(['繁体', '繁中', '繁体中文']),
    label: '繁体',
    value: 'cht',
    tokens: ['cht', 'zht', 'tc', 'big5', 'zh-tw', 'zh-hk', 'zh-mo', 'zh-hant', 'zh-hant-tw', 'zh-hant-hk', 'traditional', '繁体', '繁中', '繁体中文'],
  },
};

const CHINESE_SUBTITLE_GENERIC = {
  label: '中文',
  value: 'zho',
  tokens: ['chi', 'zho', 'zh', 'cmn', 'zh-cmn', 'chinese', '中文'],
};

const getChineseLanguageSemantic = (label) => Object.values(CHINESE_LANGUAGE_SEMANTICS)
  .find(item => item.labels.has(String(label || '').trim())) || null;

const normalizeExactTokens = values => [...new Set(values
  .map(normalizeMediaInfoCodeToken)
  .filter(Boolean))];

const LEGACY_AMBIGUOUS_CHINESE_TOKENS = new Set(['cn', 'hk', 'tw']);

const buildChineseLanguageOption = (semantic, item = {}) => {
  const rawTokens = [item?.value, ...(Array.isArray(item?.aliases)
    ? item.aliases
    : String(item?.aliases || '').split(',')), item?.label];
  const blockedTokens = new Set(Object.values(CHINESE_LANGUAGE_SEMANTICS)
    .filter(candidate => candidate !== semantic)
    .flatMap(candidate => normalizeExactTokens(candidate.tokens)));
  return {
    label: semantic.label,
    value: semantic.value,
    _languageTokens: normalizeExactTokens([
      ...semantic.tokens,
      ...rawTokens.filter((value) => {
        const token = normalizeMediaInfoCodeToken(value);
        return !blockedTokens.has(token) && !LEGACY_AMBIGUOUS_CHINESE_TOKENS.has(token);
      }),
    ]),
  };
};

const buildGenericChineseSubtitleOption = () => ({
  label: CHINESE_SUBTITLE_GENERIC.label,
  value: CHINESE_SUBTITLE_GENERIC.value,
  _languageTokens: normalizeExactTokens(CHINESE_SUBTITLE_GENERIC.tokens),
});

export const buildMediaInfoLanguageOptions = (rawData, subtitle = false) => {
  const options = [];
  const seenValues = new Set();

  for (const item of Array.isArray(rawData) ? rawData : []) {
    if (!item?.label) continue;
    const semantic = getChineseLanguageSemantic(item.label);
    if (subtitle && semantic === CHINESE_LANGUAGE_SEMANTICS.cantonese) {
      continue;
    }
    if (!subtitle && ['simplified', 'traditional'].some(key => CHINESE_LANGUAGE_SEMANTICS[key] === semantic)) {
      continue;
    }

    const aliases = Array.isArray(item.aliases)
      ? item.aliases
      : String(item.aliases || '').split(',').map(value => value.trim()).filter(Boolean);
    const option = subtitle && semantic === CHINESE_LANGUAGE_SEMANTICS.mandarin
      ? buildGenericChineseSubtitleOption()
      : semantic
        ? buildChineseLanguageOption(semantic, { ...item, aliases })
      : {
          label: item.label,
          value: aliases.find(Boolean) || item.value,
          _languageTokens: getMediaInfoLanguageAliases({ ...item, aliases }),
        };
    if (!option.value || seenValues.has(option.value)) continue;
    seenValues.add(option.value);
    options.push(option);
  }

  if (subtitle) {
    if (!seenValues.has(CHINESE_SUBTITLE_GENERIC.value)) {
      options.push(buildGenericChineseSubtitleOption());
      seenValues.add(CHINESE_SUBTITLE_GENERIC.value);
    }
    for (const key of ['simplified', 'traditional']) {
      const semantic = CHINESE_LANGUAGE_SEMANTICS[key];
      if (!seenValues.has(semantic.value)) {
        options.push(buildChineseLanguageOption(semantic));
        seenValues.add(semantic.value);
      }
    }
  }

  return options;
};

export const findMediaInfoLanguageOption = (options, language) => {
  const tokens = getMediaInfoLanguageTokens(language);
  if (tokens.length === 0) return null;
  const exactToken = tokens[0];
  return options.find(option => (option._languageTokens || []).includes(exactToken))
    || options.find(option => (option._languageTokens || []).some(token => tokens.includes(token)))
    || null;
};

const normalizeMediaInfoTitleToken = value => String(value || '')
  .trim()
  .toLowerCase()
  .replaceAll('繁體', '繁体')
  .replaceAll('簡體', '简体')
  .replaceAll('雙語', '双语');

export const detectChineseSubtitleScript = (title) => {
  const token = normalizeMediaInfoTitleToken(title);
  if (!token) return null;
  if (/繁|正体|正體|traditional\s+chinese|chinese\s+traditional|\bcht\b|zh[-_](?:tw|hk|mo|hant)/.test(token)) {
    return 'traditional';
  }
  if (/简|簡|simplified\s+chinese|chinese\s+simplified|\bchs\b|zh[-_](?:cn|sg|hans)/.test(token)) {
    return 'simplified';
  }
  return null;
};

export const inferChineseSubtitleLanguageFromTitle = (title) => {
  const normalizedTitle = normalizeMediaInfoTitleToken(title);
  if (/^(简繁|繁简)/.test(normalizedTitle)) return null;
  const hasSimplifiedMarker = /简体|简中|中简/.test(normalizedTitle);
  const hasTraditionalMarker = /繁体|繁中|中繁/.test(normalizedTitle);
  if (hasSimplifiedMarker && hasTraditionalMarker) return null;
  if (hasSimplifiedMarker !== hasTraditionalMarker) {
    return hasSimplifiedMarker ? 'chs' : 'cht';
  }

  const hasCompactDualLanguage = /^(简|繁)(?!体|中).+?(双语|雙語)/.test(normalizedTitle)
    || /^(简|繁)(?!体|中)[英日韩西德法葡俄泰越意]$/.test(normalizedTitle);
  if (!hasCompactDualLanguage) {
    return null;
  }
  const script = detectChineseSubtitleScript(normalizedTitle);
  return script === 'simplified' ? 'chs' : script === 'traditional' ? 'cht' : null;
};

const CHINESE_SUBTITLE_LANGUAGE_TOKENS = new Set([
  'chi', 'zho', 'zh', 'chs', 'cht', 'cmn', 'zh-cmn', 'yue', 'zh-yue', 'chinese',
]);

export const isChineseSubtitleStream = (stream, selectedLanguage, optionLabel) => {
  if (stream?.Type !== 'Subtitle') return false;
  if (detectChineseSubtitleScript(stream.Title)) return true;
  if (['中文', '简体', '繁体', '国语', '粤语'].includes(String(optionLabel || '').trim())) {
    return true;
  }
  return getMediaInfoLanguageTokens(selectedLanguage || stream.Language).some(token => (
    CHINESE_SUBTITLE_LANGUAGE_TOKENS.has(token)
    || token.startsWith('zh-')
    || token.startsWith('yue-')
  ));
};

export const resolveMediaInfoLanguageSelection = (
  options,
  stream,
  defaultChineseSubtitleLanguage = 'chs',
) => {
  if (!stream) return null;
  const languageOption = findMediaInfoLanguageOption(options, stream.Language);
  if (stream.Type !== 'Subtitle') {
    return languageOption?.value ?? stream.Language;
  }

  const languageToken = normalizeMediaInfoCodeToken(stream.Language);
  const isChineseLanguage = [
    'chi', 'zho', 'zh', 'chs', 'cht', 'yue', 'cmn', 'chinese',
  ].includes(languageToken) || languageToken.startsWith('zh-') || languageToken.startsWith('yue-');
  if (!isChineseLanguage) {
    return languageOption?.value ?? stream.Language;
  }

  if (['chs', 'cht'].includes(languageOption?.value)) {
    return languageOption?.value ?? stream.Language;
  }
  if (languageToken === 'yue' || languageToken.startsWith('yue-') || languageToken === 'zh-yue') {
    return options.find(option => option.label === '繁体')?.value
      ?? languageOption?.value
      ?? stream.Language;
  }
  // 默认配置仅决定编辑器的初始选项，不根据标题推断或修改原始 Language。
  if (languageOption?.value === CHINESE_SUBTITLE_GENERIC.value
    || ['chi', 'zho', 'zh', 'cmn', 'zh-cmn', 'chinese'].includes(languageToken)) {
    const configuredValue = defaultChineseSubtitleLanguage === CHINESE_SUBTITLE_GENERIC.value
      ? CHINESE_SUBTITLE_GENERIC.value
      : CHINESE_LANGUAGE_SEMANTICS.simplified.value;
    return options.find(option => option.value === configuredValue)?.value
      ?? languageOption?.value
      ?? stream.Language;
  }
  return languageOption?.value ?? stream.Language;
};

export const getMediaInfoLanguageTitle = (streamType, optionLabel, language) => {
  const label = String(optionLabel || '').trim();
  const unknown = new Set(['', 'und', 'unk', 'xx', 'zxx', 'unknown', 'undefined', 'none', 'null',
    'undetermined', 'no linguistic content', '未知', '无语言', '无语言内容']);
  if (unknown.has(label.toLowerCase())
    || (language !== undefined && unknown.has(normalizeMediaInfoCodeToken(language)))) return '';
  if (!label || streamType !== 'Subtitle') return label;
  if (['国语', '粤语', '简体', '繁体'].includes(label)) return label;
  return label.endsWith('语') ? `${label.slice(0, -1)}文` : label;
};
