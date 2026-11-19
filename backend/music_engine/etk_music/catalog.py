"""把 MP 的音乐来源与识别规则接入 ETK 独立音乐菜单。"""
from concurrent.futures import ThreadPoolExecutor
from copy import deepcopy
from itertools import zip_longest
from pathlib import Path
from .audio import AudioMetadataHelper
from .meta import MetaMusic
from .models import MusicInfo, MusicAlbumInfo, MusicLyrics
from .types import MediaSource
from .recognition import recognize_music_sources, unique_music_match
from .musicbrainz import MusicBrainzModule
from .theaudiodb import TheAudioDbModule
from .douban import DoubanMusic
from .listenbrainz import ListenBrainzModule
from .acoustid import AcoustIdModule
from .lrclib import LrclibModule
from .amll import AmllModule
from .observation import capture_music_recognition, music_recognition_is_blocked
from .matching import align_music_tracks, music_title_matches, music_artist_evidence_matches, music_version_matches
from .evidence import _fingerprint_info_matches_evidence, _select_fingerprint_info
from .runtime import request_budget, request_timeout, get_runtime_setting


def provider(source):
    constructors = {"musicbrainz": MusicBrainzModule, "theaudiodb": TheAudioDbModule, "doubanmusic": DoubanMusic}
    if str(source) not in constructors:
        raise ValueError("请选择 MusicBrainz、TheAudioDB 或豆瓣音乐")
    module = constructors[str(source)]()
    if hasattr(module, "init_module"):
        module.init_module()
    return module


def serialize(value):
    if isinstance(value, list):
        return [serialize(item) for item in value]
    if not value:
        return None
    if isinstance(value, (MusicInfo, MusicAlbumInfo)):
        value = simplified(value, get_runtime_setting("MUSIC_METADATA_TO_SIMPLIFIED", True))
    return value.to_dict()


def search(query, sources, kind=None, count=30):
    """来源并行查询、分别去重再交错合并，与 MP 音乐目录服务一致。"""
    meta = MetaMusic.parse_query(query)
    errors = {}
    def fetch(source):
        try:
            with capture_music_recognition(seconds=90, request_limit=16) as observed:
                result = provider(source).search_music(meta, limit=count, media_source=MediaSource(source), music_types=[kind] if kind else ["recording", "album"] )
                if observed.status in {"service_error", "budget_exhausted"}:
                    errors[source] = observed.message or "音乐资料源暂时不可用"
                return result or []
        except Exception as error:
            errors[source] = str(error)
            return []
    with ThreadPoolExecutor(max_workers=3) as pool:
        groups = list(pool.map(fetch, sources))
    result, seen = [], set()
    for row in zip_longest(*groups):
        for item in row:
            if not item:
                continue
            key = (str(item.media_source), item.music_type, item.media_id)
            if key not in seen:
                seen.add(key)
                result.append(item)
    return {"items": serialize(result[:count]), "errors": errors}


def detail(source, identity, kind="recording", release=None):
    module, source = provider(source), MediaSource(source)
    if kind == "album":
        options = {"musicbrainz_release_id": release} if release and source == MediaSource.MusicBrainz else {}
        return module.music_album(source, identity, **options)
    if kind == "artist":
        function = getattr(module, "music_artist", None)
        if not function:
            raise ValueError("此来源未提供艺术家详情")
        return function(source, identity)
    return module.recognize_music(source, identity, music_type="recording")


def related(source, identity, kind, page=1, album_type=None):
    module, source = provider(source), MediaSource(source)
    if kind == "artist_albums":
        options = {"album_type": album_type} if album_type and source in {MediaSource.MusicBrainz, MediaSource.TheAudioDB} else {}
        return serialize(module.music_artist_albums(source, identity, page=page, count=30, **options) or [])
    function = getattr(module, "music_artist_related" if kind == "artist" else "music_album_related", None)
    return serialize(function(source, identity, count=24) or []) if function else []


def explore(body):
    count, page = min(max(int(body.get("count", 30)), 1), 100), max(int(body.get("page", 1)), 1)
    if body.get("source") == "doubanmusic":
        items = DoubanMusic().music_discover(MediaSource.DoubanMusic, page=page, count=count, mode="tag", tags=body.get("tags", ""), sort=body.get("douban_sort", "U"))
    elif body.get("mode") == "fresh":
        items = ListenBrainzModule().music_fresh_releases(days=int(body.get("days", 14)), sort=body.get("sort", "release_date"), past=body.get("past", True), future=body.get("future", True), offset=(page-1)*count, count=count)
    else:
        items = ListenBrainzModule().music_chart(body.get("range_name", "this_month"), offset=(page-1)*count, count=count, entity=body.get("entity", "recording"))
    items = items or []
    if body.get("min_listen_count") and body.get("source") != "doubanmusic":
        items = [item for item in items if (item.listen_count or 0)>=max(0,int(body["min_listen_count"]))]
    if body.get("with_cover"):
        items = [item for item in items if item.cover_url]
    if body.get("sort_by") in {"listen_count.asc", "listen_count.desc"}:
        items = sorted(items,key=lambda item:item.listen_count or 0,reverse=body["sort_by"].endswith("desc"))
    return {"items": serialize(items), "page": page}


