# Adapted from MoviePilot v3 e0f17f5 (GPL-3.0); see ../LICENSE.MoviePilot.
from __future__ import annotations
import re
from copy import deepcopy
from pathlib import Path
from typing import Optional, Any, TypeGuard
from .meta import MetaMusic, music_credit_values
from .models import MusicInfo
from .matching import music_track_title_is_weak, music_album_title_is_weak
from .observation import music_recognition_is_blocked
from .types import MediaSource, MUSIC_ENTITY_RECORDING
_FINGERPRINT_TITLE_QUALIFIER = re.compile(
    r"\s*[\[(（【][^\])）】]*(?:radio|single|version|edit|mix|remix|remaster(?:ed)?|"
    r"live|acoustic|demo|mono|stereo|recorded|pop)[^\])）】]*[\])）】]",
    re.IGNORECASE,
)


_CONTENT_RATING_QUALIFIER = re.compile(
    r"\s*[\[(（【]\s*(?:explicit|clean)\s*[\])）】]",
    re.IGNORECASE,
)


_SINGLE_RELEASE_SUFFIX = re.compile(
    r"\s*[-–—]\s*(?:single|单曲)\s*$",
    re.IGNORECASE,
)


def _is_regular_file(path: Path) -> bool:
    """
    判断路径是否仍指向可读取的普通文件。
    """
    return path.exists() and path.is_file()


def _has_remote_music_identity(
    info: Optional[MusicInfo],
) -> TypeGuard[MusicInfo]:
    """
    判断音乐结果是否已获得可终止回退的远程身份。
    """
    return bool(info and info.media_source and info.media_id)


def _musicbrainz_recording_meta(meta: MetaMusic, recording_id: str) -> MetaMusic:
    """
    构造 MusicBrainz Recording 直查使用的独立身份副本。
    """
    identity_meta = MetaMusic.from_dict(meta.to_dict())
    identity_meta.media_source = MediaSource.MusicBrainz
    identity_meta.media_id = recording_id
    identity_meta.music_type = MUSIC_ENTITY_RECORDING
    return identity_meta


def _without_music_identity(meta: MetaMusic) -> MetaMusic:
    """
    复制音乐元数据并移除可能误导标题搜索的远程身份。
    """
    clean_meta = MetaMusic.from_dict(meta.to_dict())
    clean_meta.media_source = None
    clean_meta.media_id = None
    return clean_meta


def _merge_music_path_evidence(
    meta: MetaMusic,
    evidence: Optional[MetaMusic],
) -> MetaMusic:
    """
    把同一文件的标签证据补入文件名搜索，保留ISRC约束但不传播来源实体ID。
    """
    if not meta or not evidence:
        return meta
    merged = MetaMusic.from_dict(meta.to_dict())
    if not merged.artists and evidence.artists:
        merged.artists = list(evidence.artists)
    if not merged.album_artist and evidence.album_artist:
        merged.album_artist = evidence.album_artist
    if not merged.album and evidence.album:
        merged.album = evidence.album
    if not merged.year and evidence.year:
        merged.year = evidence.year
    if not merged.version and evidence.version:
        merged.version = evidence.version
    if not merged.isrc and evidence.isrc:
        merged.isrc = evidence.isrc
    return merged


def _merge_contextual_music_evidence(
    meta: MetaMusic,
    contextual_meta: MetaMusic,
) -> MetaMusic:
    """
    以同目录高置信共识补齐缺失的艺人和专辑证据。
    """
    merged = MetaMusic.from_dict(meta.to_dict())
    for key, value in music_credit_values(contextual_meta).items():
        if value and not getattr(merged, key):
            setattr(merged, key, value)
            merged.field_sources[key] = contextual_meta.field_sources.get(key, "torrent")
    if not merged.artists and contextual_meta.artists:
        merged.artists = list(contextual_meta.artists)
    if not merged.album_artist and contextual_meta.album_artist:
        merged.album_artist = contextual_meta.album_artist
    if not merged.album and contextual_meta.album:
        merged.album = contextual_meta.album
    if contextual_meta.year:
        # 整理链会先用最近的发行目录年份纠正下载包中的旧标签年份；这里
        # 必须保留该强上下文，否则重新读取音频标签后又会退回创作年或旧版年。
        merged.year = contextual_meta.year
    if not merged.version and contextual_meta.version:
        merged.version = contextual_meta.version
    return merged


