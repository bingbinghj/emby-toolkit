export const musicSources = [{ title: 'MusicBrainz', value: 'musicbrainz' }, { title: 'TheAudioDB', value: 'theaudiodb' }, { title: '豆瓣音乐', value: 'doubanmusic' }];
export const sourceLabel = source => musicSources.find(item => item.value === source)?.title || '本地音乐';
export function formatMusicDuration(seconds) {
  if (!Number.isFinite(Number(seconds)) || Number(seconds) <= 0) return '';
  const total = Math.round(Number(seconds));
  const hours=Math.floor(total/3600),minutes=Math.floor(total/60)%60,secondsText=String(total%60).padStart(2,'0');
  return hours ? `${hours}:${String(minutes).padStart(2,'0')}:${secondsText}` : `${minutes}:${secondsText}`;
}
export const musicIdentity = music => ({ media_source: music.media_source, media_id: music.media_id, music_type: music.music_type || 'recording', ...(music.musicbrainz_release_id ? { musicbrainz_release_id: music.musicbrainz_release_id } : {}) });