def recognize(path, sources, selected=None):
    """保留原始标签证据，指纹只确认录音，不伪造发行版本。"""
    path = Path(path)
    meta, tags, filename = AudioMetadataHelper.read_evidence(path)
    if meta.organization_error:
        raise ValueError(meta.organization_error)
    if selected:
        if meta.music_layout=="image_cue" and selected.get("music_type", "recording")!="album":
            raise ValueError("整轨 CUE 必须选择专辑，不能使用单曲身份")
        info = detail(selected["media_source"], selected["media_id"], selected.get("music_type", "recording"), selected.get("musicbrainz_release_id"))
        if not info:
            raise ValueError("所选音乐资料已不存在或暂时无法读取")
        if selected.get("music_type") == "album":
            from .matching import expand_music_tracks
            tracks, _ = expand_music_tracks([meta])
            aligned = align_music_tracks(tracks, info.tracks, allow_title_override=True)
            if len(aligned) != len(tracks):
                raise ValueError("所选发行版本与当前曲目无法唯一对位，请纠正专辑或曲序")
            if meta.music_layout == "image_cue":
                result = info.to_music_info()
            else:
                result = deepcopy(info.tracks[aligned[0]])
            return meta, result
        return meta, info
    if meta.music_layout == "image_cue":
        # 专辑分组已按配置来源核验过；无在线结果时保留整轨专辑身份。
        return meta, MusicInfo.from_meta(meta)
    if meta.media_source and meta.media_id:
        # 原生身份固定所属来源与实体，不能把专辑 ID 当录音或跨来源查询。
        bound = provider(meta.media_source).recognize_music(meta.media_source, meta.media_id, music_type=meta.music_type or "recording")
        if bound:
            if bound.music_type=="album":
                from .album import recognize_group
                matches, _ = recognize_group([path], [meta], sources, {"media_source":str(meta.media_source),"media_id":meta.media_id,"music_type":"album","musicbrainz_release_id":meta.musicbrainz_release_id})
                return meta, matches[str(path.resolve())]
            return meta, bound
    if sources[0] == "musicbrainz" and not meta.media_id:
        with capture_music_recognition(seconds=90, request_limit=16) as observed:
            fingerprint = AcoustIdModule()
            fingerprint.init_module()
            fingerprint.identify_music_by_fingerprint(path)
            candidates = list(observed.fingerprint_candidates)
            matches = []
            for candidate in candidates[:5]:
                info = detail("musicbrainz", candidate["recording_id"])
                if _fingerprint_info_matches_evidence(info, tags, filename, fingerprint_score=candidate.get("score"), fingerprint_duration=candidate.get("duration")):
                    matches.append((candidate.get("score"), info))
            result = None if observed.failed else _select_fingerprint_info(meta, matches, tags, filename, truncated=len(candidates)>5 or any(item.get("truncated") for item in candidates))
            if result:
                return meta, result
    def identify(source):
        module = provider(source)
        for evidence in (tags, filename):
            if not evidence or not evidence.title:
                continue
            if filename is evidence:
                from .evidence import _merge_music_path_evidence
                evidence = _merge_music_path_evidence(evidence, tags)
            if source == MediaSource.DoubanMusic:
                candidates = module.search_music(evidence, music_types=["album"]) or []
                matches = []
                for item in candidates[:3]:
                    album = module.music_album(source, item.media_id)
                    if album:
                        matches.extend(track for track in album.tracks if music_title_matches(track,evidence.title) and music_artist_evidence_matches(track,evidence) and music_version_matches(track,evidence))
                info = unique_music_match(matches)
            else:
                info = module.recognize_media(meta=evidence, media_source=source, music_type="recording")
            if info and (info.media_id or music_recognition_is_blocked(info)):
                return info
        return None
    pinned = [meta.media_source] if meta.media_source and meta.media_id else [MediaSource(source) for source in sources]
    result, diagnostic = recognize_music_sources(pinned, identify, lambda info,source:bool(info and info.media_id and info.media_source==source))
    result = result or MusicInfo.from_meta(meta)
    result.raw_data["recognition"] = diagnostic
    return meta, result


def lyrics(info, path=None, budget=120):
    candidates = []
    if path:
        embedded = AudioMetadataHelper.read_lyrics(Path(path))
        if embedded:
            candidates.append(embedded)
    with request_budget(budget):
        for module in (LrclibModule(), AmllModule()):
            if request_timeout(1)<=0:
                break
            candidates.extend(module.music_lyrics_candidates(info) or [])
    if info.lyrics:
        candidates.append(MusicLyrics(provider="theaudiodb", plain_lyrics=info.lyrics, match_score=85, provider_priority=10))
    if not candidates:
        return None
    return max(candidates, key=lambda item: (item.quality_rank, item.match_score, item.provider_priority))


def simplified(info, enabled):
    from .display import MusicDisplay
    return MusicDisplay._simplify_recognized_music_info(info) if enabled else deepcopy(info)