def _merge_music_audio_quality(info: MusicInfo, meta: MetaMusic) -> MusicInfo:
    """
    合并实际音频参数和明确的本地角色标签，不改变主艺人或远端身份。
    """
    for key in (
        "audio_format",
        "audio_lossless",
        "bit_depth",
        "sample_rate",
        "bitrate",
    ):
        value = getattr(meta, key, None)
        if value is not None:
            setattr(info, key, value)
            if key in meta.field_sources:
                info.field_sources[key] = meta.field_sources[key]
    for key, value in music_credit_values(meta).items():
        if value and meta.field_sources.get(key) in {"tag", "cue", "torrent", "manual"}:
            setattr(info, key, value)
            info.field_sources[key] = meta.field_sources[key]
    return info


def _finalize_music_path_info(
    meta: MetaMusic,
    info: Optional[MusicInfo],
) -> MusicInfo:
    """
    统一远端命中和本地兜底的音频质量合并。
    """
    if info and music_recognition_is_blocked(info) and not _has_remote_music_identity(info):
        result = MusicInfo.from_meta(meta)
        result.raw_data["recognition"] = deepcopy(info.raw_data["recognition"])
    else:
        result = _merge_music_audio_quality(info or MusicInfo.from_meta(meta), meta)
    if (
        _has_remote_music_identity(result)
        and result.music_type == MUSIC_ENTITY_RECORDING
        and not result.album_type
    ):
        # 部分 MusicBrainz Recording 没有关联 Release Group 类型，统一按单曲归类。
        result.album_type = "Single"
    return result


def _fingerprint_info_matches_evidence(
    info: Optional[MusicInfo],
    tag_meta: Optional[MetaMusic],
    filename_meta: Optional[MetaMusic],
    *,
    fingerprint_score: Optional[float] = None,
    fingerprint_duration: Optional[int] = None,
) -> bool:
    """
    保留强标签冲突检查；原生高分指纹可用真实时长弥补占位名称或缺失署名。
    """
    # 仅在本层音乐证据核验时加载所需的匹配规则。
    from etk_music.matching import (  # pylint: disable=import-outside-toplevel
        music_album_matches,
        music_artist_evidence_matches,
        music_credit_conflicts,
        music_isrc_conflicts,
        music_text_key,
        music_usable_artists,
        music_version_matches,
        music_year_matches,
    )

    if not _has_remote_music_identity(info):
        return False
    primary = tag_meta if tag_meta and not music_track_title_is_weak(tag_meta) else filename_meta or tag_meta
    if not primary or music_isrc_conflicts(info, tag_meta or primary):
        return False
    weak_title = music_track_title_is_weak(primary)
    local_duration = fingerprint_duration or (tag_meta.duration if tag_meta else None) or primary.duration
    close_duration = bool(local_duration and info.duration and abs(local_duration - info.duration) <= max(3, min(8, local_duration * 0.025)))
    native_strong = fingerprint_score is not None and fingerprint_score >= 0.98 and close_duration
    if local_duration and info.duration and abs(local_duration - info.duration) > max(10, local_duration * 0.08):
        return False
    artist_meta = tag_meta if tag_meta and tag_meta.artists else primary
    artist_evidence = music_usable_artists(artist_meta.artists) or music_usable_artists(
        filename_meta.artists if filename_meta else []
    )
    collective = {music_text_key(value) for value in ("Various Artists", "Various", "VA", "群星", "众艺人", "眾藝人")}
    if (fingerprint_score is not None and artist_meta.field_sources.get("artists") in {"directory", "torrent", "album_tags"}
            and all(music_text_key(artist) in collective for artist in artist_evidence)):
        artist_evidence = []
    if not artist_evidence and not native_strong:
        return False
    if music_credit_conflicts(info, artist_meta) or (artist_evidence and not music_artist_evidence_matches(info, artist_meta, artist_evidence)):
        return False
    if (not weak_title or primary.version) and not music_version_matches(info, primary):
        return False
    if tag_meta and tag_meta is not primary and tag_meta.version and not music_version_matches(info, tag_meta):
        return False
    standalone_single = _is_standalone_single_evidence(primary)
    if (
        primary.album
        and info.album
        and not music_album_matches(info, primary.album)
        and not standalone_single
        and not native_strong
    ):
        return False
    if not native_strong and not standalone_single and not music_year_matches(info, primary):
        return False
    if weak_title:
        return native_strong
    return _fingerprint_title_matches(info, primary.title or "")


