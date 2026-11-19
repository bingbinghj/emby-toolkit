# Adapted from MoviePilot v3 e0f17f5 (GPL-3.0); see ../LICENSE.MoviePilot.
from __future__ import annotations
import re, base64, hashlib, hmac
from urllib import parse
from datetime import datetime
from typing import Any, Optional, Iterable, List, cast
from bs4 import BeautifulSoup
from .models import MusicInfo, MusicAlbumInfo
from .meta import MetaMusic
from .types import MediaSource, MediaSourceSelection, MusicEntityType, MUSIC_ENTITY_RECORDING, MUSIC_ENTITY_ALBUM
from .sources import is_media_source_selected
from .http import RequestUtils
from .runtime import get_runtime_setting
from .cache import cached
from .observation import music_request_timeout, report_music_recognition
class DoubanMusic:
    _music_source = MediaSource.DoubanMusic
    def _require_api(self):
        return self


    def search_music(
            self,
            meta: MetaMusic,
            limit: int = 20,
            media_source: Optional[MediaSourceSelection] = None,
            music_types: Optional[Iterable[MusicEntityType]] = None,
    ) -> Optional[List[MusicInfo]]:
        """
        按请求来源搜索豆瓣音乐专辑，并兼容统一音乐搜索的实体过滤参数。

        :param meta: 待识别或搜索的影视、音乐元数据
        :param limit: 请求结果上限，音乐搜索限制在 1 至 100 条
        :param media_source: 本次请求允许的数据源；与对应豆瓣影视或音乐来源不符时不处理
        :param music_types: 兼容统一音乐搜索的实体类型参数，当前忽略，实际搜索专辑
        :return: 专辑候选列表；没有关键词或无匹配时为空列表，来源不适用时返回 None
        """
        del music_types
        if not is_media_source_selected(media_source, self._music_source):
            return None
        keyword = meta.album or meta.title
        if not keyword:
            return []
        result = self._require_api().music_search(keyword=keyword, count=max(1, min(limit, 100)))
        return self._build_music_search_results(result)


    def recognize_music(
            self,
            media_source: MediaSource,
            media_id: str,
            music_type: Optional[str] = None,
    ) -> Optional[MusicInfo]:
        """
        按豆瓣音乐原生 ID 和实体类型获取专辑或专辑内曲目详情。

        按计划选择显式身份或名称候选，影视识别准备阶段可能更新输入 meta 的类型。

        :param media_source: 本次请求允许的数据源；与对应豆瓣影视或音乐来源不符时不处理
        :param media_id: 来源原生标识；音乐曲目使用“专辑标识:曲目标识”形式
        :param music_type: album 表示专辑，recording 表示录音；未指定时结合身份判断
        :return: 匹配的标准影视或音乐对象；来源、类型或标识不适用以及未命中时返回 None
        """
        if media_source != self._music_source or not media_id:
            return None
        album_id, separator, track_id = str(media_id).partition(":")
        if music_type == MUSIC_ENTITY_RECORDING and not separator:
            return None
        if music_type == MUSIC_ENTITY_ALBUM and separator:
            return None
        album = self.music_album(media_source, album_id)
        if not album:
            return None
        if separator and track_id:
            return next(
                (
                    track for track in album.tracks
                    if track.media_id == media_id or str(track.track_number or "") == track_id
                ),
                None,
            )
        return album.to_music_info()


    def music_album(
            self,
            media_source: MediaSource,
            media_id: str,
    ) -> Optional[MusicAlbumInfo]:
        """
        按豆瓣音乐专辑 ID 获取标准化专辑详情和曲目。

        :param media_source: 本次请求允许的数据源；与对应豆瓣影视或音乐来源不符时不处理
        :param media_id: 来源原生标识；音乐曲目使用“专辑标识:曲目标识”形式
        :return: 包含专辑及曲目详情的对象；来源不适用或无详情时返回 None
        """
        if media_source != self._music_source or not media_id:
            return None
        info = self._require_api().music_detail(subject_id=str(media_id))
        return self._douban_music_to_album(info) if info else None


    def music_discover(
            self,
            media_source: MediaSource,
            page: int = 1,
            count: int = 30,
            entity: str = MUSIC_ENTITY_ALBUM,
            mode: str = "chart",
            tags: str = "",
            sort: str = "U",
    ) -> Optional[List[MusicInfo]]:
        """
        按官方新碟榜或标签交集浏览豆瓣音乐，并保留豆瓣条目原生身份。

        多标签仅在每个标签最多 300 条候选内求交集，不代表全部站点结果。

        :param media_source: 本次请求允许的数据源；与对应豆瓣影视或音乐来源不符时不处理
        :param page: 请求页码，从 1 开始
        :param count: 本次最多返回的条目数
        :param entity: 兼容统一音乐发现的实体类型参数，当前忽略，实际返回专辑
        :param mode: chart 使用新碟榜，其他值使用标签页
        :param tags: 逗号分隔的标签；音乐多标签按原生标识求交集，空值使用“流行”
        :param sort: 豆瓣排序代码，直接传递给对应接口
        :return: 豆瓣音乐专辑列表；无结果为空列表，来源不适用时返回 None
        """
        if media_source != self._music_source:
            return None
        del entity
        if mode == "chart":
            chart_items = self._build_music_search_results(self._require_api().music_chart())
            start = max(page - 1, 0) * max(1, count)
            return chart_items[start:start + max(1, count)]
        selected_tags = [tag.strip() for tag in str(tags or "").split(",") if tag.strip()]
        if not selected_tags:
            selected_tags = ["流行"]
        if len(selected_tags) == 1:
            result = self._require_api().music_tag(
                tag=selected_tags[0],
                start=max(page - 1, 0) * max(1, count),
                count=max(1, count),
                sort=sort,
            )
            return self._build_music_search_results(result)

        # 豆瓣官网的多标签 URL 会按完整文本标签匹配；分别读取后按原生 ID
        # 求交集，才能实现风格与地区的真实组合筛选。
        scan_count = min(max(page * count * 4, 100), 300)
        tag_results = [
            self._build_music_search_results(
                self._require_api().music_tag(tag=tag, start=0, count=scan_count, sort=sort)
            )
            for tag in selected_tags
        ]
        if not tag_results:
            return []
        shared_ids = set(item.media_id for item in tag_results[0])
        for items in tag_results[1:]:
            shared_ids.intersection_update(item.media_id for item in items)
        matched = [item for item in tag_results[0] if item.media_id in shared_ids]
        start = max(page - 1, 0) * max(1, count)
        return matched[start:start + max(1, count)]


    def music_album_related(
            self,
            media_source: MediaSource,
            media_id: str,
            count: int = 24,
    ) -> Optional[List[MusicInfo]]:
        """
        按豆瓣音乐专辑 ID 返回相关推荐条目。

        :param media_source: 本次请求允许的数据源；与对应豆瓣影视或音乐来源不符时不处理
        :param media_id: 来源原生标识；音乐曲目使用“专辑标识:曲目标识”形式
        :param count: 本次最多返回的条目数
        :return: 豆瓣音乐专辑列表；无结果为空列表，来源不适用时返回 None
        """
        if media_source != self._music_source or not media_id:
            return None
        result = self._require_api().music_recommendations(
            subject_id=str(media_id),
            start=0,
            count=max(1, count),
        )
        return self._build_music_search_results(result)


    @classmethod
    def _build_music_search_results(
            cls,
            result: Optional[dict[str, Any] | list[Any]],
    ) -> List[MusicInfo]:
        """
        把豆瓣音乐搜索响应转换为专辑候选列表。
        """
        payload = result or {}
        if isinstance(payload, list):
            items = payload
        else:
            items = (
                payload.get("subject_collection_items")
                or payload.get("recommendations")
                or payload.get("subjects")
                or payload.get("items")
                or payload.get("musics")
                or []
            )
        candidates = []
        for item in items:
            if not isinstance(item, dict):
                continue
            target_type = str(item.get("target_type") or "").casefold()
            if isinstance(item.get("target"), dict):
                target = item["target"]
            elif isinstance(item.get("subject"), dict):
                target = item["subject"]
            else:
                target = item
            type_name = str(target.get("type_name") or target.get("subtype") or "")
            target_subject_type = str(target.get("type") or "").casefold()
            if target_type and target_type not in {"music", "音乐", "subject"}:
                continue
            if target_subject_type and target_subject_type not in {"music", "音乐"}:
                continue
            if type_name and type_name not in {"音乐", "music"}:
                continue
            media_id = cls._douban_music_text(
                target.get("id") or item.get("target_id") or item.get("id")
            )
            title = cls._douban_music_text(target.get("title") or target.get("name"))
            if not media_id or not title:
                continue
            artists = cls._douban_music_search_artists(target)
            release_date = cls._douban_music_date(target)
            cover_url = cls._douban_music_cover(target)
            candidate = MusicInfo(
                media_source=cls._music_source,
                media_id=media_id,
                music_type=MUSIC_ENTITY_ALBUM,
                title=title,
                artists=artists,
                album=title,
                album_artist=" / ".join(artists) or None,
                album_id=media_id,
                year=cls._douban_music_year(target.get("year") or release_date),
                release_date=release_date,
                cover_url=cover_url,
                names=[title],
                detail_link=f"https://music.douban.com/subject/{media_id}/",
                raw_data={
                    "rating": cls._douban_music_float(target["rating"].get("value")),
                    "rating_votes": cls._douban_music_int(target["rating"].get("count")),
                } if isinstance(target.get("rating"), dict) else {},
            )
            candidates.append(candidate)
        return candidates


    @classmethod
    def _douban_music_to_album(cls, info: dict[str, Any]) -> Optional[MusicAlbumInfo]:
        """
        把豆瓣音乐详情转换为标准专辑信息和曲目。
        """
        media_id = cls._douban_music_text(info.get("id") or info.get("subject_id"))
        title = cls._douban_music_text(info.get("title") or info.get("name"))
        if not media_id or not title:
            return None
        attrs = cast(
            dict[str, Any],
            info.get("attrs") if isinstance(info.get("attrs"), dict) else {},
        )
        artists = cls._douban_music_artists(info)
        release_date = cls._douban_music_date(info)
        tags = [
            cls._douban_music_text(item.get("name") if isinstance(item, dict) else item)
            for item in (info.get("tags") or [])
        ]
        genres = [str(item) for item in info.get("genres") or [] if item]
        rating = cast(
            dict[str, Any],
            info.get("rating") if isinstance(info.get("rating"), dict) else {},
        )
        album = MusicAlbumInfo(
            media_source=cls._music_source,
            media_id=media_id,
            title=title,
            artists=artists,
            album_type=cls._douban_music_first(
                info.get("media") or attrs.get("media")
            ) or "Album",
            release_date=release_date,
            cover_url=cls._douban_music_cover(info),
            genres=genres,
            tags=[item for item in tags if item],
            rating=cls._douban_music_float(rating.get("value") or rating.get("average")),
            rating_votes=cls._douban_music_int(
                rating.get("count") or rating.get("numRaters") or info.get("ratings_count")
            ),
            detail_link=f"https://music.douban.com/subject/{media_id}/",
            raw_data={
                "overview": cls._douban_music_text(info.get("intro") or info.get("summary")),
                "publisher": cls._douban_music_first(
                    info.get("publisher") or attrs.get("publisher")
                ),
            },
        )
        album.tracks = cls._douban_music_tracks(info, album)
        return album


    @classmethod
    def _douban_music_tracks(
            cls,
            info: dict[str, Any],
            album: MusicAlbumInfo,
    ) -> List[MusicInfo]:
        """
        从豆瓣新旧响应结构中提取专辑曲目。
        """
        attrs = cast(
            dict[str, Any],
            info.get("attrs") if isinstance(info.get("attrs"), dict) else {},
        )
        # Frodo 当前音乐详情使用 songs；tracks/attrs.tracks 兼容旧接口响应。
        tracks = info.get("songs") or info.get("tracks") or attrs.get("tracks") or []
        if isinstance(tracks, str):
            tracks = tracks.splitlines()
        elif not isinstance(tracks, list):
            tracks = []
        elif len(tracks) == 1 and isinstance(tracks[0], str) and "\n" in tracks[0]:
            tracks = tracks[0].splitlines()
        results = []
        for index, item in enumerate(tracks, start=1):
            if isinstance(item, dict):
                title = cls._douban_music_text(item.get("title") or item.get("name"))
                track_number = cls._douban_music_int(item.get("track_number") or item.get("position")) or index
                duration = cls._douban_music_int(item.get("duration"))
                duration = duration if duration and duration > 0 else None
                disc_number = cls._douban_music_int(
                    item.get("disc_number") or item.get("disc")
                )
                artists = cls._douban_music_artists(item) or list(album.artists)
                cover_url = cls._douban_music_text(item.get("cover_url")) or album.cover_url
                raw_data = {
                    key: value
                    for key, value in {
                        "apple_album_id": item.get("apple_album_id"),
                        "apple_track_id": item.get("apple_track_id"),
                        "preview_url": item.get("preview_url"),
                    }.items()
                    if value not in (None, "")
                }
            else:
                title = cls._clean_douban_track_title(item)
                track_number = index
                duration = None
                disc_number = None
                artists = list(album.artists)
                cover_url = album.cover_url
                raw_data = {}
            if not title:
                continue
            results.append(MusicInfo(
                media_source=cls._music_source,
                # 豆瓣歌曲没有独立 subject ID，使用专辑内绝对顺序避免多碟曲序重复。
                media_id=f"{album.media_id}:{index}",
                title=title,
                artists=artists,
                album=album.title,
                album_artist=album.artist or None,
                album_id=album.media_id,
                album_type=album.album_type,
                secondary_types=list(album.secondary_types),
                year=album.year,
                release_date=album.release_date,
                disc_number=disc_number,
                track_number=track_number,
                duration=duration,
                cover_url=cover_url,
                metadata_category=album.metadata_category,
                genres=list(album.genres),
                tags=list(album.tags),
                artist_country=album.artist_country,
                release_status=album.release_status,
                names=[title],
                detail_link=album.detail_link,
                raw_data=raw_data,
            ))
        for track in results:
            track.total_tracks = len(results)
        return results


    @classmethod
    def _douban_music_artists(cls, info: dict[str, Any]) -> List[str]:
        """
        从豆瓣新旧响应结构中提取艺术家名称。
        """
        attrs = cast(
            dict[str, Any],
            info.get("attrs") if isinstance(info.get("attrs"), dict) else {},
        )
        values = (
            info.get("artists")
            or info.get("artist_names")
            or info.get("author")
            or info.get("singer")
            or attrs.get("singer")
            or []
        )
        if isinstance(values, str):
            values = [values]
        artists = []
        seen = set()
        for item in values:
            value = item.get("name") if isinstance(item, dict) else item
            text = cls._douban_music_text(value)
            identity = MetaMusic.compact_text(text) if text else ""
            if not text or identity in seen:
                continue
            seen.add(identity)
            artists.append(text)
        return artists


    @classmethod
    def _douban_music_search_artists(cls, info: dict[str, Any]) -> List[str]:
        """
        提取搜索候选艺术家，缺少结构化字段时回退到卡片副标题首段。
        """
        artists = cls._douban_music_artists(info)
        if artists:
            return artists
        subtitle = cls._douban_music_text(info.get("card_subtitle"))
        if not subtitle:
            return []
        artist = re.split(r"\s+/\s+", subtitle, maxsplit=1)[0].strip()
        if not artist or re.fullmatch(r"\d{4}(?:-\d{1,2}(?:-\d{1,2})?)?", artist):
            return []
        return [artist]


    @classmethod
    def _douban_music_cover(cls, info: dict[str, Any]) -> Optional[str]:
        """
        从豆瓣多种图片字段中提取清晰封面。
        """
        pic = info.get("pic") if isinstance(info.get("pic"), dict) else {}
        cover = info.get("cover") if isinstance(info.get("cover"), dict) else {}
        cover_img = info.get("cover_img") if isinstance(info.get("cover_img"), dict) else {}
        return next(
            (
                text for value in [
                    pic.get("large"),
                    cover_img.get("url"),
                    cover.get("large"),
                    cover.get("normal"),
                    cover.get("url"),
                    info.get("cover_url"),
                    info.get("image"),
                ]
                if (text := cls._douban_music_text(value))
            ),
            None,
        )


    @classmethod
    def _douban_music_date(cls, info: dict[str, Any]) -> Optional[str]:
        """
        从豆瓣新旧响应结构中提取首个发行日期。
        """
        attrs = cast(
            dict[str, Any],
            info.get("attrs") if isinstance(info.get("attrs"), dict) else {},
        )
        return cls._douban_music_first(info.get("pubdate") or attrs.get("pubdate"))


    @staticmethod
    def _douban_music_text(value: Any) -> Optional[str]:
        """
        把豆瓣外部响应值转换为去空白文本。
        """
        text = str(value).strip() if value is not None else ""
        return text or None


    @classmethod
    def _douban_music_first(cls, value: Any) -> Optional[str]:
        """
        从豆瓣列表或标量字段中提取首个文本。
        """
        if isinstance(value, list):
            return next((text for item in value if (text := cls._douban_music_text(item))), None)
        return cls._douban_music_text(value)


    @staticmethod
    def _douban_music_int(value: Any) -> Optional[int]:
        """
        将豆瓣外部响应值安全转换为整数。
        """
        try:
            return int(value) if value not in (None, "") else None
        except (TypeError, ValueError):
            return None


    @staticmethod
    def _douban_music_float(value: Any) -> float:
        """
        将豆瓣外部评分安全转换为浮点数。
        """
        try:
            return float(value) if value not in (None, "") else 0.0
        except (TypeError, ValueError):
            return 0.0


    @classmethod
    def _douban_music_year(cls, value: Any) -> Optional[int]:
        """
        从豆瓣年份或日期文本中提取四位年份。
        """
        text = cls._douban_music_text(value)
        return int(text[:4]) if text and text[:4].isdigit() else None


    @cached(ttl=3600, skip_none=True)
    def music_tag(
            self,
            tag: str,
            start: int = 0,
            count: int = 20,
            sort: str = "U",
    ) -> dict[str, Any]:
        """
        从豆瓣音乐官方标签页读取专辑，并适配为统一的音乐条目列表。

        官网每页 20 条，按所需偏移连续读取足够页；后续页失败时可能返回已收集的部分结果。

        :param tag: 单个音乐标签名称
        :param start: 从 0 开始的条目偏移量，不是页码
        :param count: 本次最多请求的条目数
        :param sort: 豆瓣排序代码，按接口支持值传递；默认值依各入口而定
        :return: 含 items 列表的字典；无效标签或没有可解析条目时列表为空
        """
        normalized_tag = str(tag or "").strip()
        if not normalized_tag:
            return {"items": []}
        page_size = 20
        first_page = max(start, 0) // page_size
        first_offset = max(start, 0) % page_size
        required = first_offset + max(count, 1)
        items: list[dict[str, Any]] = []
        page = first_page
        while len(items) < required:
            url = f"{self._music_web_url}/tag/{parse.quote(normalized_tag, safe='')}"
            response = RequestUtils(
                ua=get_runtime_setting('NORMAL_USER_AGENT'),
                proxies=get_runtime_setting('PROXY'),
                timeout=20,
                accept_type="text/html,application/xhtml+xml",
            ).get_res(url=url, params={"start": page * page_size, "type": sort})
            page_items = self._parse_music_tag_page(response.content if response else b"")
            if not page_items:
                break
            items.extend(page_items)
            if len(page_items) < page_size:
                break
            page += 1
        return {"items": items[first_offset:first_offset + max(count, 1)]}


    @cached(ttl=3600, skip_none=True)
    def music_chart(self) -> dict[str, Any]:
        """
        从豆瓣音乐官方榜单页读取新碟榜，并补充专辑详情供卡片展示。

        逐个补充专辑详情；详情不可用时保留榜单摘要。

        :return: 含新碟榜 items 的字典；无榜单条目时为空列表
        """
        response = RequestUtils(
            ua=get_runtime_setting('NORMAL_USER_AGENT'),
            proxies=get_runtime_setting('PROXY'),
            timeout=20,
            accept_type="text/html,application/xhtml+xml",
        ).get_res(url=f"{self._music_web_url}/chart")
        chart_items = self._parse_music_chart_page(response.content if response else b"")
        items: list[dict[str, Any]] = []
        for chart_item in chart_items:
            detail = self.music_detail(subject_id=chart_item["id"])
            if isinstance(detail, dict) and detail:
                detail.setdefault("id", chart_item["id"])
                detail.setdefault("title", chart_item["title"])
                if chart_item.get("artists") and not detail.get("artists"):
                    detail["artists"] = chart_item["artists"]
                items.append(detail)
            else:
                items.append(chart_item)
        return {"items": items}


    @staticmethod
    def _parse_music_tag_page(content: bytes) -> list[dict[str, Any]]:
        """
        解析豆瓣音乐标签页中的专辑 ID、封面、发行信息和评分。
        """
        if not content:
            return []
        soup = BeautifulSoup(content, "html.parser")
        items: list[dict[str, Any]] = []
        for row in soup.select("tr.item[id]"):
            media_id = str(row.get("id") or "").strip()
            title_link = row.select_one("div.pl2 > a[href*='/subject/']")
            if not media_id or not title_link:
                continue
            title = next(title_link.stripped_strings, "").strip()
            metadata = row.select_one("div.pl2 > p.pl")
            metadata_text = metadata.get_text(" ", strip=True) if metadata else ""
            parts = [part.strip() for part in metadata_text.split("/") if part.strip()]
            release_date = next(
                (part for part in parts if re.fullmatch(r"\d{4}(?:-\d{1,2}(?:-\d{1,2})?)?", part)),
                None,
            )
            image = row.select_one("a.nbg img")
            rating_node = row.select_one("span.rating_nums")
            votes_node = row.select_one("div.star span.pl")
            votes_match = re.search(r"(\d+)\s*人评价", votes_node.get_text(" ", strip=True) if votes_node else "")
            item = {
                "id": media_id,
                "type": "music",
                "title": title,
                "artists": [{"name": parts[0]}] if parts else [],
                "pubdate": [release_date] if release_date else [],
                "year": release_date[:4] if release_date else None,
                "cover_url": image.get("src") if image else None,
                "rating": {
                    "value": rating_node.get_text(strip=True) if rating_node else None,
                    "count": votes_match.group(1) if votes_match else None,
                },
            }
            items.append(item)
        return items


    @staticmethod
    def _parse_music_chart_page(content: bytes) -> list[dict[str, Any]]:
        """
        解析豆瓣新碟榜的专辑 ID、标题和艺术家。
        """
        if not content:
            return []
        soup = BeautifulSoup(content, "html.parser")
        heading = next(
            (node for node in soup.select("h2") if "豆瓣新碟榜" in node.get_text()),
            None,
        )
        container = heading.parent if heading else None
        items: list[dict[str, Any]] = []
        for row in container.select("ul.col3 li") if container else []:
            link = row.select_one("p.entry a[href*='/subject/']")
            if not link:
                continue
            match = re.search(r"/subject/(\d+)", str(link.get("href") or ""))
            if not match:
                continue
            entry_text = row.select_one("p.entry").get_text(" ", strip=True)
            artist = entry_text.split("/", 1)[1].strip() if "/" in entry_text else ""
            items.append({
                "id": match.group(1),
                "type": "music",
                "title": link.get_text(" ", strip=True),
                "artists": [{"name": artist}] if artist else [],
            })
        return items

    _music_web_url = 'https://music.douban.com'

    @cached(ttl=3600, skip_none=True, empty_ttl=300, empty_if=lambda x: not x)
    def _music_get(self, path, **params):
        url = 'https://frodo.douban.com/api/v2' + path
        stamp = params.pop('_ts', datetime.now().strftime('%Y%m%d'))
        raw = '&'.join(['GET', parse.quote(parse.urlparse(url).path, safe=''), stamp])
        signature = base64.b64encode(hmac.new(b'bf7dddc7c9cfe6f7', raw.encode(), hashlib.sha1).digest()).decode()
        params.update(apiKey='0dad551ec0f84ed02907ff5c42e8ec70', os_rom='android', _ts=stamp, _sig=signature)
        timeout = music_request_timeout(maximum=20)
        if timeout is None:
            return None
        response = RequestUtils(ua='api-client/1 com.douban.frodo/7.18.0(230) Android/29').get_res(url, params=params, timeout=timeout)
        if response is not None and response.status_code==404:
            return {}
        if response is None or response.status_code!=200:
            report_music_recognition('service_error', '豆瓣音乐服务暂时不可用')
            return None
        value = response.json()
        if not isinstance(value,dict) or value.get('code') not in (None,0,'0'):
            report_music_recognition('service_error', '豆瓣音乐返回了无效响应')
            return None
        return value

    def music_search(self, keyword, count=20):
        return self._music_get('/search/subjects', type='music', q=keyword, start=0, count=count)

    def music_detail(self, subject_id):
        return self._music_get('/music/'+str(subject_id))

    def music_recommendations(self, subject_id, count=24):
        return self._music_get('/music/'+str(subject_id)+'/recommendations', count=count)
