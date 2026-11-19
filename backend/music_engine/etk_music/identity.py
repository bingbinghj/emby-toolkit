"""音乐来源身份保持成对传递，不与影视识别共用状态。"""
from .types import MediaSource
from typing import Any, Optional, Union, Tuple


def normalize_media_source(value):
    if isinstance(value, MediaSource):
        return value
    try:
        return MediaSource(str(value).strip().lower()) if value else None
    except ValueError:
        return None


def resolve_media_identity(
        media: Any = None,
        media_source: Optional[Union[MediaSource, str]] = None,
        media_id: Optional[Any] = None,
) -> Tuple[Optional[MediaSource], Optional[str]]:
    """
    从统一媒体对象或显式字段解析主媒体身份。

    :param media: 包含 ``media_source`` 和 ``media_id`` 的媒体对象
    :param media_source: 显式媒体来源
    :param media_id: 显式来源原生 ID
    :return: 枚举化来源和字符串 ID；任一字段无效时返回空身份
    """
    normalized_source = normalize_media_source(media_source)
    if media_source is not None or media_id is not None:
        normalized_id = str(media_id).strip() if media_id is not None else ""
        if normalized_source and normalized_id and normalized_id != "0":
            return normalized_source, normalized_id
        return None, None

    if media is None:
        return None, None
    normalized_source = normalize_media_source(
        getattr(media, "media_source", None)
        if not isinstance(media, dict)
        else media.get("media_source")
    )
    object_media_id = (
        getattr(media, "media_id", None)
        if not isinstance(media, dict)
        else media.get("media_id")
    )
    if normalized_source and object_media_id is not None:
        normalized_id = str(object_media_id).strip()
        if normalized_id and normalized_id != "0":
            return normalized_source, normalized_id
    return None, None
