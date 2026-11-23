"""MP v3.2.11 音乐名称解析回归。"""
import pytest
from etk_music.meta import MetaMusic


@pytest.mark.parametrize("title,expected,year,format", [
    ("薛之谦 (Joker Xue) - 尘 (Dust) [2019] [专辑] FLAC / Lossless / Log (100%) / Cue / CD / 自抓 / 禁转 / 2019", "尘 (Dust)", 2019, "FLAC"),
    ("薛之谦 (Joker Xue) - 尘 (Dust) [2019] [专辑] FLAC / Lossless / WEB / 自购 / 禁转 / 2019", "尘 (Dust)", 2019, "FLAC"),
    ("老狼 (Lao Lang) - 恋恋风尘 [1995] [专辑] WAV / Lossless / Log (78%) / Cue / CD / 1995 / 可替代", "恋恋风尘", 1995, "WAV"),
    ("夏曰入侵企画 (Summer Warz) - 尘星 [2024] [单曲专辑] FLAC / 24bit Lossless / WEB / 2024", "尘星", 2024, "FLAC"),
])
def test_transfer_specs_do_not_pollute_music_identity(title, expected, year, format):
    meta = MetaMusic.parse_resource(title)
    assert meta.title == expected
    assert meta.year == year
    assert meta.artists == [title.partition(" - ")[0]]
    assert meta.audio_format == format
    assert meta.audio_lossless is True


@pytest.mark.parametrize("title", ["Artist - CD / Log", "Artist - Love / Life"])
def test_real_title_slashes_are_preserved(title):
    assert MetaMusic.parse_resource(title).title == title.partition(" - ")[2]
