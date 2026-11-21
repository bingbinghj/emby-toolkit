"""手动标签资料搜索：公开歌曲、专辑数据复用榜单 HTTP 和缓存，不获取音频。"""
import hashlib
import html
import json
import re
import time
from .cache import Cache, cached
from .streaming import SOURCES, read_json, track, _url, _date

ALBUM_SOURCES = {"netease", "qqmusic", "apple", "kuwo", "kugou"}


@cached(maxsize=256, ttl=1800, region="streaming-metadata")
def get(url, params=None):
    data = read_json(url, params, "音乐资料源")
    if not isinstance(data, dict) or data.get("code") not in (None, 0, 200) or data.get("subcode", 0) != 0 or data.get("error_code", 0) != 0 or data.get("errcode", 0) != 0:
        raise ValueError("音乐资料源暂时不可用，请稍后重试")
    if isinstance(data.get("req_1"), dict) and data["req_1"].get("code") != 0:
        raise ValueError("QQ 音乐资料暂时不可用，请稍后重试")
    # 不把限流页或缺少业务数据的响应缓存成成功结果。
    shapes = [
        ("search/get/web", ("result",)), ("/api/v1/album/", ("album",)),
        ("/api/song/detail/", ("songs",)), ("itunes.apple.com", ("results",)),
        ("musicu.fcg", ("req_1",)), ("query_lyric", ("lyric", "trans")),
        ("y.qq.com", ("data",)), ("complexsearch.kugou.com", ("data",)),
        ("search.kuwo.cn", ("abslist", "albumlist", "musiclist")),
        ("getSongInfo.php", ("songName",)),
        ("mobilecdn.kugou.com", ("data",)),
    ]
    for segment, keys in shapes:
        if segment in url:
            if not any(key in data for key in keys):
                raise ValueError("音乐资料源返回了无效数据，请稍后重试")
            break
    return data


def clean(value):
    return html.unescape(re.sub(r"<[^>]*>", "", str(value or "")))


def remember(item):
    """短期保存搜索到的公开标签，详情接口缺失的平台按同一 ID 取回。"""
    item.update(type="音乐", artist=" / ".join(item.get("artists") or []))
    date = item.get("release_date") or ""
    if re.match(r"^\d{4}", date):
        item.update(release_year=int(date[:4]), year=date[:4])
    Cache(1000, 1800, "streaming-selections").set(f"{item['media_source']}:{item['music_type']}:{item['media_id']}", item)
    return item


def qq_track(row):
    if "songid" in row:
        row = {**row, "id":row["songid"], "mid":row["songmid"], "name":row["songname"],
               "album":{"id":row.get("albumid"), "mid":row.get("albummid"), "name":row.get("albumname")},
               "time_public":_date(row.get("pubtime", 0)*1000)}
    item = track("qqmusic", row)
    item.update(media_id=row["mid"], album_id=str(row.get("album", {}).get("mid") or ""), track_number=row.get("index_album") or row.get("belongCD"))
    return remember(item)


def apple_track(row):
    return remember({"media_source":"apple", "media_id":str(row["trackId"]), "music_type":"recording",
        "title":row["trackName"], "artists":[row["artistName"]], "album":row.get("collectionName"), "album_id":str(row.get("collectionId") or ""),
        "cover_url":_url(row.get("artworkUrl100")).replace("100x100bb", "600x600bb"), "release_date":row.get("releaseDate"),
        "track_number":row.get("trackNumber"), "total_tracks":row.get("trackCount"), "disc_number":row.get("discNumber"), "total_discs":row.get("discCount"),
        "duration":(row.get("trackTimeMillis") or 0)/1000})


def netease_track(row):
    item = track("netease", row)
    album = row.get("album") or row.get("al") or {}
    item.update(album_id=str(album.get("id") or ""), track_number=row.get("no") or row.get("position"), total_tracks=album.get("size"))
    return remember(item)


def kuwo_track(row):
    pic = row.get("pic120") or row.get("hts_MVPIC") or row.get("web_albumpic_short") or ""
    if pic and not pic.startswith("http"):
        pic = "https://img2.kuwo.cn/star/albumcover/"+pic
    return remember({"media_source":"kuwo", "media_id":str(row.get("MUSICRID") or row.get("id")).removeprefix("MUSIC_"), "music_type":"recording",
        "title":clean(row.get("SONGNAME") or row.get("NAME") or row.get("name")), "artists":[clean(row.get("ARTIST") or row.get("artist"))],
        "album":clean(row.get("ALBUM") or row.get("album")), "album_id":str(row.get("ALBUMID") or row.get("albumId") or ""),
        "cover_url":_url(pic), "duration":int(row.get("DURATION") or row.get("duration") or 0),
        "release_date":row.get("RELEASEDATE") or row.get("releasedate") or "", "track_number":int(row.get("track") or 0) or None})


