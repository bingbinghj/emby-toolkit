# Adapted from MoviePilot v3 e0f17f5aafd2a2cdc56d4466e60c11d125c603c9 (GPL-3.0).
# Music-only host integration; upstream license: ../LICENSE.MoviePilot.
from __future__ import annotations
from dataclasses import dataclass
from typing import Optional, Union, List
from .types import MediaSource, MediaType

@dataclass
class MetaBase(object):
    isfile: bool = False
    title: Optional[str] = ''
    org_string: Optional[str] = None
    subtitle: Optional[str] = None
    type: MediaType = MediaType.UNKNOWN
    cn_name: Optional[str] = None
    en_name: Optional[str] = None
    original_name: Optional[str] = None
    year: Optional[Union[str, int]] = None
    total_season: int = 0
    begin_season: Optional[int] = None
    end_season: Optional[int] = None
    total_episode: int = 0
    begin_episode: Optional[int] = None
    end_episode: Optional[int] = None
    _explicit_episodes: Optional[List[int]] = None
    part: Optional[str] = None
    resource_type: Optional[str] = None
    resource_effect: Optional[str] = None
    resource_pix: Optional[str] = None
    resource_team: Optional[str] = None
    customization: Optional[str] = None
    web_source: Optional[str] = None
    video_encode: Optional[str] = None
    video_bit: Optional[str] = None
    audio_encode: Optional[str] = None
    apply_words: Optional[List[str]] = None
    media_source: Optional[MediaSource] = None
    media_id: Optional[str] = None
    episode_group: Optional[str] = None
    fps: Optional[int] = None
    _subtitle_flag = False
    _title_episodel_re = 'Episode\\s+(\\d{1,4})'
    _subtitle_season_re = '(?<!(?:不包含|不包括|不含)\\s*)(?<![全共]\\s*)[第\\s]+([0-9一二三四五六七八九十S\\-]+)\\s*季(?!\\s*[全共])'
    _subtitle_season_all_re = '[全共]\\s*([0-9一二三四五六七八九十]+)\\s*季'
    _subtitle_episode_re = '(?<!(?:不包含|不包括|不含)\\s*)(?<![全共]\\s*)[第\\s]+([0-9一二三四五六七八九十百零EP]+)\\s*[集话話期幕](?!\\s*[全共])'
    _subtitle_episode_between_re = '[第]*\\s*([0-9一二三四五六七八九十百零]+)\\s*[集话話期幕]?\\s*-\\s*第*\\s*([0-9一二三四五六七八九十百零]+)\\s*[集话話期幕]'
    _subtitle_episode_all_re = '([0-9一二三四五六七八九十百零]+)\\s*集\\s*全|[全共]\\s*([0-9一二三四五六七八九十百零]+)\\s*[集话話期幕]'

    def __init__(self, title: str, subtitle: Optional[str]=None, isfile: bool=False) -> None:
        """
        保存原始标题、辅助描述和文件识别上下文。

        :param title: 原始标题；空标题不写入识别状态
        :param subtitle: 可补充识别信息的副标题，未提供时为 None
        :param isfile: 标题是否来自文件路径
        """
        if not title:
            return
        self.org_string = title.strip() if title else None
        self.subtitle = subtitle.strip() if subtitle else None
        self.isfile = isfile
