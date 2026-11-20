"""公开音乐榜单：沿用 MP 探索的来源、筛选、分页和卡片数据，不获取音频。"""
import json
from datetime import datetime, timezone
from urllib.parse import urlsplit
from .cache import cached
from .http import RequestUtils
from .runtime import request_budget

SOURCES = {"netease": "网易云音乐", "qqmusic": "QQ 音乐", "apple": "Apple Music", "kugou": "酷狗音乐", "kuwo": "酷我音乐"}
REGIONS = {"cn": "中国大陆", "hk": "中国香港", "tw": "中国台湾", "us": "美国", "jp": "日本", "kr": "韩国", "gb": "英国"}


def read_json(url, params=None, label="音乐来源"):
    host = urlsplit(url).netloc
    referer = "https://y.qq.com/" if host.endswith("y.qq.com") else f"https://{host}/"
    headers = {} if host in {"rss.marketingtools.apple.com", "itunes.apple.com"} else {"Referer": referer}
    client = RequestUtils(ua="Mozilla/5.0", headers=headers, timeout=18)
    try:
        response = client.get_res(url, params=params, stream=True)
        if response is None:
            raise ValueError(f"{label}暂时无法连接，请稍后重试")
        with response:
            if response.status_code != 200:
                raise ValueError(f"{label}请求失败（HTTP {response.status_code}），请稍后重试")
            content = bytearray()
            for chunk in response.iter_content(65536):
                content.extend(chunk)
                if len(content) > 4*1024*1024:
                    raise ValueError(f"{label}响应过大")
            try:
                return json.loads(content)
            except (ValueError, UnicodeDecodeError):
                raise ValueError(f"{label}返回了无效数据，请稍后重试") from None
    finally:
        client.close()


@cached(maxsize=128, ttl=1800, region="streaming-charts", skip_empty=True)
def _get(url, params=None):
    data = read_json(url, params)
    if not isinstance(data, dict) or data.get("code") not in (None, 0, 200):
        raise ValueError("音乐榜单暂时不可用，请稍后重试")
    if isinstance(data.get("req_1"), dict) and data["req_1"].get("code") != 0:
        raise ValueError("QQ 音乐榜单暂时不可用，请稍后重试")
    return data


def _qq(method, params):
    data = _get("https://u.y.qq.com/cgi-bin/musicu.fcg", {"data": json.dumps({"comm": {"ct": 24, "cv": 0}, "req_1": {"module": "musicToplist.ToplistInfoServer", "method": method, "param": params}})})
    response = data.get("req_1") or {}
    if data.get("code") != 0 or response.get("code") != 0:
        raise ValueError("QQ 音乐榜单暂时不可用")
    return response["data"]


def _children(value):
    for child in value.get("child", []):
        if child.get("sourceid") and not child.get("child"):
            yield child
        yield from _children(child)


def charts(source):
    if source == "netease":
        data = _get("https://music.163.com/api/toplist")
        return [{"value": str(row["id"]), "title": row["name"]} for row in data.get("list", [])]
    if source == "qqmusic":
        return [{"value": str(row["topId"]), "title": row["title"], "period": row["period"]}
                for group in _qq("GetAll", {}).get("group", []) for row in group.get("toplist", [])]
    if source == "kugou":
        data = _get("https://m.kugou.com/rank/list", {"json": "true"})
        return [{"value": str(row["rankid"]), "title": row["rankname"]} for row in data.get("rank", {}).get("list", [])]
    if source == "kuwo":
        data = _get("https://wapi.kuwo.cn/api/pc/bang/list")
        return [{"value": str(row["sourceid"]), "title": row["name"]} for row in _children(data)]
    return [{"value": "most-played", "title": "热门歌曲"}]


def _url(value):
    text = str(value or "").replace("{size}", "400")
    return "https://"+text[7:] if text.startswith("http://") else text if text.startswith("https://") else ""


def _date(value):
    return datetime.fromtimestamp(value/1000, timezone.utc).strftime("%Y-%m-%d") if isinstance(value, (int, float)) and value > 0 else ""