def search(source, query, kind="recording", page=1, count=20):
    if source not in SOURCES or kind not in {"recording", "album"}:
        raise ValueError("音乐资料来源或类型无效")
    if not isinstance(query, str) or not query.strip() or len(query)>200:
        raise ValueError("请输入不超过 200 字的歌曲、专辑或歌手名称")
    page, count = max(1, min(int(page), 100)), max(1, min(int(count), 50))
    offset, query = (page-1)*count, query.strip()
    if kind == "album" and source not in ALBUM_SOURCES:
        raise ValueError("此来源仅支持歌曲资料搜索")
    if source == "netease":
        result = get("https://music.163.com/api/search/get/web", {"s":query,"type":10 if kind=="album" else 1,"limit":count,"offset":offset}).get("result")
        if not isinstance(result, dict): raise ValueError("网易云音乐未返回有效搜索结果")
        rows, total = result.get("albums" if kind=="album" else "songs", []), result.get("albumCount" if kind=="album" else "songCount", 0)
        items = [album_item(source, row) if kind=="album" else netease_track(row) for row in rows]
    elif source == "qqmusic":
        result = get("https://c.y.qq.com/soso/fcgi-bin/search_for_qq_cp", {"w":query,"p":page,"n":count,"format":"json","t":8 if kind=="album" else 0}).get("data", {}).get("album" if kind=="album" else "song")
        if not isinstance(result, dict): raise ValueError("QQ 音乐未返回有效搜索结果")
        rows, total = result.get("list", []), result.get("totalnum", 0)
        items = [album_item(source,row) if kind=="album" else qq_track(row) for row in rows]
    elif source == "apple":
        result = get("https://itunes.apple.com/search", {"term":query,"entity":"album" if kind=="album" else "song","country":"us","limit":count,"offset":offset})
        rows = result.get("results", [])
        items = [album_item(source,row) if kind=="album" else apple_track(row) for row in rows]
        total = offset+len(rows)+(1 if len(rows)==count else 0)
    elif source == "kuwo":
        result = get("https://search.kuwo.cn/r.s", {"all":query,"ft":"album" if kind=="album" else "music","pn":page-1,"rn":count,"rformat":"json","encoding":"utf8","mobi":1,"client":"kt","itemset":"web_2013"})
        rows = result.get("albumlist" if kind=="album" else "abslist", [])
        items = [album_item(source,row) if kind=="album" else kuwo_track(row) for row in rows]
        total = int(result.get("TOTAL") or result.get("total") or offset+len(rows)+(1 if len(rows)==count else 0))
    elif kind == "album":
        result = get("http://mobilecdn.kugou.com/api/v3/search/album", {"keyword":query,"page":page,"pagesize":count})["data"]
        rows, total = result.get("info", []), result.get("total", 0)
        items = [album_item(source, row) for row in rows]
    else:
        result = kugou_search(query, page, count)
        rows, total = result.get("lists", []), result.get("total", 0)
        items = [remember({"media_source":source,"media_id":row["FileHash"],"music_type":"recording","title":clean(row.get("SongName")),
            "artists":[clean(value) for value in row.get("SingerName", "").split("、")],"album":clean(row.get("AlbumName")),"album_id":str(row.get("AlbumID") or ""),
            "cover_url":_url(row.get("Image")),"duration":row.get("Duration"),"release_date":row.get("PublishDate") or row.get("PublishTime") or ""}) for row in rows]
    return {"items":items,"page":page,"has_more":offset+len(items)<int(total),"total":int(total)}


@cached(maxsize=128, ttl=1800, region="kugou-metadata-search")
def kugou_search(query, page, count):
    # music-tag-web 的公开搜索签名，使用标准库 MD5，免去 JS 执行环境。
    stamp = str(int(time.time()*1000))
    params = {"keyword":query,"page":page,"pagesize":count,"bitrate":0,"isfuzzy":0,"tag":"em","inputtype":0,"platform":"WebFilter","userid":-1,"clientver":2000,"iscorrection":1,"privilege_filter":0,"srcappid":2919,"clienttime":stamp,"mid":stamp,"uuid":stamp,"dfid":"-"}
    key = "NVPh5oo715z5DIWAeQlhMDsWXXQV4hwt"
    params["signature"] = hashlib.md5((key+"".join(f"{k}={v}" for k,v in sorted(params.items()))+key).encode()).hexdigest()
    return get("https://complexsearch.kugou.com/v2/search/song", params)["data"]


def album_item(source, row):
    item = {"media_source":source,"music_type":"album"}
    if source=="netease":
        item.update(media_id=str(row["id"]),title=row["name"],artists=[a["name"] for a in row.get("artists") or [row.get("artist") or {}] if a.get("name")],cover_url=_url(row.get("picUrl")),release_date=_date(row.get("publishTime")),total_tracks=row.get("size"))
    elif source=="qqmusic":
        mid = row.get("albumMID") or row["mid"]
        item.update(media_id=str(mid),title=row.get("albumName") or row["name"],artists=[row.get("singerName") or row.get("singername") or ""],cover_url=f"https://y.gtimg.cn/music/photo_new/T002R500x500M000{mid}.jpg",release_date=row.get("publicTime") or row.get("aDate"))
    elif source=="apple":
        item.update(media_id=str(row["collectionId"]),title=row["collectionName"],artists=[row["artistName"]],cover_url=_url(row.get("artworkUrl100")).replace("100x100bb","600x600bb"),release_date=row.get("releaseDate"),total_tracks=row.get("trackCount"))
    elif source=="kugou":
        item.update(media_id=str(row["albumid"]),title=clean(row["albumname"]),artists=[clean(name) for name in (row.get("singername") or "").split("、")],cover_url=_url(row.get("imgurl")),release_date=row.get("publishtime"),total_tracks=row.get("songcount"))
    else:
        item.update(media_id=str(row["albumid"]),title=clean(row["name"]),artists=[clean(row.get("artist"))],cover_url=_url(row.get("hts_img") or row.get("img")),release_date=row.get("pub"),total_tracks=int(row.get("musiccnt") or row.get("songnum") or 0))
    item["album"] = item["title"]
    return remember(item)


