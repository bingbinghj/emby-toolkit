export const musicSources = [{ title: 'MusicBrainz', value: 'musicbrainz' }, { title: 'TheAudioDB', value: 'theaudiodb' }, { title: '豆瓣音乐', value: 'doubanmusic' }];
export const streamingSources = [{title:'网易云音乐',value:'netease'},{title:'QQ 音乐',value:'qqmusic'},{title:'Apple Music',value:'apple'},{title:'酷狗音乐',value:'kugou'},{title:'酷我音乐',value:'kuwo'}];
export const gdSources = [{title:'网易云音乐 · GD 音乐台',value:'gd_netease'},{title:'JOOX · GD 音乐台',value:'gd_joox'},{title:'Bilibili · GD 音乐台',value:'gd_bilibili'}];
export const metadataSources = [...musicSources, ...streamingSources, ...gdSources];
export const lyricSources = [{title:'网易云音乐',value:'netease'},{title:'QQ 音乐',value:'qqmusic'},{title:'酷狗音乐',value:'kugou'},{title:'酷我音乐',value:'kuwo'},...gdSources,{title:'LRCLIB',value:'lrclib'}];
export const sourceLabel = source => [...musicSources,...streamingSources,...lyricSources].find(item => item.value === source)?.title || '本地音乐';
export function musicSearchTerm(item = {}, kind = 'recording') {
  if (kind === 'album') {
    const album = item.album || '';
    const year = String(item.release_year || item.year || item.release_date || '').match(/^\d{4}/)?.[0];
    return album && year && !new RegExp(`[（(]${year}[）)]\\s*$`).test(album) ? `${album} (${year})` : album;
  }
  const artist = item.artists?.filter(Boolean).join(' / ') || item.album_artist || '';
  if (kind === 'artist') return artist;
  return item.title || '';
}
export function formatMusicDuration(seconds) {
  if (!Number.isFinite(Number(seconds)) || Number(seconds) <= 0) return '';
  const total = Math.round(Number(seconds));
  const hours=Math.floor(total/3600),minutes=Math.floor(total/60)%60,secondsText=String(total%60).padStart(2,'0');
  return hours ? `${hours}:${String(minutes).padStart(2,'0')}:${secondsText}` : `${minutes}:${secondsText}`;
}
export const musicIdentity = music => ({ media_source: music.media_source, media_id: music.media_id, music_type: music.music_type || 'recording', ...(music.musicbrainz_release_id ? { musicbrainz_release_id: music.musicbrainz_release_id } : {}) });

export function musicReleaseOptions(album = {}) {
  const items=(album.releases || []).filter(item=>item.media_id).map(item=>({value:item.media_id,title:[item.title,item.date,item.country,item.formats?.join('/'),item.track_count ? `${item.track_count} 首曲目` : ''].filter(Boolean).join(' · ')}));
  if(album.musicbrainz_release_id && !items.some(item=>item.value===album.musicbrainz_release_id))items.unshift({value:album.musicbrainz_release_id,title:[album.title,album.release_date,album.country,album.total_tracks ? `${album.total_tracks} 首曲目` : ''].filter(Boolean).join(' · ')});
  return items;
}

export function musicFieldOptions(field, metadata = {}, album = null) {
  const values=[metadata?.[field]];
  if(field==='isrc')values.push(metadata?.raw_data?.isrcs);
  if(field==='performer_role')values.push(Object.keys(metadata?.performers || {}));
  if(album){
    if(field==='album')values.push(album.title);
    else if(field==='album_artist')values.push(album.album_artist || album.artist);
    else if(['total_tracks','total_discs','release_year','original_year'].includes(field))values.push(album[field]);
  }
  return [...new Set(values.flat().filter(value=>value!==undefined && value!==null && value!=='' && (typeof value==='string' || typeof value==='number')))];
}

export function musicReleaseError(selection) {
  const release=selection?.musicbrainz_release_id?.trim();
  if (!release) return '';
  if (selection.media_source!=='musicbrainz' || selection.music_type!=='album') return '具体发行版本仅用于 MusicBrainz 专辑';
  if (!/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(release)) return '具体发行版本 ID 必须是有效 UUID';
  if (!selection.media_id?.trim()) return '请先选择所属专辑组';
  return '';
}
