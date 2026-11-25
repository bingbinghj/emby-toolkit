"""Adapted from MoviePilot 6201565 (GPL-3.0): automatic track alignment."""
import asyncio
from copy import deepcopy
from unittest.mock import AsyncMock, Mock
import pytest
from etk_music.models import MusicInfo, MusicAlbumInfo
from etk_music.meta import MetaMusic
from etk_music.types import MediaSource, MUSIC_ENTITY_RECORDING
from etk_music.matching import align_music_tracks_by_policy, music_album_candidate_matches, normalize_auto_music_track_match
from etk_music.album import _verified_source_album
from etk_music.musicbrainz import MusicBrainzModule
from etk_music import runtime


@pytest.mark.parametrize('policy,matched', [('strict', False), ('fallback_ignore_duration', True), ('ignore_duration', True), ('sequential', False)])
def test_automatic_group_uses_saved_policy_and_manual_choice_stays_explicit(tmp_path, monkeypatch, policy, matched):
    from etk_music.album import recognize_group
    from etk_music import catalog
    meta, tracks = _ost_local_metas(track2_duration=227)
    files = [tmp_path / '01.flac', tmp_path / '02.flac']
    detail = _ost_release_detail()
    calls = []
    class Source:
        def match_music_album(self, context, logical, **options):
            calls.append(options.get('track_match', 'strict'))
            return MusicBrainzModule._select_release_match(context, logical, [detail], **options)
    monkeypatch.setattr(catalog, 'provider', lambda _: Source())
    monkeypatch.setitem(runtime._settings, 'MUSIC_TRACK_MATCH', policy)
    result, _ = recognize_group(files, tracks, ['musicbrainz'], directory=tmp_path / meta.album)
    assert len(result) == (2 if matched else 0)
    assert calls == [normalize_auto_music_track_match(policy)]
    monkeypatch.setattr(catalog, 'detail', lambda *_: MusicBrainzModule._release_to_album(detail))
    with pytest.raises(ValueError, match='不能唯一对位'):
        recognize_group(files, tracks, ['musicbrainz'], {'media_source':'musicbrainz','media_id':'album'}, track_match='strict')

def _ost_release_detail(
    release_id: str = "rel-ost-1",
    group_id: str = "rg-ost-1",
    *,
    title: str = "Game OST",
    track2_length_ms: int = 28_000,
    recording2_id: str = "rec-2",
) -> dict:
    """
    构造含隐藏曲目时长差异的 MusicBrainz 发行详情载荷。
    """
    return {
        "id": release_id,
        "title": title,
        "date": "2024-05-01",
        "country": "JP",
        "text-representation": {"script": "Latn"},
        "release-group": {"id": group_id, "primary-type": "Album", "first-release-date": "2024-05-01"},
        "artist-credit": [{"artist": {"id": "artist-1", "name": "OST Composer"}}],
        "media": [
            {
                "position": 1,
                "track-count": 2,
                "tracks": [
                    {
                        "id": f"{release_id}-t1",
                        "position": 1,
                        "title": "Opening Theme",
                        "length": 180_000,
                        "recording": {"id": "rec-1", "title": "Opening Theme", "length": 180_000},
                    },
                    {
                        "id": f"{release_id}-t2",
                        "position": 2,
                        "title": "Hidden Medley",
                        "length": track2_length_ms,
                        "recording": {"id": recording2_id, "title": "Hidden Medley", "length": track2_length_ms},
                    },
                ],
            }
        ],
    }


def _ost_local_metas(*, track2_duration: int = 227) -> tuple[MetaMusic, list[MetaMusic]]:
    """
    构造第 2 首物理时长与线上记录显著不同的本地 OST 曲目线索。
    """
    album_meta = MetaMusic(
        title="Game OST",
        album="Game OST",
        artists=["OST Composer"],
        album_artist="OST Composer",
        year=2024,
        field_sources={"album": "tag", "album_artist": "tag"},
    )
    tracks = [
        MetaMusic(
            title="Opening Theme",
            album="Game OST",
            artists=["OST Composer"],
            album_artist="OST Composer",
            disc_number=1,
            track_number=1,
            duration=180,
            field_sources={"title": "tag", "album": "tag", "artists": "tag"},
        ),
        MetaMusic(
            title="Hidden Medley",
            album="Game OST",
            artists=["OST Composer"],
            album_artist="OST Composer",
            disc_number=1,
            track_number=2,
            duration=track2_duration,
            field_sources={"title": "tag", "album": "tag", "artists": "tag"},
        ),
    ]
    return album_meta, tracks


