"""没有专辑接口的资料源按自身搜索结果归并；不冒充完整发行曲目。"""
import hashlib
import json
from .cache import Cache


def group(source, tracks):
    cache = Cache(200, 1800, "music-search-albums")
    albums = {}
    for track in tracks:
        title = str(track.get("album") or "").strip()
        if not title:
            continue
        artists = track.get("artists") or []
        identity = "album" + hashlib.sha256(json.dumps([title, sorted(artists)], ensure_ascii=False).encode()).hexdigest()
        if identity not in albums:
            previous = cache.get(f"{source}:{identity}")
            albums[identity] = previous or {"id":identity, "media_id":identity, "source":source, "media_source":source,
                "music_type":"album", "type":"音乐", "title":title, "album":title, "artists":artists,
                "artist":" / ".join(artists), "tracks":[], "track_scope":"search_results"}
        album = albums[identity]
        known = {row.get("media_id") or row.get("id") for row in album["tracks"]}
        if (track.get("media_id") or track.get("id")) not in known:
            album["tracks"].append(track)
    for album in albums.values():
        cache.set(f"{source}:{album['id']}", album)
    return [{key:value for key,value in album.items() if key != "tracks"} for album in albums.values()]


def detail(source, identity):
    album = Cache(200, 1800, "music-search-albums").get(f"{source}:{identity}")
    if not album:
        raise ValueError("所选专辑搜索结果已过期，请重新搜索")
    return album