def detail(source, identity, kind="recording"):
    if source not in SOURCES or kind not in {"recording","album"} or not isinstance(identity,str) or not re.fullmatch(r"[A-Za-z0-9]{1,100}",identity):
        raise ValueError("音乐资料 ID 无效")
    cached_item = Cache(1000,1800,"streaming-selections").get(f"{source}:{kind}:{identity}")
    if kind=="album":
        if source=="netease":
            result=get(f"https://music.163.com/api/v1/album/{identity}"); row=result["album"]
            item=album_item(source,row); tracks=[netease_track(song) for song in result.get("songs") or row.get("songs",[])]
        elif source=="qqmusic":
            row=get("https://c.y.qq.com/v8/fcg-bin/fcg_v8_album_info_cp.fcg",{"albummid":identity,"format":"json"})["data"]
            item=album_item(source,row); tracks=[qq_track(song) for song in row.get("list",[])]
        elif source=="apple":
            rows=get("https://itunes.apple.com/lookup",{"id":identity,"entity":"song","country":"us","limit":200}).get("results",[])
            item=album_item(source,next(row for row in rows if row.get("wrapperType")=="collection")); tracks=[apple_track(row) for row in rows if row.get("kind")=="song"]
        elif source=="kuwo":
            row=get("https://search.kuwo.cn/r.s",{"stype":"albuminfo","albumid":identity,"alflac":1,"pn":0,"rn":1000,"rformat":"json","encoding":"utf8","mobi":1})
            item=album_item(source,row); tracks=[kuwo_track(song) for song in row.get("musiclist",[])]
        elif source=="kugou":
            item = cached_item or album_item(source, get("http://mobilecdn.kugou.com/api/v3/album/info", {"albumid":identity})["data"])
            tracks = []
            for page in range(1, 11):
                data = get("http://mobilecdn.kugou.com/api/v3/album/song", {"albumid":identity,"page":page,"pagesize":100})["data"]
                for row in data.get("info", []):
                    artist, separator, title = clean(row.get("filename")).partition(" - ")
                    tracks.append({"media_source":source,"media_id":row["hash"],"music_type":"recording", "title":title if separator else artist,
                        "artists":artist.split("、") if separator else item["artists"],"album":item["title"],"album_id":identity,"duration":row.get("duration")})
                if page*100 >= int(data.get("total") or 0) or not data.get("info"):
                    break
        else: raise ValueError("此来源仅支持歌曲资料搜索")
        for song in tracks:
            song["cover_url"]=song.get("cover_url") or item.get("cover_url")
            song["release_year"]=song.get("release_year") or item.get("release_year")
            song["album_artist"]=item.get("artist")
            song["total_tracks"]=item.get("total_tracks") or len(tracks)
            remember(song)
        return {**item,"tracks":tracks}
    if cached_item and cached_item.get("album_artist"):
        return cached_item
    if source=="netease":
        rows=get("https://music.163.com/api/song/detail/",{"ids":json.dumps([int(identity)])}).get("songs",[])
        if rows: return netease_track(rows[0])
    elif source=="qqmusic":
        request={"comm":{"ct":24,"cv":0},"req_1":{"module":"music.pf_song_detail_svr","method":"get_song_detail","param":{"song_id":int(identity) if identity.isdigit() else 0,"song_mid":"" if identity.isdigit() else identity,"song_type":0}}}
        response=get("https://u.y.qq.com/cgi-bin/musicu.fcg",{"data":json.dumps(request)}).get("req_1",{})
        if response.get("code")!=0: raise ValueError("QQ 音乐歌曲资料暂时不可用")
        return qq_track(response["data"]["track_info"])
    elif source=="apple":
        rows=get("https://itunes.apple.com/lookup",{"id":identity,"country":"us"}).get("results",[])
        if rows: return apple_track(rows[0])
    elif cached_item:
        return cached_item
    elif source=="kugou":
        row=get("https://m.kugou.com/app/i/getSongInfo.php",{"cmd":"playInfo","hash":identity})
        if row.get("songName"):
            return remember({"media_source":source,"media_id":identity,"music_type":"recording","title":row["songName"],"artists":[row.get("singerName") or ""],"cover_url":_url(row.get("album_img")),"duration":row.get("timeLength")})
    raise ValueError("没有读取到该歌曲资料，请重新搜索后选择")