def _fingerprint_title_matches(info: MusicInfo, title_evidence: str) -> bool:
    """
    在音频与署名已核验后比较曲名别称及展示差异，避免混入发行身份判断。
    """
    from difflib import SequenceMatcher

    # 仅在本层音乐证据核验时加载所需的匹配规则。
    from etk_music.matching import (  # pylint: disable=import-outside-toplevel
        music_base_title,
        music_text_key,
        music_title_matches,
        music_titles,
    )

    if music_title_matches(info, title_evidence):
        return True
    # 上层已核验音频、艺人及版本，允许常见展示后缀与轻微标签拼写差异。
    evidence_key = music_text_key(music_base_title(title_evidence))
    version_suffix = re.compile(
        r"(?:radio|single|version|edit|mix|remix|remaster(?:ed)?|live|acoustic|"
        r"demo|mono|stereo|recorded)+"
    )
    for title in music_titles(info):
        candidate_key = music_text_key(music_base_title(title))
        if not candidate_key:
            continue
        shorter, longer = sorted((candidate_key, evidence_key), key=len)
        if shorter and longer.startswith(shorter) and version_suffix.fullmatch(longer[len(shorter):]):
            return True
        qualified_evidence_key = music_text_key(
            music_base_title(_FINGERPRINT_TITLE_QUALIFIER.sub("", title_evidence))
        )
        qualified_candidate_key = music_text_key(
            music_base_title(_FINGERPRINT_TITLE_QUALIFIER.sub("", title))
        )
        if qualified_evidence_key and qualified_evidence_key == qualified_candidate_key:
            return True
        if SequenceMatcher(None, candidate_key, evidence_key).ratio() >= 0.82:
            return True
    return False


def _is_standalone_single_evidence(meta: MetaMusic) -> bool:
    """
    判断本地标签是否明确把当前录音描述为同名单曲发行。
    """
    # 仅在本层音乐证据核验时加载所需的匹配规则。
    from etk_music.matching import (  # pylint: disable=import-outside-toplevel
        music_base_title,
        music_text_key,
    )

    title_key = music_text_key(
        music_base_title(
            _SINGLE_RELEASE_SUFFIX.sub(
                "",
                _CONTENT_RATING_QUALIFIER.sub("", meta.title or ""),
            )
        )
    )
    album_key = music_text_key(
        music_base_title(
            _SINGLE_RELEASE_SUFFIX.sub(
                "",
                _CONTENT_RATING_QUALIFIER.sub("", meta.album or ""),
            )
        )
    )
    return bool(title_key and album_key and title_key == album_key)


def _reconcile_fingerprint_release(
    info: MusicInfo,
    tag_meta: Optional[MetaMusic],
    filename_meta: Optional[MetaMusic],
    *,
    verified_audio: bool = False,
) -> MusicInfo:
    """
    指纹只确认录音，用本地发行证据校正远端随意选取的关联专辑。

    MusicBrainz Recording 可以同时收录于单曲和原声专辑。指纹证明的是录音
    身份，不能确认实际发行；采用本地专辑字段时移除没有证明的发行标识，
    保留字段来源，旧插件的单ID回执仍只适用原有同名单曲纠正。
    """
    # 仅在本层音乐证据核验时加载所需的匹配规则。
    from etk_music.matching import (  # pylint: disable=import-outside-toplevel
        music_album_matches,
        music_year_matches,
    )
    primary = tag_meta if tag_meta and tag_meta.title else filename_meta
    if (
        not primary
        or not (_is_standalone_single_evidence(primary) or verified_audio)
        or not primary.album
        or music_album_title_is_weak(primary)
        or (
            music_album_matches(info, primary.album)
            and music_year_matches(info, primary)
        )
    ):
        return info

    reconciled = deepcopy(info)
    reconciled.album = primary.album
    reconciled.album_artist = primary.album_artist or info.artist
    reconciled.album_id = None
    reconciled.musicbrainz_release_id = primary.musicbrainz_release_id
    reconciled.musicbrainz_release_group_id = primary.musicbrainz_release_group_id
    reconciled.musicbrainz_release_track_id = primary.musicbrainz_release_track_id
    reconciled.release_year = primary.release_year
    reconciled.original_year = primary.original_year or info.original_year
    reconciled.album_type = primary.album_type or ("Single" if _is_standalone_single_evidence(primary) else None)
    reconciled.secondary_types = list(primary.secondary_types)
    reconciled.year = int(primary.year) if primary.year is not None else None
    reconciled.release_date = None
    reconciled.release_status = None
    reconciled.disc_number = primary.disc_number
    reconciled.track_number = primary.track_number
    reconciled.total_tracks = primary.total_tracks
    reconciled.total_discs = primary.total_discs
    reconciled.album_aliases = []
    reconciled.cover_url = None
    reconciled.category = ""
    reconciled.metadata_category = reconciled.album_type or ""
    reconciled.classification = None
    for key in ("album", "album_artist", "year", "release_year", "disc_number", "track_number", "total_tracks", "total_discs",
                "musicbrainz_release_id", "musicbrainz_release_group_id", "musicbrainz_release_track_id"):
        if getattr(reconciled, key, None) is not None:
            reconciled.field_sources[key] = primary.field_sources.get(key, "tag" if primary is tag_meta else "filename")
        else:
            reconciled.field_sources.pop(key, None)
    return reconciled


