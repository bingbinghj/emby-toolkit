"""手动歌词候选搜索与读取；应用由既有编辑预览和原子保存流程完成。"""
import re
import time
from .cache import cached, connection
from .lrclib import LrclibModule
from .streaming import read_json

GD_API = "https://music-api.gdstudio.xyz/api.php"
GD_SOURCES = {"netease", "joox", "bilibili"}
ALBUM_SOURCES = {"netease", "qqmusic", "kuwo"}


def _reserve_gd():
    # GD 公布每 5 分钟最多 50 次。多个 worker 共用有界计数，不排队等待。
    with connection() as db:
        db.execute("CREATE TABLE IF NOT EXISTS lyrics_requests(at REAL)")
        db.commit()
        db.execute("BEGIN IMMEDIATE")
        db.execute("DELETE FROM lyrics_requests WHERE at<=?", (time.time()-300,))
        if db.execute("SELECT COUNT(*) FROM lyrics_requests").fetchone()[0] >= 45:
            raise ValueError("GD 音乐台请求较频繁，请稍后再试或选择 LRCLIB")
        db.execute("INSERT INTO lyrics_requests VALUES(?)", (time.time(),))


@cached(maxsize=128, ttl=600, region="lyrics-manual-gd")
def _gd(params):
    _reserve_gd()
    data = read_json(GD_API, params, "GD 音乐台")
    valid = isinstance(data, list) if params["types"] == "search" else isinstance(data, dict) and any(key in data for key in ("lyric", "tlyric"))
    if not valid:
        raise ValueError("GD 音乐台暂时未返回有效结果，请稍后再试或选择其他来源")
    return data


def _source(body):
    source = body.get("source") or "netease"
    if source not in GD_SOURCES | {"lrclib", "qqmusic", "kuwo"}:
        raise ValueError("请选择有效的歌词来源")
    return source


def search(body):
    source = _source(body)
    query = body.get("query")
    if not isinstance(query, str) or not query.strip() or len(query) > 200:
        raise ValueError("请输入不超过 200 字的歌曲或歌手名称")
    page = max(1, min(int(body.get("page", 1)), 100))
    if body.get("kind") == "album" or body.get("album_id") or source in {"qqmusic", "kuwo"}:
        from . import streaming_metadata
        if body.get("kind") == "album" and source not in ALBUM_SOURCES:
            raise ValueError("当前来源不提供专辑搜索，请选择网易云音乐、QQ 音乐或酷我音乐")
        if body.get("album_id"):
            album = streaming_metadata.detail(source, str(body["album_id"]), "album")
            rows = album["tracks"]
            result = {"items":rows[(page-1)*20:page*20],"has_more":page*20<len(rows),"page":page}
        else:
            result = streaming_metadata.search(source, query, body.get("kind") or "recording", page)
        result["items"] = [{**row,"id":row["media_id"],"source":source} for row in result["items"]]
        return result
    if source == "lrclib":
        rows = LrclibModule()._request_json("/api/search", params={"q": query.strip()})
        if not isinstance(rows, list):
            raise ValueError("LRCLIB 暂时不可用，请稍后再试")
        available = [row for row in rows if row.get("syncedLyrics") or row.get("plainLyrics")]
        items = [{"id": str(row["id"]), "source": source, "title": row.get("trackName"), "artists": [row.get("artistName", "")],
                  "album": row.get("albumName"), "duration": row.get("duration"), "synced": bool(row.get("syncedLyrics"))}
                 for row in available[(page-1)*20:page*20]]
        return {"items": items, "page": page, "has_more": page*20 < len(available)}
    rows = _gd({"types": "search", "source": source, "name": query.strip(), "count": 20, "pages": page})
    if not isinstance(rows, list):
        raise ValueError("GD 音乐台暂时未返回搜索结果，请稍后再试或选择其他来源")
    items = [{"id": str(row.get("lyric_id") or row["id"]), "source": source, "title": row.get("name"),
              "artists": row.get("artist") if isinstance(row.get("artist"), list) else [str(row.get("artist") or "")],
              "album": row.get("album"), "duration": row.get("duration")}
             for row in rows if row.get("id") is not None and row.get("source", source) == source]
    return {"items": items, "page": page, "has_more": len(rows) >= 20}


def detail(body):
    source, identity = _source(body), body.get("id")
    if not isinstance(identity, str) or not re.fullmatch(r"[\w=-]{1,200}", identity, re.ASCII):
        raise ValueError("歌词 ID 无效")
    if source == "qqmusic":
        from .streaming_metadata import get
        data = get("https://c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg", {"format":"json","songmid":identity,"nobase64":1})
        variants = [("original","原文",data.get("lyric")),("translation","译文",data.get("trans"))]
    elif source == "kuwo":
        data = read_json("https://m.kuwo.cn/newh5/singles/songinfoandlrc", {"musicId":identity}, "酷我音乐")
        rows = (data.get("data") or {}).get("lrclist") or []
        lines=[]
        for row in rows:
            seconds=float(row.get("time") or 0)
            lines.append(f"[{int(seconds//60):02d}:{seconds%60:05.2f}]{row.get('lineLyric') or ''}")
        variants = [("original","原文","\n".join(lines))]
    elif source == "lrclib":
        if not identity.isdigit():
            raise ValueError("LRCLIB 歌词 ID 无效")
        data = LrclibModule()._request_json(f"/api/get/{identity}")
        variants = [("synced", "同步歌词", (data or {}).get("syncedLyrics")), ("plain", "纯文本", (data or {}).get("plainLyrics"))]
    else:
        data = _gd({"types": "lyric", "source": source, "id": identity})
        if not isinstance(data, dict):
            raise ValueError("此歌曲没有返回可用歌词")
        variants = [("original", "原文", data.get("lyric")), ("translation", "译文", data.get("tlyric"))]
    items = [{"value": key, "title": title, "text": text} for key, title, text in variants
             if isinstance(text, str) and text.strip() and len(text.encode("utf-8")) <= 1024*1024]
    if not items:
        raise ValueError("此歌曲没有可用歌词，请选择其他版本")
    return {"variants": items, "source": source, "id": identity}
