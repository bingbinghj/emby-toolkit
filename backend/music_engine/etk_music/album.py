# Adapted from MoviePilot v3 e0f17f5 (GPL-3.0); see ../LICENSE.MoviePilot.
"""复用 MP 的专辑上下文、发行核验与唯一曲目对位。"""
from __future__ import annotations
from copy import deepcopy
from pathlib import Path
from typing import Optional, Literal
from .audio import AudioMetadataHelper
from .models import MusicAlbumInfo, MusicInfo
from .meta import MetaMusic, music_credit_values
from .matching import (music_text_key, music_album_candidate_matches,
                      music_album_lookup_plan, music_album_has_consistent_tracks, expand_music_tracks, align_music_tracks)
from .recognition import unique_music_match, recognize_music_sources
from .observation import report_music_recognition
from .types import MediaSource, MUSIC_ENTITY_ALBUM, MUSIC_ENTITY_RECORDING

def _verified_source_album(albums: list[MusicAlbumInfo], meta: MetaMusic, tracks: list[MetaMusic]) -> Optional[MusicAlbumInfo]:
    """
    只使用完整对位的唯一专辑，返回副本以免污染来源自己的详情缓存。
    """
    matched = unique_music_match([album for album in albums if music_album_candidate_matches(album, meta, tracks)])
    if matched is None:
        return None
    result = deepcopy(matched)
    result.raw_data.update(match_coverage=1, match_basis="source_track_evidence")
    return result

def _lookup_source_album(
        chain, source: MediaSource, meta: MetaMusic, tracks: list[MetaMusic],
) -> Optional[MusicAlbumInfo]:
    """
    通过来源链驱动纯检索计划，豆瓣不将专辑搜索结果伪装成独立录音。
    """
    plan = music_album_lookup_plan(source, meta, tracks)
    response: list[MusicInfo] | MusicAlbumInfo | None
    kinds: tuple[str, ...]
    try:
        action = next(plan)
        while True:
            if action.truncated:
                report_music_recognition("ambiguous", "符合名称的候选专辑过多，请补充信息或手动选择")
                return None
            if action.meta is not None:
                kinds = ("album",) if action.meta.music_type == MUSIC_ENTITY_ALBUM else ("recording",)
                response = [] if source == MediaSource.DoubanMusic and action.meta.music_type == MUSIC_ENTITY_RECORDING else (
                    chain.search_music(action.meta, limit=10, music_types=kinds))
            else:
                response = chain.get_music_album(action.album_id or "")
            action = plan.send(response)
    except StopIteration as completed:
        return _verified_source_album(completed.value, meta, tracks)
    finally:
        plan.close()

def _apply_album_resource_credits(album_meta: MetaMusic, contextual_meta: MetaMusic) -> None:
    """
    只复制明确绑定当前发行的角色，不聚合不同曲目的表演阵容或制造来源。
    """
    for key, value in music_credit_values(contextual_meta).items():
        if value:
            setattr(album_meta, key, value)
            if key in contextual_meta.field_sources:
                album_meta.field_sources[key] = contextual_meta.field_sources[key]