def _select_fingerprint_info(
        meta: MetaMusic, matches: list[tuple[Optional[float], MusicInfo]],
        tag_meta: Optional[MetaMusic], filename_meta: Optional[MetaMusic],
        *, truncated: bool = False,
) -> Optional[MusicInfo]:
    """
    只采用唯一可解释的录音候选，近分多录音保留待确认，不用文本回退掩盖歧义。
    """
    if not matches:
        return None
    if tag_meta and tag_meta.media_source == MediaSource.MusicBrainz and tag_meta.media_id:
        tagged = next(((score, info) for score, info in matches if info.media_id == tag_meta.media_id), None)
        if tagged is None:
            return None
        score, info = tagged
        return _finish_fingerprint_recording(score, info, tag_meta, filename_meta)
    ranked = sorted(matches, key=lambda item: item[0] if item[0] is not None else 1.0, reverse=True)
    if truncated or (len(ranked) > 1 and round((ranked[0][0] or 0) - (ranked[1][0] or 0), 6) <= 0.05):
        pending = MusicInfo.from_meta(meta)
        pending.media_source, pending.media_id = None, None
        pending.raw_data["recognition"] = {"status": "ambiguous", "message": "指纹对应多个合理的录音，请手动选择歌曲或专辑", "candidates": [
            {"media_source": str(info.media_source), "media_id": info.media_id, "title": info.title, "artist": info.artist, "score": score}
            for score, info in ranked[:5]
        ]}
        return pending
    score, info = ranked[0]
    return _finish_fingerprint_recording(score, info, tag_meta, filename_meta)


def _finish_fingerprint_recording(
        score: Optional[float], info: MusicInfo, tag_meta: Optional[MetaMusic], filename_meta: Optional[MetaMusic],
) -> MusicInfo:
    """
    仅确认Recording；发行ID只能沿用实际标签，不能从指纹详情所选关联发行推断。
    """
    if score is None:
        # 旧插件的单ID回执沿用既有对象及文本核验契约，不伪造原生指纹结论。
        return _reconcile_fingerprint_release(info, tag_meta, filename_meta)
    result = deepcopy(_reconcile_fingerprint_release(info, tag_meta, filename_meta,
                                                     verified_audio=score is not None and score >= 0.98))
    result.album_id = tag_meta.musicbrainz_release_group_id if tag_meta else None
    result.field_sources.pop("album_id", None)
    for key in ("musicbrainz_release_id", "musicbrainz_release_group_id", "musicbrainz_release_track_id"):
        value = getattr(tag_meta, key, None)
        setattr(result, key, value)
        if value:
            result.field_sources[key] = tag_meta.field_sources.get(key, "tag") if tag_meta else "tag"
        else:
            result.field_sources.pop(key, None)
    result.raw_data["recognition"] = {"status": "matched", "method": "fingerprint", "score": score,
                                       "identity_type": "recording", "release_verified": False}
    return result


def _fingerprint_candidate_list(recording_id: Optional[str], observed: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """
    旧插件仍只给ID时保持原核验要求，不给它制造AcoustID分数。
    """
    if observed:
        return observed[:5]
    return [{"recording_id": recording_id, "score": None, "duration": None}] if recording_id else []


def _matching_fingerprint(
        candidate: dict[str, Any], info: Optional[MusicInfo],
        tag_meta: Optional[MetaMusic], filename_meta: Optional[MetaMusic],
) -> bool:
    """
    同步和异步候选采用同一证据核验规则。
    """
    return _fingerprint_info_matches_evidence(info, tag_meta, filename_meta,
                                             fingerprint_score=candidate.get("score"),
                                             fingerprint_duration=candidate.get("duration"))


def _music_info_matches_text_evidence(
    info: Optional[MusicInfo],
    meta: Optional[MetaMusic],
) -> bool:
    """
    统一校验版本、年份、ISRC和演奏证据，插件结果也不能绕过明确冲突。
    """
    # 仅在本层音乐证据核验时加载所需的匹配规则。
    from etk_music.matching import (  # pylint: disable=import-outside-toplevel
        music_artist_evidence_matches,
        music_credit_conflicts,
        music_isrc_conflicts,
        music_isrc_matches,
        music_version_matches,
        music_year_matches,
    )

    if not _has_remote_music_identity(info) or not meta:
        return False
    return bool(music_version_matches(info, meta) and music_year_matches(info, meta)
                and not music_credit_conflicts(info, meta)
                and not music_isrc_conflicts(info, meta)
                and (not any(music_credit_values(meta).values()) or music_isrc_matches(info, meta)
                     or music_artist_evidence_matches(info, meta)))