def test_domain_alignment_policy_and_source_candidate_guard():
    """
    领域层回落策略只在严格对位不完整时尝试忽略时长，忽略时长仍禁止无曲名目录误匹配。
    """
    album_meta, local_tracks = _ost_local_metas(track2_duration=227)
    remote_tracks = [
        MusicInfo(
            title="Opening Theme",
            artists=["OST Composer"],
            media_source=MediaSource.TheAudioDB,
            media_id="rec-1",
            disc_number=1,
            track_number=1,
            duration=180,
        ),
        MusicInfo(
            title="Hidden Medley",
            artists=["OST Composer"],
            media_source=MediaSource.TheAudioDB,
            media_id="rec-2",
            disc_number=1,
            track_number=2,
            duration=28,
        ),
    ]
    album = MusicAlbumInfo(
        title="Game OST",
        artists=["OST Composer"],
        release_date="2024-05-01",
        media_source=MediaSource.TheAudioDB,
        media_id="album-1",
        tracks=remote_tracks,
    )

    assert align_music_tracks_by_policy(local_tracks, remote_tracks, track_match="strict") == {0: 0}
    assert align_music_tracks_by_policy(local_tracks, remote_tracks, track_match="fallback_ignore_duration") == {
        0: 0,
        1: 1,
    }
    assert align_music_tracks_by_policy(local_tracks, remote_tracks, track_match="ignore_duration") == {0: 0, 1: 1}

    assert music_album_candidate_matches(album, album_meta, local_tracks, track_match="strict") is False
    assert music_album_candidate_matches(album, album_meta, local_tracks, track_match="ignore_duration") is True

    assert _verified_source_album([album], album_meta, local_tracks, track_match="strict") is None
    fallback_album = _verified_source_album([album], album_meta, local_tracks, track_match="fallback_ignore_duration")
    assert fallback_album is not None
    assert fallback_album.raw_data["track_match"] == "ignore_duration"

    # 非 MusicBrainz 来源严格命中多个候选时回落模式保持歧义。
    alt_album = deepcopy(album)
    alt_album.media_id = "album-2"
    _meta, normal_tracks = _ost_local_metas(track2_duration=28)
    assert _verified_source_album([album, alt_album], album_meta, normal_tracks, track_match="fallback_ignore_duration") is None

    weak_album_meta = MetaMusic(title="CD1", album="CD1")
    weak_tracks = [
        MetaMusic(title="Track 01", track_number=1, duration=180),
        MetaMusic(title="Track 02", track_number=2, duration=28),
    ]
    assert music_album_candidate_matches(album, weak_album_meta, weak_tracks, track_match="ignore_duration") is False


@pytest.mark.parametrize("asynchronous", [False, True])
@pytest.mark.parametrize(
    ("policy", "expected_release", "expected_track_match"),
    [
        ("strict", None, None),
        ("fallback_ignore_duration", "rel-ost-1", "ignore_duration"),
        ("ignore_duration", "rel-ost-1", "ignore_duration"),
    ],
)
def test_musicbrainz_match_album_respects_auto_track_match_policy(
    monkeypatch,
    asynchronous,
    policy,
    expected_release,
    expected_track_match,
):
    """
    MusicBrainz 同步和异步专辑匹配按配置执行严格、回落或忽略时长，回落不增加额外网络请求。
    """
    album_meta, tracks = _ost_local_metas(track2_duration=227)
    detail = _ost_release_detail()
    detail_calls: list[str] = []

    def request_json(path, params=None):
        """
        返回模拟的发行详情响应并统计请求次数。
        """
        detail_calls.append(path)
        assert path == "/release/rel-ost-1"
        return deepcopy(detail)

    monkeypatch.setitem(runtime._settings, "MUSIC_TRACK_MATCH", policy)
    monkeypatch.setattr(MusicBrainzModule, "_search_release_candidates", Mock(return_value=[{"id": "rel-ost-1"}]))
    monkeypatch.setattr(
        MusicBrainzModule, "_async_search_release_candidates", AsyncMock(return_value=[{"id": "rel-ost-1"}]),
    )
    monkeypatch.setattr(MusicBrainzModule, "_request_json", staticmethod(request_json))
    monkeypatch.setattr(MusicBrainzModule, "_async_request_json", AsyncMock(side_effect=request_json))
    monkeypatch.setattr(MusicBrainzModule, "_lookup_artist_aliases", Mock(return_value=[]))
    monkeypatch.setattr(MusicBrainzModule, "_async_lookup_artist_aliases", AsyncMock(return_value=[]))

    module = MusicBrainzModule()
    result = (
        asyncio.run(module.async_match_music_album(album_meta, tracks))
        if asynchronous
        else module.match_music_album(album_meta, tracks)
    )
    assert len(detail_calls) == 1
    if expected_release is None:
        assert result is None
    else:
        assert result is not None
        assert result.musicbrainz_release_id == expected_release
        assert result.raw_data["track_match"] == expected_track_match


def test_musicbrainz_fallback_never_bypasses_strict_ambiguity_or_unlabeled_tracks():
    """
    严格校验已判定多版本歧义时回落模式保持歧义；忽略时长模式不接受纯无标签曲目目录。
    """
    album_meta, normal_tracks = _ost_local_metas(track2_duration=28)
    ambiguous_details = [
        _ost_release_detail("rel-a", "rg-a", track2_length_ms=28_000, recording2_id="rec-a2"),
        _ost_release_detail("rel-b", "rg-b", track2_length_ms=28_000, recording2_id="rec-b2"),
    ]
    assert (
        MusicBrainzModule._select_release_match(
            album_meta, normal_tracks, ambiguous_details, track_match="fallback_ignore_duration",
        )
        is None
    )

    weak_tracks = [
        MetaMusic(title="Track 01", album="Game OST", artists=["OST Composer"], track_number=1, duration=180),
        MetaMusic(title="Track 02", album="Game OST", artists=["OST Composer"], track_number=2, duration=28),
    ]
    assert (
        MusicBrainzModule._select_release_match(
            album_meta, weak_tracks, [_ost_release_detail()], track_match="ignore_duration",
        )
        is None
    )
