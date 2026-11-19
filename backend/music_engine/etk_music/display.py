# Adapted from MoviePilot v3 e0f17f5 (GPL-3.0); see ../LICENSE.MoviePilot.
from __future__ import annotations
from copy import deepcopy
from typing import Any, Union, Iterable
from .models import MusicInfo, MusicAlbumInfo
from .types import MUSIC_ENTITY_ALBUM
from .text import convert as zhconv_convert
from .runtime import get_runtime_setting

class MusicDisplay:
    _music_simplified_text_fields = ("title","album","album_artist","album_type","version","metadata_category")
    _music_simplified_list_fields = ("artists","secondary_types","genres","tags","names")

    @staticmethod
    def _contains_han(value: str) -> bool:
        """
        判断名称中是否包含可用于中文展示的汉字。
        """
        return any("\u3400" <= char <= "\u9fff" for char in value)

    @classmethod
    def _music_artist_aliases(
        cls,
        info: Union[MusicInfo, MusicAlbumInfo],
        artist_index: int,
    ) -> list[str]:
        """
        按艺术家身份提取别名，避免多位艺术家的别名相互串用。
        """
        artists = list(info.artists or [])
        current_name = str(artists[artist_index] or "") if artist_index < len(artists) else ""
        artist_ids = list(info.artist_ids or [])
        artist_id = str(artist_ids[artist_index] or "") if artist_index < len(artist_ids) else ""
        raw_data = info.raw_data if isinstance(info.raw_data, dict) else {}
        credits = raw_data.get("artist-credit")
        for credit in credits if isinstance(credits, list) else []:
            if not isinstance(credit, dict):
                continue
            artist = credit.get("artist") or {}
            if not isinstance(artist, dict):
                continue
            if artist_id and str(artist.get("id") or "") != artist_id:
                continue
            credit_names = {
                str(value).strip()
                for value in (credit.get("name"), artist.get("name"))
                if str(value or "").strip()
            }
            if not artist_id and current_name not in credit_names:
                continue
            aliases = [credit.get("name"), artist.get("name")]
            aliases.extend(
                alias.get("name")
                for alias in artist.get("aliases") or []
                if isinstance(alias, dict)
            )
            return list(dict.fromkeys(
                str(alias).strip() for alias in aliases if str(alias or "").strip()
            ))
        if len(artists) == 1:
            return list(info.artist_aliases or [])
        return []

    @classmethod
    def _simplify_music_artist_name(
        cls,
        name: str,
        aliases: Iterable[str],
    ) -> str:
        """
        将艺术家名称转为简体，并在可信别名中优先选择汉字名称。
        """
        candidates = [name, *aliases]
        seen: set[str] = set()
        for candidate in candidates:
            text = str(candidate or "").strip()
            if not text or text.casefold() in seen:
                continue
            seen.add(text.casefold())
            simplified = zhconv_convert(text, "zh-hans")
            if cls._contains_han(simplified):
                return str(simplified)
        return str(zhconv_convert(name, "zh-hans"))

    @classmethod
    def _simplify_music_artist_names(cls, info: Union[MusicInfo, MusicAlbumInfo]) -> list[str]:
        """
        逐位转换艺术家名称，保留多艺术家结果的身份边界。
        """
        return [
            cls._simplify_music_artist_name(
                str(name),
                cls._music_artist_aliases(info, index),
            )
            for index, name in enumerate(info.artists or [])
        ]

    @classmethod
    def _simplify_recognized_music_info(cls, info: MusicInfo | MusicAlbumInfo) -> MusicInfo | MusicAlbumInfo:
        """
        按开关转换音乐及专辑曲目文本，保留来源缓存、原文别名和身份。
        """
        if not get_runtime_setting("MUSIC_METADATA_TO_SIMPLIFIED", True):
            return info
        updates: dict[str, Any] = {}
        for field_name in cls._music_simplified_text_fields:
            value = getattr(info, field_name, None)
            if isinstance(value, str):
                converted = zhconv_convert(value, "zh-hans")
                if converted != value:
                    updates[field_name] = converted
        for field_name in cls._music_simplified_list_fields:
            value = getattr(info, field_name, None)
            if isinstance(value, list):
                converted_items = (
                    cls._simplify_music_artist_names(info)
                    if field_name == "artists"
                    else [
                        zhconv_convert(item, "zh-hans")
                        if isinstance(item, str)
                        else item
                        for item in value
                    ]
                )
                if converted_items != value:
                    updates[field_name] = converted_items
        if isinstance(info, MusicAlbumInfo):
            tracks = [cls._simplify_recognized_music_info(track) for track in info.tracks]
            if any(converted is not original for converted, original in zip(tracks, info.tracks)):
                updates["tracks"] = tracks
        if not updates:
            return info
        simplified: MusicInfo | MusicAlbumInfo = deepcopy(info)
        alias_field: str
        for field_name, alias_field in (
            ("title", "title_aliases"),
            ("album", "album_aliases"),
            ("artists", "artist_aliases"),
            ("album_artist", "artist_aliases"),
            ("names", "title_aliases"),
        ):
            if field_name not in updates:
                continue
            if field_name in ("album_artist", "names") and info.music_type != MUSIC_ENTITY_ALBUM:
                continue
            original = getattr(info, field_name)
            originals: list[Any] = original if isinstance(original, list) else [original]
            setattr(simplified, alias_field, list(dict.fromkeys([
                *(getattr(simplified, alias_field, None) or []), *originals,
            ])))
        for field_name, value in updates.items():
            setattr(simplified, field_name, value)
        return simplified