def track(source, row):
    item = {"media_source": source, "music_type": "recording"}
    if source == "netease":
        album = row.get("album") or row.get("al") or {}
        item.update(media_id=str(row["id"]), title=row["name"], artists=[a["name"] for a in row.get("artists", row.get("ar", []))],
                    album=album.get("name"), cover_url=_url(album.get("picUrl")), duration=(row.get("duration", row.get("dt", 0)) or 0)/1000,
                    release_date=_date(album.get("publishTime")), detail_link=f"https://music.163.com/#/song?id={row['id']}")
    elif source == "qqmusic":
        album = row.get("album") or {}
        cover_id = album.get("pmid") or album.get("mid")
        item.update(media_id=str(row["id"]), title=row.get("title") or row["name"], artists=[a["name"] for a in row.get("singer", [])],
                    album=album.get("title") or album.get("name"), duration=row.get("interval"), release_date=row.get("time_public"),
                    cover_url=f"https://y.gtimg.cn/music/photo_new/T002R300x300M000{cover_id}.jpg" if cover_id else "",
                    overview=row.get("subtitle"), detail_link=f"https://y.qq.com/n/ryqq/songDetail/{row['mid']}")
    elif source == "apple":
        item.update(media_id=str(row["id"]), title=row["name"], artists=[row["artistName"]], release_date=row.get("releaseDate"),
                    cover_url=_url(row.get("artworkUrl100")).replace("100x100bb", "400x400bb"), detail_link=_url(row.get("url")),
                    genres=[g["name"] for g in row.get("genres", []) if g.get("genreId") != "34"])
    elif source == "kugou":
        item.update(media_id=str(row["hash"]), title=row.get("songname") or row.get("filename"),
                    artists=[a["author_name"] for a in row.get("authors", [])], album=row.get("album_name"), duration=row.get("duration"),
                    cover_url=_url(row.get("album_sizable_cover") or row.get("trans_param", {}).get("union_cover")),
                    detail_link=f"https://www.kugou.com/song/#hash={row['hash']}")
    else:
        item.update(media_id=str(row["id"]), title=row["name"], artists=[row["artist"]] if row.get("artist") else [],
                    album=row.get("album"), duration=int(row.get("song_duration") or 0),
                    cover_url=_url(row.get("pic")), detail_link=f"https://www.kuwo.cn/play_detail/{row['id']}")
    return item


def explore(body):
    source = body.get("source")
    if source not in SOURCES:
        raise ValueError("音乐探索来源无效")
    page = max(1, min(int(body.get("page", 1)), 1000))
    # 酷狗网页接口固定每页 30 项，其他来源统一使用同一页大小。
    size, region = 30, body.get("region") or "cn"
    if region not in REGIONS:
        raise ValueError("Apple Music 地区无效")
    with request_budget(30):
        options = charts(source)
        if not options:
            raise ValueError(f"{SOURCES[source]}没有返回可用榜单")
        identity = str(body.get("chart") or options[0]["value"])
        selected = next((row for row in options if row["value"] == identity), None)
        if not selected:
            raise ValueError("此榜单已不可用，请重新选择")
        start = (page-1)*size
        if source == "netease":
            data = _get("https://music.163.com/api/playlist/detail", {"id": identity})
            playlist = data.get("result") or data.get("playlist") or {}
            if not isinstance(playlist.get("tracks"), list):
                raise ValueError("网易云音乐榜单暂时不可用")
            all_rows = playlist["tracks"]
            rows, total = all_rows[start:start+size], len(all_rows)
        elif source == "qqmusic":
            data = _qq("GetDetail", {"topid": int(identity), "period": selected["period"], "offset": start, "num": size})
            rows, total = data.get("songInfoList", []), int(data.get("data", {}).get("totalNum", 0))
        elif source == "apple":
            data = _get(f"https://rss.marketingtools.apple.com/api/v2/{region}/music/most-played/100/songs.json")
            if not isinstance(data.get("feed", {}).get("results"), list):
                raise ValueError("Apple Music 榜单暂时不可用")
            all_rows = data["feed"]["results"]
            rows, total = all_rows[start:start+size], len(all_rows)
        elif source == "kugou":
            data = _get("https://m.kugou.com/rank/info/", {"rankid": identity, "page": page, "json": "true"})
            songs = data.get("songs", {})
            rows, total = songs.get("list", []), int(songs.get("total", 0))
        else:
            data = _get("https://kbangserver.kuwo.cn/ksong.s", {"from": "pc", "fmt": "json", "type": "bang", "data": "content", "id": identity, "pn": page-1, "rn": size, "show_copyright_off": 0})
            rows, total = data.get("musiclist", []), int(data.get("num", 0))
        return {"items": [dict(track(source, row), chart_rank=start+index+1) for index, row in enumerate(rows)],
                "total": total, "page": page, "page_size": size, "has_more": start+len(rows) < total and bool(rows),
                "charts": options, "chart": identity, "region": region}
