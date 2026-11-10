export const userPermissionGroups = [
  { title: '访问权限', icon: 'mdi-account-key-outline', fields: [
    ['EnableRemoteAccess', '允许远程连接'],
    ['EnableUserPreferenceAccess', '允许修改个人设置'],
    ['IsHidden', '在登录界面隐藏此用户'],
    ['IsHiddenRemotely', '在远程登录界面隐藏此用户'],
    ['IsHiddenFromUnusedDevices', '在未使用的设备上隐藏此用户'],
  ] },
  { title: '媒体播放', icon: 'mdi-play-circle-outline', fields: [
    ['EnableMediaPlayback', '允许媒体播放'],
    ['EnableAudioPlaybackTranscoding', '允许音频转码'],
    ['EnableVideoPlaybackTranscoding', '允许视频转码'],
    ['EnablePlaybackRemuxing', '允许重新封装媒体'],
    ['EnableRemoteControlOfOtherUsers', '允许远程控制其他用户'],
    ['EnableSharedDeviceControl', '允许远程控制共享设备'],
    ['EnableLiveTvAccess', '允许访问直播电视'],
    ['EnableLiveTvManagement', '允许管理直播电视'],
  ] },
  { title: '媒体管理', icon: 'mdi-folder-cog-outline', fields: [
    ['EnableContentDownloading', '允许媒体下载'],
    ['EnableSubtitleDownloading', '允许字幕下载'],
    ['EnableSubtitleManagement', '允许字幕管理'],
    ['EnableContentDeletion', '允许媒体删除'],
    ['EnableMediaConversion', '允许媒体转换'],
    ['EnablePublicSharing', '允许公开分享'],
  ] },
];

export const userPlaybackFields = [
  ['PlayDefaultAudioTrack', '播放默认音轨'],
  ['RememberAudioSelections', '记住音轨选择'],
  ['RememberSubtitleSelections', '记住字幕选择'],
  ['EnableNextEpisodeAutoPlay', '自动播放下一集'],
  ['HidePlayedInLatest', '在最新入库中隐藏已播放的媒体'],
  ['HidePlayedInMoreLikeThis', '在相似推荐中隐藏已播放的媒体'],
  ['HidePlayedInSuggestions', '在建议中隐藏已播放的媒体'],
  ['DisplayMissingEpisodes', '显示缺失的剧集'],
];

export const subtitleModes = [
  { title: '智能', value: 'Smart' },
  { title: '自动', value: 'Default' },
  { title: '总是播放字幕', value: 'Always' },
  { title: '只播放强制字幕', value: 'OnlyForced' },
  { title: '不播放字幕', value: 'None' },
];

export function changedUserFields(current = {}, original = {}) {
  return Object.fromEntries(Object.entries(current).filter(([key, value]) =>
    Object.hasOwn(original, key) && JSON.stringify(value) !== JSON.stringify(original[key])));
}

export function userFieldsMatch(expected = {}, saved = {}) {
  return Object.entries(expected).every(([key, value]) => JSON.stringify(value) === JSON.stringify(saved[key]));
}

export function validAvatar(file) {
  return ['image/jpeg', 'image/png', 'image/gif', 'image/webp'].includes(file?.type)
    && file.size > 0 && file.size <= 800 * 1024;
}
