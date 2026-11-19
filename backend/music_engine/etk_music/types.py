# Adapted from MoviePilot v3 e0f17f5aafd2a2cdc56d4466e60c11d125c603c9 (GPL-3.0).
# Music-only host integration; upstream license: ../LICENSE.MoviePilot.
from __future__ import annotations
from enum import Enum
import re
from pydantic_core import CoreSchema
from pydantic import GetJsonSchemaHandler
from typing import Literal, Union, Optional
MEDIA_SOURCE_IDENTIFIER_PATTERN = r"^[a-z][a-z0-9_]*$"
_MEDIA_SOURCE_IDENTIFIER_RE = re.compile(MEDIA_SOURCE_IDENTIFIER_PATTERN)
_MEDIA_SOURCE_VALUE_ALIASES = {"tmdb": "themoviedb"}
class MediaType(Enum):
    """
    媒体条目的业务类型。
    """
    MOVIE = '电影'
    TV = '电视剧'
    MUSIC = '音乐'
    COLLECTION = '系列'
    UNKNOWN = '未知'

    @staticmethod
    def from_agent(key: str) -> Optional['MediaType']:
        """
        将 Agent 媒体类型转换为 MediaType。
        """
        _map = {'movie': MediaType.MOVIE, 'tv': MediaType.TV, 'music': MediaType.MUSIC}
        return _map.get(key.strip().lower() if key else '')

    def to_agent(self) -> str:
        """
        将 MediaType 转换为 Agent 使用的媒体类型。
        """
        return {MediaType.MOVIE: 'movie', MediaType.TV: 'tv', MediaType.MUSIC: 'music'}.get(self, self.value)

class MediaSource(str, Enum):
    """
    媒体主身份的数据来源，内置来源为常量，插件来源为动态扩展成员。
    """
    TMDB = 'themoviedb'
    Douban = 'douban'
    Bangumi = 'bangumi'
    AniList = 'anilist'
    IMDb = 'imdb'
    TVDB = 'tvdb'
    MusicBrainz = 'musicbrainz'
    TheAudioDB = 'theaudiodb'
    DoubanMusic = 'doubanmusic'

    def __str__(self) -> str:
        """
        返回可直接用于 API 和数据库的规范值。
        """
        return self.value

    @classmethod
    def _missing_(cls, value: object) -> Optional['MediaSource']:
        """
        将合法插件来源标识解析为动态枚举成员，并规范化内置别名。
        """
        if not isinstance(value, str):
            return None
        normalized = value.strip().casefold()
        normalized = _MEDIA_SOURCE_VALUE_ALIASES.get(normalized, normalized)
        known_member = cls._value2member_map_.get(normalized)
        if isinstance(known_member, cls):
            return known_member
        if not _MEDIA_SOURCE_IDENTIFIER_RE.fullmatch(normalized):
            return None
        member = str.__new__(cls, normalized)
        member._name_ = normalized
        member._value_ = normalized
        known_member = cls._value2member_map_.setdefault(normalized, member)
        if isinstance(known_member, cls):
            return known_member
        return None

    @classmethod
    def __get_pydantic_json_schema__(cls, core_schema: CoreSchema, handler: GetJsonSchemaHandler) -> dict[str, object]:
        """
        在 OpenAPI 中声明可扩展标识格式，避免把内置成员误写成完整白名单。
        """
        schema: dict[str, object] = handler(core_schema)
        schema.pop('enum', None)
        schema['pattern'] = MEDIA_SOURCE_IDENTIFIER_PATTERN
        schema['examples'] = [source.value for source in cls]
        return schema

class MediaRecognizeType(Enum):
    """
    媒体识别数据源。
    """
    Douban = '豆瓣'
    TMDB = 'TheMovieDb'
    TVDB = 'TheTvDb'
    Bangumi = 'Bangumi'
    AniList = 'AniList'
    IMDb = 'IMDb'
    MusicBrainz = 'MusicBrainz'
    TheAudioDB = 'TheAudioDB'

class ModuleType(Enum):
    """
    模块提供的主要服务类型。
    """
    Downloader = 'downloader'
    MediaServer = 'mediaserver'
    Notification = 'notification'
    MediaRecognize = 'mediarecognize'
    Indexer = 'indexer'
    Other = 'other'

class OtherModulesType(Enum):
    """
    标识字幕、歌词等不属于专用服务分类的模块。
    """
    Subtitle = '站点字幕'
    Fanart = 'Fanart'
    FileManager = '文件整理'
    Filter = '过滤器'
    Indexer = '站点索引'
    PostgreSQL = 'PostgreSQL'
    Redis = 'Redis'
    ListenBrainz = 'ListenBrainz'
    Lrclib = 'LRCLIB'
    Amll = 'AMLL TTML'
    AcoustId = 'AcoustID'
MUSIC_ENTITY_RECORDING = 'recording'
MUSIC_ENTITY_ALBUM = 'album'
MUSIC_ENTITY_ARTIST = 'artist'
MUSIC_ENTITY_TYPES = ('recording', 'album', 'artist')
MUSIC_SUBSCRIBABLE_TYPES = ('recording', 'album')
MusicEntityType = Literal['recording', 'album', 'artist']
MediaSourceSelection = Union[MediaSource, tuple[MediaSource, ...], list[MediaSource]]

LISTENBRAINZ_CHART_RANGES = ('this_week', 'this_month', 'this_year', 'week', 'month', 'quarter', 'half_yearly', 'year', 'all_time')

LISTENBRAINZ_FRESH_SORTS = ('release_date', 'artist_credit_name', 'release_name')

LISTENBRAINZ_FRESH_MAX_DAYS = 90
