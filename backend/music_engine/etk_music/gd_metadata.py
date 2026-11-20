"""GD 音乐台资料渠道；与平台直连来源使用不同标识和请求缓存。"""
import re
from .cache import Cache
from .lyrics_search import GD_SOURCES, _gd


def search(source, query, kind="recording", page=1, count=20):
    if source not in GD_SOURCES or kind != "recording":
        raise ValueError("GD 音乐台提供歌曲资料搜索，请选择单曲")
    if not isinstance(query, str) or not query.strip() or len(query) > 200:
        raise ValueError("请输入不超过 200 字的搜索词")
    page, count = max(1, min(int(page), 100)), max(1, min(int(count), 30))
    channel = GD_SOURCES[source]
    rows = _gd({"types":"search", "source":channel, "name":query.strip(), "count":count, "pages":page})
    items = []
    cache = Cache(1000, 1800, "gd-metadata-selections")
    for row in rows:
        if row.get("id") is None or row.get("source", channel) != channel:
            continue
        artists = row.get("artist") or []
        if not isinstance(artists, list):
            artists = [str(artists)]
        item = {"media_source":source, "media_id":str(row["id"]), "music_type":"recording", "type":"音乐",
                "title":row.get("name") or "", "artists":artists, "artist":" / ".join(artists),
                "album":row.get("album") or "", "picture_id":row.get("pic_id")}
        cache.set(f"{source}:{item['media_id']}", item)
        items.append(item)
    return {"items":items, "page":page, "has_more":len(rows)>=count}


def detail(source, identity, kind="recording"):
    if source not in GD_SOURCES or kind != "recording" or not re.fullmatch(r"[\w=-]{1,200}", str(identity), re.ASCII):
        raise ValueError("GD 音乐资料标识无效")
    item = Cache(1000, 1800, "gd-metadata-selections").get(f"{source}:{identity}")
    if not item:
        raise ValueError("GD 音乐台的所选资料已过期，请按歌曲名称重新搜索")
    item = dict(item)
    if item.get("picture_id"):
        data = _gd({"types":"pic", "source":GD_SOURCES[source], "id":str(item["picture_id"]), "size":500})
        item["cover_url"] = data["url"]
    return item