def _album_context_with_resource(
    directory: Path,
    metas: list[MetaMusic],
    contextual_meta: Optional[MetaMusic],
) -> MetaMusic:
    """
    以文件自身专辑线索为先，用绑定到本次文件的资源证据补足目录查询。
    """
    album_meta = MetaMusic.from_album_context(directory.name, metas)
    for key in ("album", "album_artist"):
        tagged = {
            music_text_key(str(getattr(meta, key))) for meta in metas
            if getattr(meta, key) and meta.field_sources.get(key) in {"tag", "cue", "manual", "album_tags"}
        }
        if len(tagged) > 1:
            album_meta.organization_error = "同一发行范围内的专辑标签冲突，请分别选择发行版本"
        elif tagged:
            album_meta.field_sources[key] = "album_tags"
            if len(metas) == 1:
                setattr(album_meta, key, getattr(metas[0], key))
                if key == "album":
                    album_meta.title = metas[0].album or album_meta.title
                elif key == "album_artist":
                    album_meta.artists = [str(metas[0].album_artist)]
    for key in ("musicbrainz_release_id", "musicbrainz_release_group_id", "release_year", "original_year"):
        values = {getattr(meta, key) for meta in metas if getattr(meta, key)}
        if len(values) > 1 and key != "original_year":
            album_meta.organization_error = "同一发行范围内的发行身份或年份冲突，请核对标签"
        elif len(values) == 1:
            setattr(album_meta, key, next(iter(values)))
            album_meta.field_sources[key] = "album_tags"
        elif contextual_meta:
            setattr(album_meta, key, getattr(contextual_meta, key))
            if key in contextual_meta.field_sources:
                album_meta.field_sources[key] = contextual_meta.field_sources[key]
    if not contextual_meta:
        return album_meta
    _apply_album_resource_credits(album_meta, contextual_meta)
    if contextual_meta.album and not any(meta.album for meta in metas):
        album_meta.album = contextual_meta.album
        album_meta.title = contextual_meta.album
        album_meta.field_sources["album"] = contextual_meta.field_sources.get("album", "torrent")
    if not album_meta.artists:
        album_meta.artists = (
            [contextual_meta.album_artist] if contextual_meta.album_artist
            else list(contextual_meta.artists)
        )
    if not album_meta.album_artist and contextual_meta.album_artist:
        album_meta.album_artist = contextual_meta.album_artist
    for key in ("year", "version"):
        local_values = {getattr(meta, key) for meta in metas if getattr(meta, key)}
        if not getattr(album_meta, key):
            if len(local_values) == 1:
                setattr(album_meta, key, next(iter(local_values)))
            elif not local_values:
                setattr(album_meta, key, getattr(contextual_meta, key))
    return album_meta

def album_track_map(
    files: list[Path],
    metas: list[MetaMusic],
    album: MusicAlbumInfo,
    *,
    allow_title_override: bool = False,
    track_match: Literal["strict", "ignore_duration", "sequential"] = "strict",
) -> dict[str, MusicInfo]:
    """
    逻辑曲目全部对位后才输出对应物理文件，整轨始终保持 Album 身份。
    """
    if len(files) != len(metas) or not music_album_has_consistent_tracks(album):
        return {}
    logical, owners = expand_music_tracks(metas)
    aligned = align_music_tracks(logical, album.tracks, allow_title_override=allow_title_override, track_match=track_match)
    result: dict[str, MusicInfo] = {}
    for index, path in enumerate(files):
        positions = [position for position, owner in enumerate(owners) if owner == index]
        if not positions or any(position not in aligned for position in positions):
            continue
        info = album.to_music_info() if metas[index].music_layout == "image_cue" else deepcopy(album.tracks[aligned[positions[0]]])
        if not allow_title_override:
            for key, value in music_credit_values(metas[index]).items():
                if value:
                    setattr(info, key, value)
                    info.field_sources.pop(key, None)
                    if key in metas[index].field_sources:
                        info.field_sources[key] = metas[index].field_sources[key]
        for key in ("match_score", "match_coverage", "match_basis"):
            if key in album.raw_data:
                info.raw_data[key] = album.raw_data[key]
        info.set_library_category(album.library_category)
        info.classification = deepcopy(album.classification)
        info.classification_facts = dict(album.classification_facts)
        result[str(path.resolve())] = info
    return result


class SourceAlbums:
    def __init__(self, module, source):
        self.module, self.source = module, source

    def search_music(self, meta, **options):
        return self.module.search_music(meta, media_source=self.source, **options)

    def get_music_album(self, identity):
        return self.module.music_album(self.source, identity)


