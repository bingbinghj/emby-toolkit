// MoviePilot-Frontend v3.2.13 AccountSettingSystem (MIT).
export const musicReleaseRegions = [
  ['中国大陆', 'CN'], ['中国台湾', 'TW'], ['中国香港', 'HK'], ['全球', 'XW'],
  ['日本', 'JP'], ['美国', 'US'], ['英国', 'GB'], ['韩国', 'KR'],
].map(([title, value]) => ({ title, value }));

export const musicReleaseScripts = [
  ['简体中文', 'Hans'], ['繁体中文', 'Hant'], ['拉丁文字', 'Latn'], ['日文', 'Jpan'], ['韩文', 'Kore'],
].map(([title, value]) => ({ title, value }));

export const musicTrackMatchItems = [
  { title: '严格校验（默认）', value: 'strict' },
  { title: '严格校验失败时回落忽略时长', value: 'fallback_ignore_duration' },
  { title: '忽略时长校验', value: 'ignore_duration' },
];
