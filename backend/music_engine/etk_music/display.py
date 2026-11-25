# Adapted from MoviePilot v3 e0f17f5 (GPL-3.0); see ../LICENSE.MoviePilot.
from __future__ import annotations
from copy import deepcopy
from typing import Any, Union, Iterable
from .models import MusicInfo, MusicAlbumInfo
from .types import MUSIC_ENTITY_ALBUM
from .text import convert as zhconv_convert
from .runtime import get_runtime_setting

class MusicDisplay:
    _music_simplified_text_fields = ("title","album","album_artist","album_type","version","metadata_category","comment","description")
    _music_simplified_list_fields = ("artists","secondary_types","genres","tags","names","composers","conductors","orchestras")

    @staticmethod
    def _contains_han(value: str) -> bool:
        """
        判断名称中是否包含可用于中文展示的汉字。
        """
        return any("\u3400" <= char <= "\u9fff" for char in value)

    @staticmethod
    def _chinese_aliases(artist: dict) -> list[str]:
        return [alias.get("name") for alias in artist.get("aliases") or []
                if isinstance(alias, dict) and (not alias.get("locale")
                    or str(alias["locale"]).lower().replace("_", "-").split("-")[0] == "zh")]

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
            aliases.extend(cls._chinese_aliases(artist))
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
    def _simplify_credit_name(cls, info: MusicInfo | MusicAlbumInfo, name: str) -> str:
        """复用当前资料里按身份绑定的别名；同名的不同人物不能相互替换。"""
        identities: dict[str, list[str]] = {}
        for index, artist in enumerate(info.artists or []):
            identity = info.artist_ids[index] if index < len(info.artist_ids or []) else f"credit-{index}"
            identities.setdefault(identity, []).extend([artist, *cls._music_artist_aliases(info, index)])
        raw = info.raw_data if isinstance(info.raw_data, dict) else {}
        relations = list(raw.get("relations") or [])
        for relation in list(relations):
            if isinstance(relation, dict) and relation.get("type") == "performance":
                relations.extend((relation.get("work") or {}).get("relations") or [])
        for relation in relations:
            artist = relation.get("artist") if isinstance(relation, dict) else None
            if isinstance(artist, dict) and artist.get("id"):
                identities.setdefault(artist["id"], []).extend([
                    relation.get("target-credit"), artist.get("name"),
                    *cls._chinese_aliases(artist),
                ])
        matches = [aliases for aliases in identities.values()
                   if name.casefold() in {str(value).casefold() for value in aliases if value}]
        return cls._simplify_music_artist_name(name, matches[0]) if len(matches) == 1 else str(zhconv_convert(name, "zh-hans"))

    @classmethod
    def _simplify_album_artist(cls, info: MusicInfo, value: str) -> str:
        """按专辑原始署名逐人转换合唱，不能按斜杠猜分人物或乐队名称。"""
        raw = info.raw_data if isinstance(info.raw_data, dict) else {}
        credits = [credit for credit in raw.get("album-artist-credit") or []
                   if isinstance(credit, dict) and isinstance(credit.get("artist"), dict)]
        names = [credit.get("name") or credit["artist"].get("name") or "" for credit in credits]
        if names and all(names) and value == " / ".join(names):
            return " / ".join(cls._simplify_music_artist_name(name, cls._chinese_aliases(credit["artist"]))
                              for name, credit in zip(names, credits))
        if info.artists and value == " / ".join(info.artists):
            return " / ".join(cls._simplify_music_artist_names(info))
        return cls._simplify_credit_name(info, value)

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
                if field_name == "album_artist":
                    converted = cls._simplify_album_artist(info, value)
                elif field_name in {"title", "album"}:
                    converted = cls._simplify_music_artist_name(value, getattr(info, field_name + "_aliases", None) or [])
                else:
                    converted = zhconv_convert(value, "zh-hans")
                if converted != value:
                    updates[field_name] = converted
        for field_name in cls._music_simplified_list_fields:
            value = getattr(info, field_name, None)
            if isinstance(value, list):
                converted_items = (
                    cls._simplify_music_artist_names(info)
                    if field_name == "artists"
                    else [cls._simplify_credit_name(info, item) for item in value]
                    if field_name in {"composers", "conductors", "orchestras"}
                    else [
                        zhconv_convert(item, "zh-hans")
                        if isinstance(item, str)
                        else item
                        for item in value
                    ]
                )
                if converted_items != value:
                    updates[field_name] = converted_items
        performers = {}
        for role, names in (info.performers or {}).items():
            converted_role = str(zhconv_convert(role, "zh-hans"))
            performers[converted_role] = list(dict.fromkeys([
                *performers.get(converted_role, []), *(cls._simplify_credit_name(info, name) for name in names),
            ]))
        if performers != info.performers:
            updates["performers"] = performers
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


def simplify_metadata_dict(value: dict) -> dict:
    """直连和 GD 资料保留各自 ID、扩展字段，只转换标签文本。"""
    if not get_runtime_setting("MUSIC_METADATA_TO_SIMPLIFIED", True):
        return value
    fields = (*MusicDisplay._music_simplified_text_fields, *MusicDisplay._music_simplified_list_fields,
              "performers", "artist_ids", "artist_aliases", "title_aliases", "album_aliases", "raw_data")
    info = MusicInfo(**{key: value[key] for key in fields if value.get(key) is not None})
    converted = MusicDisplay._simplify_recognized_music_info(info)
    result = {**value, **{key: getattr(converted, key) for key in fields if key in value}}
    if "artist" in value:
        result["artist"] = " / ".join(converted.artists) if converted.artists else zhconv_convert(value["artist"] or "", "zh-hans")
    if isinstance(value.get("tracks"), list):
        result["tracks"] = [simplify_metadata_dict(track) for track in value["tracks"]]
    return result