def recognize_group(files, metas, sources, selected=None, directory=None):
    from .catalog import provider, detail
    if selected:
        album = detail(selected["media_source"], selected["media_id"], "album", selected.get("musicbrainz_release_id"))
        if not album or not album.tracks:
            raise ValueError("所选专辑或发行版本没有可用曲目")
        result = album_track_map(files, metas, album, allow_title_override=True)
        if len(result) != len(files):
            raise ValueError("所选发行与文件不能唯一对位，请核对专辑、曲序和时长")
        diagnostic = {"status": "manual", "message": "手动选择发行并核验曲目"}
    else:
        context = _album_context_with_resource(directory or files[0].parent, metas, None)
        if context.organization_error:
            raise ValueError(context.organization_error)
        logical, _ = expand_music_tracks(metas)
        if not logical or len(logical)<2 and not (context.musicbrainz_release_id or context.musicbrainz_release_group_id):
            return {}, {}
        pinned = [MediaSource.MusicBrainz] if context.musicbrainz_release_id or context.musicbrainz_release_group_id else [MediaSource(source) for source in sources]
        def recognize(source):
            module = provider(source)
            if source==MediaSource.MusicBrainz:
                return module.match_music_album(context, logical)
            return _lookup_source_album(SourceAlbums(module, source), source, context, logical)
        album, diagnostic = recognize_music_sources(pinned, recognize, lambda item, source: bool(item and item.tracks and item.media_source==source))
        if not album:
            return {}, diagnostic
        result = album_track_map(files, metas, album)
        if len(result)!=len(files):
            return {}, {"status":"conflict", "message":"专辑不能覆盖当前全部音频，请核对发行版本"}
    for path, meta in zip(files, metas):
        info = result[str(path.resolve())]
        if not selected and album.media_source != MediaSource.MusicBrainz:
            # MP 次级来源的专辑身份不能证明具体发行，保留本地碟号与版本年份。
            for key in ("disc_number", "track_number", "total_tracks", "total_discs", "year", "release_year", "original_year"):
                value = getattr(meta, key, None)
                if value:
                    setattr(info, key, value)
                    if key in meta.field_sources:
                        info.field_sources[key] = meta.field_sources[key]
                    else:
                        info.field_sources.pop(key, None)
        info.raw_data["recognition"] = {**diagnostic, "identity_type":"album", "release_verified":bool(album.musicbrainz_release_id)}
    return result, diagnostic


def _preserve_music_image_tags(path: Path, meta: MetaMusic) -> None:
    """
    整轨 CUE 的识别结果使用专辑标题，但已有音频标签仍须按保留策略留存。

    :param path: 已下载的音频路径
    :param meta: 可原地调整的 CUE 合并元数据
    """
    if meta.music_layout != "image_cue":
        return
    tags = AudioMetadataHelper.read_tags(path)
    if not tags:
        return
    meta.title = tags.title or meta.title
    meta.artists = list(tags.artists or meta.artists)
    meta.track_number = tags.track_number

def _merge_music_album_metadata(
        local_meta: MetaMusic,
        album: MusicInfo,
        track: Optional[MusicInfo] = None,
) -> MetaMusic:
    """
    把专辑级字段合并到音轨；仅在曲目已可靠对位时采用非空的线上曲目字段。

    单碟专辑除外：线上碟数已确认为 1 时碟号必为 1，本地错标的碟号不能
    再与 total_discs=1 配成 "2/1" 写进标签。音轨身份仍使用本地录音 ID，
    不能用专辑 ID 补位。

    :param local_meta: 当前音频读取的标签与路径证据
    :param album: 需要合并的专辑信息
    :param track: 已可靠对位的专辑曲目；为 None 时保留本地曲目字段
    :return: 不修改输入对象的合并结果
    """
    merged = deepcopy(local_meta)
    merged.artists = list(local_meta.artists or album.artists)
    merged.album = album.album or album.title or local_meta.album
    merged.album_artist = album.album_artist or album.artist or local_meta.album_artist
    merged.year = album.year or local_meta.year
    merged.total_tracks = album.total_tracks or local_meta.total_tracks
    merged.total_discs = album.total_discs or local_meta.total_discs
    if album.total_discs == 1:
        merged.disc_number = 1
    if track:
        merged.title = track.title or merged.title
        merged.artists = list(track.artists or merged.artists)
        merged.track_number = track.track_number or merged.track_number
        merged.total_tracks = track.total_tracks or merged.total_tracks
        if album.total_discs != 1:
            merged.disc_number = track.disc_number or merged.disc_number
    # MetaMusic 的身份代表音轨；保留已有单曲 ID，不能用专辑 ID 补位，
    # 否则会误写 musicbrainz_trackid，使下次拿 release-group 查询 recording。
    return merged
