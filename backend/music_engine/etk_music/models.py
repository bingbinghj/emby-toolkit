# Adapted from MoviePilot v3 e0f17f5aafd2a2cdc56d4466e60c11d125c603c9 (GPL-3.0).
# Music-only host integration; upstream license: ../LICENSE.MoviePilot.
from __future__ import annotations
import re
import typing
from collections.abc import Mapping
from dataclasses import asdict, dataclass, field, fields
from typing import Any, Optional, Self
import yaml
from yaml.constructor import ConstructorError
from yaml.events import AliasEvent
from yaml.nodes import MappingNode
from .meta import MetaMusic, audio_quality_score, audio_quality_tier, format_audio_quality, infer_audio_lossless, music_credit_values, normalize_audio_format
from .category import ClassificationFactValue, ClassificationResult
from .identity import normalize_media_source, resolve_media_identity
from .types import MUSIC_ENTITY_ALBUM, MUSIC_ENTITY_ARTIST, MUSIC_ENTITY_RECORDING, MediaSource, MediaType
class _LyricsfileSafeLoader(yaml.SafeLoader):
    """
    在 SafeLoader 基础上拒绝 Lyricsfile 规范禁止的重复映射键。
    """

    def construct_mapping(
        self, node: MappingNode, deep: bool = False
    ) -> dict[Any, Any]:
        """
        构造映射并在解析阶段拒绝重复键。
        """
        if not isinstance(node, MappingNode):
            raise ConstructorError(None, None, "Lyricsfile 映射节点无效", node.start_mark)
        keys = set()
        for key_node, _value_node in node.value:
            key = self.construct_object(key_node, deep=deep)
            try:
                duplicated = key in keys
            except TypeError as err:
                raise ConstructorError(None, None, "Lyricsfile 映射键必须可哈希", key_node.start_mark) from err
            if duplicated:
                raise ConstructorError(None, None, f"Lyricsfile 存在重复键：{key}", key_node.start_mark)
            keys.add(key)
        return typing.cast(
            dict[Any, Any], super().construct_mapping(node, deep=deep)
        )


def _validate_music_type(value: object) -> None:
    """
    校验音乐模型类型字段，仅接受音乐或空值。
    """
    if value in {None, MediaType.MUSIC, MediaType.MUSIC.value, "music"}:
        return
    raise ValueError(f"不支持的音乐媒体类型：{value}")


def _music_string_list(value: object) -> list[str]:
    """
    将音乐标签原始值归一为非空字符串列表，兼容单值、列表与逗号分隔。
    """
    if value is None:
        return []
    if isinstance(value, str):
        return [value] if value else []
    if isinstance(value, (list, tuple, set)):
        return [str(item) for item in value if str(item)]
    return [str(value)]


def _music_aligned_list(value: object) -> list[str]:
    """
    保留原始位置的字符串列表，用于与艺术家名称按下标对应的 ID 列表。
    """
    if value is None:
        return []
    if isinstance(value, (list, tuple)):
        return [str(item or "") for item in value]
    return [str(value or "")]


def _music_optional_int(value: object) -> int | None:
    """
    将音乐技术参数安全转换为整数，空值与非数字返回 None。
    """
    if value in {None, ""}:
        return None
    try:
        return int(str(value))
    except (TypeError, ValueError):
        return None


def _music_optional_float(value: object) -> float:
    """
    将音乐评分安全转换为浮点数，空值与异常返回 0.0。
    """
    if value in {None, ""}:
        return 0.0
    try:
        return float(str(value))
    except (TypeError, ValueError):
        return 0.0


def _music_year_of(release_date: object) -> int | None:
    """
    从 MusicBrainz 可变精度日期（YYYY / YYYY-MM / YYYY-MM-DD）提取年份。
    """
    text = str(release_date or "")[:4]
    return int(text) if text.isdigit() else None


def _music_init_values(model: type, data: dict[str, Any]) -> dict[str, Any]:
    """
    按 dataclass 可初始化字段过滤字典，避免传入非法构造参数。
    """
    init_names = {item.name for item in fields(model) if item.init}
    return {key: value for key, value in data.items() if key in init_names}


def _classification_result(value: object) -> ClassificationResult | None:
    """
    将分类结果字典恢复为独立模型，避免领域对象共享可变标签列表。
    """
    if value is None:
        return None
    if isinstance(value, ClassificationResult):
        return value.model_copy(deep=True)
    if isinstance(value, Mapping):
        return ClassificationResult.model_validate(dict(value))
    raise TypeError("分类结果必须是 ClassificationResult、字典或空值")


def _classification_payload(value: ClassificationResult | None) -> dict[str, Any] | None:
    """
    把分类结果转换为可持久化字典。
    """
    return value.model_dump(mode="json") if value else None


def _music_category_tokens(value: object) -> list[str]:
    """
    拆分并规范化音乐来源分类文本，供旧载荷语义判定。
    """
    return [
        re.sub(r"[\s_-]+", " ", part).strip().casefold()
        for part in re.split(r"[/\|,;、，]+", str(value or ""))
        if part.strip()
    ]


def _legacy_music_category_is_metadata(
    category: object,
    *,
    album_type: object = None,
    secondary_types: object = None,
    genres: object = None,
    tags: object = None,
    classification: ClassificationResult | None = None,
) -> bool:
    """
    判断旧音乐 category 是否是来源描述，而不是人工库分类。

    旧音乐模型曾把专辑类型、流派写入 category，同时订阅人工分类也可能
    覆盖同一字段。优先用分类结果确认库分类；其余仅在文本能由标准音乐
    元数据解释时判为来源描述，无法解释时保守保留为库分类。
    """
    category_tokens = _music_category_tokens(category)
    if not category_tokens:
        return False

    selection = None
    if classification:
        selection = classification.effective or classification.recommended
    if selection and selection.category_path:
        category_text = " / ".join(category_tokens)
        path_tokens = _music_category_tokens(" / ".join(selection.category_path))
        if category_text in {
            " / ".join(path_tokens),
            path_tokens[-1] if path_tokens else "",
        }:
            return False

    evidence = [str(album_type or "")]
    for value in (secondary_types, genres, tags):
        evidence.extend(_music_string_list(value))
    evidence_tokens = {
        token
        for value in evidence
        for token in _music_category_tokens(value)
    }
    category_text = " ".join(category_tokens)
    if any(
        token and (token in category_text or category_text in token)
        for token in evidence_tokens
    ):
        return True

    source_descriptors = {
        "album",
        "ep",
        "single",
        "broadcast",
        "other",
        "live",
        "compilation",
        "soundtrack",
        "spoken word",
        "interview",
        "audiobook",
        "audio book",
        "audio drama",
        "remix",
        "dj mix",
        "mixtape",
        "street",
        "demo",
        "person",
        "group",
        "orchestra",
        "choir",
        "character",
    }
    return all(token in source_descriptors for token in category_tokens)


def _resolve_music_categories(
    *,
    category: object,
    library_category: object,
    metadata_category: object,
    album_type: object = None,
    secondary_types: object = None,
    genres: object = None,
    tags: object = None,
    classification: ClassificationResult | None = None,
) -> tuple[str, str]:
    """
    解析新旧音乐分类字段，新字段存在时禁止来源描述升级为库分类。
    """
    compatible = str(category or "").strip()
    library = str(library_category or "").strip()
    metadata = str(metadata_category or "").strip()
    if library or metadata:
        return library or compatible, metadata
    if not compatible:
        return "", ""
    if _legacy_music_category_is_metadata(
        compatible,
        album_type=album_type,
        secondary_types=secondary_types,
        genres=genres,
        tags=tags,
        classification=classification,
    ):
        return "", compatible
    return compatible, ""


@dataclass
class MusicLyrics:
    """
    标准化单曲歌词候选，保留来源匹配度和 Lyricsfile 原始内容。

    :param provider: 歌词提供方名称
    :param provider_id: 提供方内的歌词或曲目标识；未提供时为 None
    :param instrumental: 是否明确标记为纯音乐
    :param plain_lyrics: 不含时间轴的歌词；未提供时为 None
    :param synced_lyrics: 带时间轴的歌词；未提供时为 None
    :param lyricsfile: 保留提供方的 Lyricsfile 原始内容
    :param language: 歌词语言；未知时为 None
    :param match_score: 与目标曲目的匹配得分，用于候选排序
    :param provider_priority: 提供方排序优先级，用于同分候选选择
    """

    provider: str
    provider_id: str | None = None
    instrumental: bool = False
    plain_lyrics: str | None = None
    synced_lyrics: str | None = None
    lyricsfile: str | None = None
    language: str | None = None
    match_score: int = 0
    provider_priority: int = 0

    _lyricsfile_max_bytes = 1024 * 1024
    _lyricsfile_max_lines = 10000
    _lyricsfile_max_nodes = 50000
    _lyricsfile_max_depth = 20

    def __post_init__(self) -> None:
        """
        补全 Lyricsfile 中可安全降级为 LRC 或纯文本的内容。
        """
        if not self.lyricsfile:
            return
        parsed = self._parse_lyricsfile(self.lyricsfile)
        if not parsed:
            return
        metadata_value = parsed.get("metadata")
        metadata: dict[str, Any] = (
            metadata_value if isinstance(metadata_value, dict) else {}
        )
        self.instrumental = self.instrumental or bool(
            metadata.get("instrumental", parsed.get("instrumental"))
        )
        self.language = self.language or self._optional_text(
            metadata.get("language", parsed.get("language"))
        )
        if not self.synced_lyrics:
            self.synced_lyrics = self._lyricsfile_to_lrc(parsed)
        if not self.plain_lyrics:
            self.plain_lyrics = self._lyricsfile_to_plain(parsed)

    @property
    def content(self) -> str | None:
        """
        优先返回同步歌词，不存在时回退到纯文本歌词。
        """
        return self.synced_lyrics or self.plain_lyrics

    @property
    def extension(self) -> str | None:
        """
        根据歌词内容返回适合播放器扫描的旁挂文件扩展名。
        """
        if self.synced_lyrics:
            return ".lrc"
        if self.plain_lyrics:
            return ".txt"
        return None

    @property
    def quality_rank(self) -> int:
        """
        返回可比较的歌词质量等级，逐字同步高于逐行同步和纯文本。
        """
        if self.lyricsfile and self._lyricsfile_has_words(self.lyricsfile):
            return 4
        if self.synced_lyrics:
            return 3
        if self.plain_lyrics:
            return 1
        if self.instrumental:
            return 1
        return 0

    @property
    def identity_key(self) -> tuple[str, str, str]:
        """
        构造候选去重键，兼容未提供来源 ID 的插件结果。
        """
        return (
            self.provider.casefold(),
            self.provider_id or "",
            self.content or self.lyricsfile or "",
        )

    @classmethod
    def _parse_lyricsfile(cls, content: str) -> dict[str, Any] | None:
        """
        安全解析受大小和行数约束的 Lyricsfile YAML，并拒绝锚点别名。
        """
        if len(content.encode("utf-8")) > cls._lyricsfile_max_bytes:
            return None
        try:
            if any(isinstance(event, AliasEvent) for event in yaml.parse(content)):
                return None
            payload = yaml.load(content, Loader=_LyricsfileSafeLoader)
        except yaml.YAMLError:
            return None
        if not isinstance(payload, dict):
            return None
        if payload.get("version") != "1.0" or not isinstance(payload.get("metadata"), dict):
            return None
        if not cls._lyricsfile_value_is_safe(payload):
            return None
        lines = payload.get("lines")
        if isinstance(lines, list) and len(lines) > cls._lyricsfile_max_lines:
            return None
        return payload if cls._lyricsfile_structure_is_valid(payload) else None

    @classmethod
    def _lyricsfile_structure_is_valid(cls, payload: dict[str, Any]) -> bool:
        """
        校验 Lyricsfile 1.0 的必需元数据、歌词形状和毫秒时间范围。
        """
        metadata = payload.get("metadata")
        if not isinstance(metadata, dict):
            return False
        if not cls._optional_text(metadata.get("title")) or not cls._optional_text(metadata.get("artist")):
            return False
        lines = payload.get("lines")
        plain = payload.get("plain")
        if lines is not None and not isinstance(lines, list):
            return False
        if plain is not None and not isinstance(plain, str):
            return False
        if metadata.get("instrumental") is True:
            return not lines and not str(plain or "").strip()
        if not lines and not str(plain or "").strip():
            return False
        return all(cls._lyricsfile_line_is_valid(line) for line in lines or [])

    @classmethod
    def _lyricsfile_line_is_valid(cls, line: Any) -> bool:
        """
        校验单行与逐字时间戳，拒绝布尔值伪装整数和倒序区间。
        """
        if not isinstance(line, dict) or not isinstance(line.get("text"), str):
            return False
        start = line.get("start_ms")
        end = line.get("end_ms")
        if isinstance(start, bool) or not isinstance(start, int) or start < 0:
            return False
        if end is not None and (
                isinstance(end, bool) or not isinstance(end, int) or end < start
        ):
            return False
        words = line.get("words")
        if words is None:
            return True
        if not isinstance(words, list):
            return False
        for word in words:
            if not isinstance(word, dict) or not isinstance(word.get("text"), str):
                return False
            word_start = word.get("start_ms")
            word_end = word.get("end_ms")
            if isinstance(word_start, bool) or not isinstance(word_start, int) or word_start < 0:
                return False
            if word_end is not None and (
                    isinstance(word_end, bool)
                    or not isinstance(word_end, int)
                    or word_end < word_start
            ):
                return False
        return True

    @classmethod
    def _lyricsfile_value_is_safe(cls, value: Any, depth: int = 0, count: Optional[list[int]] = None) -> bool:
        """
        限制 YAML 解析后的类型、深度和节点数，避免外部文档消耗过量资源。
        """
        if depth > cls._lyricsfile_max_depth:
            return False
        counter = count if count is not None else [0]
        counter[0] += 1
        if counter[0] > cls._lyricsfile_max_nodes:
            return False
        if value is None or isinstance(value, (str, int, bool)):
            return True
        if isinstance(value, list):
            return all(cls._lyricsfile_value_is_safe(item, depth + 1, counter) for item in value)
        if isinstance(value, dict):
            return all(
                isinstance(key, str)
                and cls._lyricsfile_value_is_safe(item, depth + 1, counter)
                for key, item in value.items()
            )
        return False

    @classmethod
    def _lyricsfile_has_words(cls, content: str) -> bool:
        """
        判断 Lyricsfile 是否包含逐字时间轴。
        """
        payload = cls._parse_lyricsfile(content)
        return bool(
            payload
            and any(
                isinstance(line, dict) and isinstance(line.get("words"), list)
                for line in payload.get("lines") or []
            )
        )

    @staticmethod
    def _optional_text(value: Any) -> str | None:
        """
        把可选标量规范化为非空文本。
        """
        text = str(value or "").strip()
        return text or None

    @classmethod
    def _lyricsfile_to_plain(cls, payload: dict[str, Any]) -> str | None:
        """
        从 Lyricsfile plain 或时间行生成播放器可读纯文本。
        """
        plain = cls._optional_text(payload.get("plain"))
        if plain:
            return plain
        texts = []
        for line in payload.get("lines") or []:
            if not isinstance(line, dict):
                continue
            text = cls._optional_text(line.get("text"))
            if not text and isinstance(line.get("words"), list):
                text = "".join(
                    str(word.get("text") or "")
                    for word in line["words"]
                    if isinstance(word, dict)
                ).strip() or None
            if text:
                texts.append(text)
        return "\n".join(texts) or None

    @classmethod
    def _lyricsfile_to_lrc(cls, payload: dict[str, Any]) -> str | None:
        """
        把 Lyricsfile 行级毫秒时间轴转换为通用 LRC。
        """
        output = []
        for line in payload.get("lines") or []:
            if not isinstance(line, dict):
                continue
            start_ms = _music_optional_int(line.get("start_ms"))
            if start_ms is None:
                continue
            start_ms = max(start_ms, 0)
            text = cls._optional_text(line.get("text"))
            if not text and isinstance(line.get("words"), list):
                text = "".join(
                    str(word.get("text") or "")
                    for word in line["words"]
                    if isinstance(word, dict)
                ).strip() or None
            if not text:
                continue
            minutes, remainder = divmod(start_ms, 60000)
            seconds = remainder / 1000
            output.append(f"[{minutes:02d}:{seconds:05.2f}]{text}")
        return "\n".join(output) or None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> Self:
        """
        从模块或插件返回字典恢复标准歌词对象。
        """
        values = _music_init_values(cls, data)
        values["provider"] = str(values.get("provider") or "")
        values["provider_id"] = (
            str(values["provider_id"])
            if values.get("provider_id") is not None
            else None
        )
        values["instrumental"] = bool(values.get("instrumental"))
        return cls(**values)


@dataclass
class MusicInfo:
    """
    标准化音乐元数据信息。

    :ivar type: 固定为音乐媒体类型，不接受初始化参数
    :param media_source: 媒体主身份的数据来源，与 media_id 配对使用
    :param media_id: 当前来源的原生媒体标识，与 media_source 配对使用
    :param music_type: 音乐实体类型，默认表示单曲录音
    :param title: 音乐实体的标题；未知时为 None
    :param artists: 参与署名的艺术家名称，保留来源顺序
    :param artist_ids: 艺术家标准 ID，顺序与 artists 一致，供详情页关联跳转
    :param album: 曲目所属专辑名称；未知时为 None
    :param album_artist: 专辑层级的艺术家署名；未知时为 None
    :param album_id: 所属专辑的 MusicBrainz 发行组标识
    :param musicbrainz_release_id: 发行、发行组和发行曲目身份独立于 Recording 主身份，不能相互代用
    :param musicbrainz_release_group_id: MusicBrainz 发行组标识，用于关联同一专辑的不同版本
    :param musicbrainz_release_track_id: MusicBrainz 发行版本中的曲目标识，不等同于录音标识
    :param album_type: 专辑主类型：Album、EP、Single 等
    :param secondary_types: 专辑副类型：Live、Compilation、Soundtrack 等
    :param year: 展示或匹配使用的年份；未知时为 None
    :param original_year: 作品最早发行年份；未知时为 None
    :param release_year: 当前发行版本的年份；未知时为 None
    :param release_date: 来源提供的发行日期文本；未知时为 None
    :param release_status: 发行状态，例如正式发行或宣传版本；未知时为 None
    :param disc_number: 曲目所属碟号；未知时为 None
    :param track_number: 曲目在所属碟中的序号；未知时为 None
    :param total_tracks: 所属发行版本的曲目总数；未知时为 None
    :param total_discs: 发行版本的总碟数；未知时为 None
    :param duration: 曲目时长，单位为秒
    :param isrc: 国际标准录音代码；未知时为 None
    :param cover_url: 封面图片链接；未提供封面时为 None
    :param lyrics: 曲目的歌词文本；尚未获取时为 None
    :param version: 曲目的版本说明，例如现场版或混音版
    :param audio_format: 音频编码或容器格式；未知时为 None
    :param audio_lossless: 是否为无损音频；未知时为 None
    :param bit_depth: 音频采样位深，单位为位
    :param sample_rate: 音频采样率，单位为赫兹
    :param bitrate: 音频码率，单位为比特每秒
    :param category: 兼容字段，过渡期始终与 library_category 同步
    :param library_category: 自动分类或人工覆盖后的媒体库相对目录分类
    :param metadata_category: 数据源提供的专辑类型、流派等描述性分类
    :param classification: 分类规则的判定结果；未执行分类时为 None
    :param classification_facts: 插件来源提交的受控扩展分类事实，统一收口时由宿主注册表校验
    :param genres: 来源提供的音乐流派名称列表
    :param tags: 来源提供的音乐标签列表
    :param artist_country: 艺术家所属国家；未知时为 None
    :param names: 可用于搜索匹配的名称列表
    :param title_aliases: 来源提供的同一实体别名及展示转换前的原文，不混入所属专辑或无关联艺术家
    :param album_aliases: 所属专辑的别名，不混入曲目或艺术家别名
    :param artist_aliases: 参与艺术家的别名，不混入无关联艺术家名称
    :param detail_link: 来源站点的详情页链接；未知时为 None
    :param listen_count: 来源记录的收听次数；未知时为 None
    :param raw_data: 来源返回的原始元数据，供来源特有字段读取
    :param field_sources: 字段名到元数据来源的映射，用于保留合并后的来源依据
    :param composers: 古典角色独立于发行署名；乐器/声部映射保留演奏者的具体职责
    :param conductors: 指挥姓名列表，与专辑发行署名独立
    :param orchestras: 参与演奏的乐团名称列表
    :param performers: 乐器或声部到演奏者姓名列表的映射
    :param music_layout: 音乐整理采用的目录布局；尚未确定时为 None
    :param organization_error: 音乐整理准入失败的原因；未失败时为 None
    :cvar recognize_cache_hit: 本地识别缓存命中标记，不参与序列化，默认 False
    """

    type: MediaType = field(default=MediaType.MUSIC, init=False)
    media_source: MediaSource | None = None
    media_id: str | None = None
    music_type: str = MUSIC_ENTITY_RECORDING
    title: str | None = None
    artists: list[str] = field(default_factory=list)
    artist_ids: list[str] = field(default_factory=list)
    album: str | None = None
    album_artist: str | None = None
    album_id: str | None = None
    musicbrainz_release_id: str | None = field(default=None, kw_only=True)
    musicbrainz_release_group_id: str | None = field(default=None, kw_only=True)
    musicbrainz_release_track_id: str | None = field(default=None, kw_only=True)
    album_type: str | None = None
    secondary_types: list[str] = field(default_factory=list)
    year: int | None = None
    original_year: int | None = field(default=None, kw_only=True)
    release_year: int | None = field(default=None, kw_only=True)
    release_date: str | None = None
    release_status: str | None = None
    disc_number: int | None = None
    track_number: int | None = None
    total_tracks: int | None = None
    total_discs: int | None = field(default=None, kw_only=True)
    duration: int | None = None
    isrc: str | None = None
    cover_url: str | None = None
    lyrics: str | None = None
    version: str | None = None
    audio_format: str | None = None
    audio_lossless: bool | None = None
    bit_depth: int | None = None
    sample_rate: int | None = None
    bitrate: int | None = None
    category: str = ""
    library_category: str = ""
    metadata_category: str = ""
    classification: ClassificationResult | None = None
    classification_facts: dict[str, ClassificationFactValue] = field(default_factory=dict)
    genres: list[str] = field(default_factory=list)
    tags: list[str] = field(default_factory=list)
    artist_country: str | None = None
    names: list[str] = field(default_factory=list)
    title_aliases: list[str] = field(default_factory=list)
    album_aliases: list[str] = field(default_factory=list)
    artist_aliases: list[str] = field(default_factory=list)
    detail_link: str | None = None
    listen_count: int | None = None
    raw_data: dict[str, Any] = field(default_factory=dict)
    field_sources: dict[str, str] = field(default_factory=dict, kw_only=True)
    composers: list[str] = field(default_factory=list, kw_only=True)
    conductors: list[str] = field(default_factory=list, kw_only=True)
    orchestras: list[str] = field(default_factory=list, kw_only=True)
    performers: dict[str, list[str]] = field(default_factory=dict, kw_only=True)
    music_layout: str | None = field(default=None, kw_only=True)
    organization_error: str | None = field(default=None, kw_only=True)
    # 内部标记：是否命中本地识别缓存，不参与序列化；与 MediaInfo 保持一致，
    # 显式声明以保留 getattr(..., False) 的默认值语义（__getattr__ 兜底会覆盖它）
    recognize_cache_hit = False

    def __setstate__(self, state: dict[str, Any]) -> None:
        """
        恢复旧版本音乐缓存时补齐新增别名字段，避免列表字段被缺失属性兜底为 None。
        """
        self.__dict__.update(state)
        self.__dict__.update(music_credit_values(state))
        for name in ("title_aliases", "album_aliases", "artist_aliases"):
            self.__dict__.setdefault(name, [])
        self.__dict__.setdefault("field_sources", {})

    def __post_init__(self) -> None:
        """
        规范化媒体身份，并兼容拆分旧音乐分类字段。
        """
        self.field_sources = dict(self.field_sources or {})
        self.__dict__.update(music_credit_values(self))
        self.year = self.year if self.year is not None else self.release_year or self.original_year
        self.media_source, self.media_id = resolve_media_identity(media=self)
        classification = _classification_result(self.classification)
        library_category, metadata_category = _resolve_music_categories(
            category=self.category,
            library_category=self.library_category,
            metadata_category=self.metadata_category,
            album_type=self.album_type,
            secondary_types=self.secondary_types,
            genres=self.genres,
            tags=self.tags,
            classification=classification,
        )
        self.__dict__["classification"] = classification
        self.__dict__["library_category"] = library_category
        self.__dict__["category"] = library_category
        self.__dict__["metadata_category"] = metadata_category
        self.__dict__["_category_semantics_ready"] = True

    def __setattr__(self, name: str, value: Any) -> None:
        """
        兼容旧调用方直接写 category，并在初始化后同步库分类字段。
        """
        if (
            name in {"category", "library_category"}
            and self.__dict__.get("_category_semantics_ready")
        ):
            normalized = str(value or "").strip()
            self.__dict__["category"] = normalized
            self.__dict__["library_category"] = normalized
            return
        self.__dict__[name] = value

    def __getattr__(self, name: str) -> None:
        """
        影视专用字段兜底返回 None：音乐模型不存在这些字段，避免下游逐点安全访问。

        兼容影视媒体共享的通用读写路径（通知、整理、历史等），任何缺失字段按
        空值处理；真实字段仍由类定义接管，不受影响。dunder 特殊方法除外——
        copy/pickle 等机制依赖 hasattr 探测 __setstate__ 等钩子，兜底返回 None
        会导致探测误判而调用失败。注意这会掩盖属性名拼写错误，属于权衡取舍。
        """
        if name.startswith("__") and name.endswith("__"):
            raise AttributeError(name)
        return None

    @property
    def artist(self) -> str:
        """
        返回兼容现有展示组件的艺术家文本。
        """
        return " / ".join(self.artists)

    @property
    def audio_quality(self) -> str | None:
        """
        返回 hires、lossless 或 lossy 音质等级。
        """
        return audio_quality_tier(
            self.audio_format, self.audio_lossless, self.bit_depth, self.sample_rate, self.bitrate
        )

    @property
    def audio_quality_score(self) -> int:
        """
        返回音乐订阅洗版使用的音质优先级。
        """
        return audio_quality_score(
            self.audio_format, self.audio_lossless, self.bit_depth, self.sample_rate, self.bitrate
        )

    @property
    def audio_specs(self) -> str | None:
        """
        返回识别结果和通知使用的格式化音频参数。
        """
        return format_audio_quality(
            self.audio_format, self.audio_lossless, self.bit_depth, self.sample_rate, self.bitrate
        )

    @property
    def episode_group(self) -> None:
        """
        音乐没有剧集组，兼容现有下载历史字段。
        """
        return None

    @property
    def season(self) -> None:
        """
        音乐没有季信息，兼容失败冷却和目录逻辑。
        """
        return None

    @property
    def vote_average(self) -> float:
        """
        音乐当前没有评分字段，兼容订阅统计与持久化。
        """
        return 0.0

    @property
    def overview(self) -> str:
        """
        返回兼容订阅描述字段的音乐摘要。
        """
        parts = [self.artist, self.album, self.version]
        return " · ".join(part for part in parts if part)

    @property
    def title_year(self) -> str:
        """
        返回包含年份的展示标题。
        """
        if not self.title:
            return ""
        return f"{self.title} ({self.year})" if self.year else self.title

    @property
    def poster_path(self) -> str | None:
        """
        返回兼容现有媒体卡片的封面地址。
        """
        return self.cover_url

    @property
    def backdrop_path(self) -> str | None:
        """
        返回兼容现有下载卡片的背景地址。
        """
        return self.cover_url

    def get_message_image(self, default: bool | None = None) -> str | None:
        """
        返回通知消息使用的音乐封面。
        """
        return self.cover_url

    def get_poster_image(self, default: bool | None = None) -> str | None:
        """
        返回海报位使用的音乐封面。
        """
        return self.cover_url

    def get_backdrop_image(self, default: bool = False) -> str | None:
        """
        返回背景图位使用的音乐封面。
        """
        return self.cover_url

    def set_library_category(self, category: str | None) -> None:
        """
        设置媒体库分类，并同步过渡期兼容 category 字段。
        """
        self.library_category = category or ""

    def clear(self) -> None:
        """
        清理不参与队列展示和持久化的上游原始响应。
        """
        self.raw_data = {}

    def to_dict(self) -> dict[str, Any]:
        """
        转换为统一媒体身份的 Context 外层字典。
        """
        payload = asdict(self)
        payload["classification"] = _classification_payload(self.classification)
        payload["category"] = self.library_category
        payload.update(
            {
                "type": self.type.value,
                "media_source": (
                    self.media_source.value
                    if isinstance(self.media_source, MediaSource)
                    else self.media_source
                ),
                "artist": self.artist,
                "title_year": self.title_year,
                "poster_path": self.poster_path,
                "backdrop_path": self.backdrop_path,
                "overview": self.overview,
                "vote_average": self.vote_average,
                "audio_quality": self.audio_quality,
                "audio_quality_score": self.audio_quality_score,
                "audio_specs": self.audio_specs,
            }
        )
        return payload

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> Self:
        """
        从字典恢复标准化音乐元数据。
        """
        _validate_music_type(data.get("type"))
        values = _music_init_values(cls, data)
        values["classification"] = _classification_result(values.get("classification"))
        values["media_source"] = normalize_media_source(values.get("media_source"))
        values["artists"] = _music_string_list(values.get("artists") or data.get("artist"))
        values["artist_ids"] = _music_aligned_list(values.get("artist_ids"))
        for key in ("secondary_types", "genres", "tags", "title_aliases", "album_aliases", "artist_aliases"):
            values[key] = _music_string_list(values.get(key))
        values["names"] = _music_string_list(values.get("names"))
        values["music_type"] = str(values.get("music_type") or MUSIC_ENTITY_RECORDING)
        values["audio_format"] = normalize_audio_format(values.get("audio_format"))
        values["audio_lossless"] = infer_audio_lossless(
            values.get("audio_format"), values.get("audio_lossless")
        )
        values["raw_data"] = dict(values.get("raw_data") or {})
        values["field_sources"] = dict(values.get("field_sources") or {})
        if "library_category" in data or "metadata_category" in data:
            library_value = (
                data.get("library_category")
                if "library_category" in data
                else data.get("category")
            )
            values["library_category"] = str(library_value or "")
            values["metadata_category"] = str(data.get("metadata_category") or "")
            values["category"] = values["library_category"]
        else:
            library_category, metadata_category = _resolve_music_categories(
                category=data.get("category"),
                library_category=None,
                metadata_category=None,
                album_type=values.get("album_type"),
                secondary_types=values.get("secondary_types"),
                genres=values.get("genres"),
                tags=values.get("tags"),
                classification=values.get("classification"),
            )
            values["category"] = library_category
            values["library_category"] = library_category
            values["metadata_category"] = metadata_category
        for key in (
            "year",
            "original_year",
            "release_year",
            "disc_number",
            "track_number",
            "total_tracks",
            "total_discs",
            "duration",
            "listen_count",
            "bit_depth",
            "sample_rate",
            "bitrate",
        ):
            values[key] = _music_optional_int(values.get(key))
        return cls(**values)

    @classmethod
    def from_meta(cls, meta: MetaMusic) -> Self:
        """
        将文件名和音频标签解析结果转换为无远端依赖的标准音乐信息。
        """
        return cls(
            **music_credit_values(meta),
            media_source=normalize_media_source(meta.media_source),
            media_id=meta.media_id,
            music_type=meta.music_type or MUSIC_ENTITY_RECORDING,
            album_type=meta.album_type,
            secondary_types=list(meta.secondary_types),
            music_layout=meta.music_layout,
            organization_error=meta.organization_error,
            musicbrainz_release_id=meta.musicbrainz_release_id,
            musicbrainz_release_group_id=meta.musicbrainz_release_group_id,
            musicbrainz_release_track_id=meta.musicbrainz_release_track_id,
            original_year=meta.original_year,
            release_year=meta.release_year,
            field_sources=dict(meta.field_sources),
            title=meta.title,
            artists=list(meta.artists),
            album=meta.album,
            album_artist=meta.album_artist,
            year=_music_optional_int(meta.year),
            disc_number=meta.disc_number,
            track_number=meta.track_number,
            total_tracks=meta.total_tracks,
            total_discs=meta.total_discs,
            duration=meta.duration,
            isrc=meta.isrc,
            version=meta.version,
            audio_format=meta.audio_format,
            audio_lossless=meta.audio_lossless,
            bit_depth=meta.bit_depth,
            sample_rate=meta.sample_rate,
            bitrate=meta.bitrate,
            names=[name for name in (meta.title, meta.album) if name],
        )


@dataclass
class MusicRelease:
    """
    音乐专辑下的单个发行版本（MusicBrainz Release）。

    :param media_id: MusicBrainz 具体发行版本标识
    :param title: 当前发行版本的标题
    :param date: 发行日期文本；未知时为 None
    :param country: 来源提供的国家代码；未知时为 None
    :param status: 发行状态，例如正式发行或宣传版本
    :param packaging: 实体发行的包装形式
    :param formats: 各介质的格式名称列表
    :param track_count: 当前发行版本的总曲目数；未知时为 None
    :param cover_url: 封面图片链接；未提供封面时为 None
    """

    media_id: str | None = None
    title: str | None = None
    date: str | None = None
    country: str | None = None
    status: str | None = None
    packaging: str | None = None
    formats: list[str] = field(default_factory=list)
    track_count: int | None = None
    cover_url: str | None = None

    @property
    def year(self) -> int | None:
        """
        返回发行版本年份。
        """
        return _music_year_of(self.date)

    def to_dict(self) -> dict[str, Any]:
        """
        转换为可传输的字典。
        """
        payload = asdict(self)
        payload["year"] = self.year
        return payload

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> Self:
        """
        从字典恢复发行版本信息。
        """
        values = _music_init_values(cls, data)
        values["formats"] = _music_string_list(values.get("formats"))
        values["track_count"] = _music_optional_int(values.get("track_count"))
        return cls(**values)


@dataclass
class MusicAlbumInfo:
    """
    标准化音乐专辑信息（MusicBrainz Release Group）。

    :ivar type: 固定为音乐媒体类型，不接受初始化参数
    :ivar music_type: 固定为专辑实体，不接受初始化参数
    :param media_source: 媒体主身份的数据来源，与 media_id 配对使用
    :param media_id: 当前来源的原生媒体标识，与 media_source 配对使用
    :param musicbrainz_release_id: MusicBrainz 具体发行版本标识，不等同于录音或发行组标识
    :param musicbrainz_release_group_id: MusicBrainz 发行组标识，用于关联同一专辑的不同版本
    :param original_year: 作品最早发行年份；未知时为 None
    :param total_discs: 发行版本的总碟数；未知时为 None
    :param title: 音乐实体的标题；未知时为 None
    :param artists: 参与署名的艺术家名称，保留来源顺序
    :param artist_ids: 艺术家标准标识列表，顺序与 artists 对应
    :param title_aliases: 当前音乐实体的别名及展示转换前的原名
    :param artist_aliases: 参与艺术家的别名，不混入无关联艺术家名称
    :param album_type: 专辑主类型：Album、EP、Single、Broadcast、Other
    :param secondary_types: 专辑副类型：Live、Compilation、Soundtrack、Remix 等
    :param library_category: 自动分类或人工覆盖后的媒体库相对目录分类
    :param metadata_category: 数据源提供的专辑类型和副类型描述
    :param classification: 分类规则的判定结果；未执行分类时为 None
    :param classification_facts: 来源提交的扩展分类事实，由宿主字段注册表校验
    :param release_date: 来源提供的发行日期文本；未知时为 None
    :param release_status: 发行状态，例如正式发行或宣传版本；未知时为 None
    :param artist_country: 艺术家所属国家；未知时为 None
    :param cover_url: 封面图片链接；未提供封面时为 None
    :param genres: 来源提供的音乐流派名称列表
    :param tags: 来源提供的音乐标签列表
    :param rating: 来源给出的评分，默认 0 表示尚无评分
    :param rating_votes: 参与评分的人数；未知时为 None
    :param detail_link: 来源站点的详情页链接；未知时为 None
    :param tracks: 专辑的曲目详情，保留来源顺序
    :param releases: 同一发行组下的发行版本列表
    :param raw_data: 来源返回的原始元数据，供来源特有字段读取
    :param field_sources: 字段名到元数据来源的映射，用于保留合并后的来源依据
    :param composers: 古典角色独立于发行署名；乐器/声部映射保留演奏者的具体职责
    :param conductors: 指挥姓名列表，与专辑发行署名独立
    :param orchestras: 参与演奏的乐团名称列表
    :param performers: 乐器或声部到演奏者姓名列表的映射
    """

    type: MediaType = field(default=MediaType.MUSIC, init=False)
    music_type: str = field(default=MUSIC_ENTITY_ALBUM, init=False)
    media_source: MediaSource | None = None
    media_id: str | None = None
    musicbrainz_release_id: str | None = field(default=None, kw_only=True)
    musicbrainz_release_group_id: str | None = field(default=None, kw_only=True)
    original_year: int | None = field(default=None, kw_only=True)
    total_discs: int | None = field(default=None, kw_only=True)
    title: str | None = None
    artists: list[str] = field(default_factory=list)
    artist_ids: list[str] = field(default_factory=list)
    title_aliases: list[str] = field(default_factory=list)
    artist_aliases: list[str] = field(default_factory=list)
    album_type: str | None = None
    secondary_types: list[str] = field(default_factory=list)
    library_category: str = ""
    metadata_category: str = ""
    classification: ClassificationResult | None = None
    classification_facts: dict[str, ClassificationFactValue] = field(default_factory=dict)
    release_date: str | None = None
    release_status: str | None = None
    artist_country: str | None = None
    cover_url: str | None = None
    genres: list[str] = field(default_factory=list)
    tags: list[str] = field(default_factory=list)
    rating: float = 0.0
    rating_votes: int | None = None
    detail_link: str | None = None
    tracks: list[MusicInfo] = field(default_factory=list)
    releases: list[MusicRelease] = field(default_factory=list)
    raw_data: dict[str, Any] = field(default_factory=dict)
    field_sources: dict[str, str] = field(default_factory=dict, kw_only=True)
    composers: list[str] = field(default_factory=list, kw_only=True)
    conductors: list[str] = field(default_factory=list, kw_only=True)
    orchestras: list[str] = field(default_factory=list, kw_only=True)
    performers: dict[str, list[str]] = field(default_factory=dict, kw_only=True)

    def __setstate__(self, state: dict[str, Any]) -> None:
        """
        恢复旧版专辑缓存时补齐别名列表，保留既有媒体身份及曲目数据。
        """
        self.__dict__.update(state)
        self.__dict__.update(music_credit_values(state))
        for name in ("title_aliases", "artist_aliases"):
            self.__dict__.setdefault(name, [])
        self.__dict__.setdefault("field_sources", {})

    def __post_init__(self) -> None:
        """
        规范化媒体身份，并补全专辑描述分类和分类结果。
        """
        self.field_sources = dict(self.field_sources or {})
        self.__dict__.update(music_credit_values(self))
        self.media_source, self.media_id = resolve_media_identity(media=self)
        self.library_category = str(self.library_category or "").strip()
        self.metadata_category = str(self.metadata_category or "").strip() or (
            " / ".join(
                part for part in [self.album_type, *self.secondary_types] if part
            )
        )
        self.classification = _classification_result(self.classification)

    @property
    def artist(self) -> str:
        """
        返回兼容现有展示组件的艺术家文本。
        """
        return " / ".join(self.artists)

    @property
    def year(self) -> int | None:
        """
        返回专辑首次发行年份。
        """
        return _music_year_of(self.release_date)

    @property
    def category(self) -> str:
        """
        返回兼容字段，其语义始终等同媒体库分类。
        """
        return self.library_category

    @category.setter
    def category(self, value: str | None) -> None:
        """
        兼容旧调用方直接写 category，并同步媒体库分类。
        """
        self.set_library_category(value)

    def set_library_category(self, category: str | None) -> None:
        """
        设置媒体库分类，来源描述分类保持不变。
        """
        self.library_category = str(category or "").strip()

    @property
    def track_count(self) -> int:
        """
        返回专辑内已解析的音乐数量。
        """
        return len(self.tracks)

    @property
    def duration(self) -> int | None:
        """
        返回专辑内所有音乐时长之和。
        """
        durations = [track.duration for track in self.tracks if track.duration]
        return sum(durations) if durations else None

    @property
    def title_year(self) -> str:
        """
        返回包含年份的专辑展示标题。
        """
        if not self.title:
            return ""
        return f"{self.title} ({self.year})" if self.year else self.title

    @property
    def poster_path(self) -> str | None:
        """
        返回兼容现有媒体卡片的封面地址。
        """
        return self.cover_url

    @property
    def backdrop_path(self) -> str | None:
        """
        返回兼容现有详情页背景的封面地址。
        """
        return self.cover_url

    @property
    def overview(self) -> str:
        """
        返回专辑摘要，供卡片和通知复用。
        """
        parts = [
            self.artist,
            self.metadata_category,
            self.release_date,
            " / ".join(self.genres[:3]),
        ]
        return " · ".join(part for part in parts if part)

    def to_dict(self) -> dict[str, Any]:
        """
        转换为兼容前端 MediaInfo 结构的字典。
        """
        payload = {
            key: value
            for key, value in asdict(self).items()
            if key not in {"tracks", "releases", "type"}
        }
        payload["classification"] = _classification_payload(self.classification)
        payload.update(
            {
                "type": self.type.value,
                "artist": self.artist,
                "album": self.title,
                "year": self.year,
                "category": self.category,
                "duration": self.duration,
                "total_tracks": self.track_count,
                "title_year": self.title_year,
                "poster_path": self.poster_path,
                "backdrop_path": self.backdrop_path,
                "media_source": (
                    self.media_source.value
                    if isinstance(self.media_source, MediaSource)
                    else self.media_source
                ),
                "overview": self.overview,
                "vote_average": self.rating,
                "tracks": [track.to_dict() for track in self.tracks],
                "releases": [release.to_dict() for release in self.releases],
            }
        )
        return payload

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> Self:
        """
        从字典恢复标准化专辑信息。
        """
        _validate_music_type(data.get("type"))
        values = _music_init_values(cls, data)
        values["classification"] = _classification_result(values.get("classification"))
        values["media_source"] = normalize_media_source(values.get("media_source"))
        for key in ("artists", "secondary_types", "genres", "tags", "title_aliases", "artist_aliases"):
            values[key] = _music_string_list(values.get(key))
        values["artist_ids"] = _music_aligned_list(values.get("artist_ids"))
        values["rating"] = _music_optional_float(values.get("rating"))
        values["rating_votes"] = _music_optional_int(values.get("rating_votes"))
        values["tracks"] = [
            item if isinstance(item, MusicInfo) else MusicInfo.from_dict(item)
            for item in data.get("tracks") or []
        ]
        values["releases"] = [
            item if isinstance(item, MusicRelease) else MusicRelease.from_dict(item)
            for item in data.get("releases") or []
        ]
        values["raw_data"] = dict(values.get("raw_data") or {})
        values["field_sources"] = dict(values.get("field_sources") or {})
        values["original_year"] = _music_optional_int(values.get("original_year"))
        values["total_discs"] = _music_optional_int(values.get("total_discs"))
        if "library_category" in data or "metadata_category" in data:
            library_value = (
                data.get("library_category")
                if "library_category" in data
                else data.get("category")
            )
            values["library_category"] = str(library_value or "")
            values["metadata_category"] = str(data.get("metadata_category") or "")
        else:
            library_category, metadata_category = _resolve_music_categories(
                category=data.get("category"),
                library_category=None,
                metadata_category=None,
                album_type=values.get("album_type"),
                secondary_types=values.get("secondary_types"),
                genres=values.get("genres"),
                tags=values.get("tags"),
                classification=values.get("classification"),
            )
            values["library_category"] = library_category
            values["metadata_category"] = metadata_category
        return cls(**values)

    def to_music_info(self) -> MusicInfo:
        """
        转换为专辑卡片使用的音乐信息，供列表接口统一返回。
        """
        return MusicInfo(
            **music_credit_values(self),
            media_source=self.media_source,
            media_id=self.media_id,
            musicbrainz_release_id=self.musicbrainz_release_id,
            musicbrainz_release_group_id=self.musicbrainz_release_group_id,
            original_year=self.original_year,
            release_year=self.year if self.musicbrainz_release_id else None,
            total_discs=self.total_discs,
            field_sources=dict(self.field_sources),
            music_type=MUSIC_ENTITY_ALBUM,
            title=self.title,
            artists=list(self.artists),
            artist_ids=list(self.artist_ids),
            album=self.title,
            album_artist=self.artist or None,
            album_id=self.media_id,
            title_aliases=list(self.title_aliases),
            album_aliases=list(self.title_aliases),
            artist_aliases=list(self.artist_aliases),
            album_type=self.album_type,
            secondary_types=list(self.secondary_types),
            year=self.year,
            release_date=self.release_date,
            release_status=self.release_status,
            total_tracks=self.track_count or None,
            duration=self.duration,
            cover_url=self.cover_url,
            library_category=self.library_category,
            metadata_category=self.metadata_category,
            classification=self.classification,
            genres=list(self.genres),
            tags=list(self.tags),
            artist_country=self.artist_country,
            names=[name for name in (self.title,) if name],
            detail_link=self.detail_link,
        )


@dataclass
class MusicArtistInfo:
    """
    标准化音乐艺术家信息（MusicBrainz Artist）。

    :ivar type: 固定为音乐媒体类型，不接受初始化参数
    :ivar music_type: 固定为艺术家实体，不接受初始化参数
    :param media_source: 媒体主身份的数据来源，与 media_id 配对使用
    :param media_id: 当前来源的原生媒体标识，与 media_source 配对使用
    :param name: 艺术家的展示名称；未知时为 None
    :param sort_name: 来源提供的艺术家排序名称；未知时为 None
    :param disambiguation: MusicBrainz 消歧义说明，同名艺术家依靠该字段区分
    :param artist_type: 艺术家类型：Person、Group、Orchestra、Choir、Character、Other
    :param gender: 艺术家的性别描述；未知时为 None
    :param country: 来源提供的国家代码；未知时为 None
    :param area: 艺术家所属地区名称；未知时为 None
    :param begin_date: 艺术家的出生或成立日期文本
    :param end_date: 艺术家的逝世或解散日期文本
    :param ended: 来源是否明确标记艺术家活动已结束
    :param genres: 来源提供的音乐流派名称列表
    :param tags: 来源提供的音乐标签列表
    :param aliases: 艺术家的别名列表
    :param library_category: 自动分类或人工覆盖后的媒体库相对目录分类
    :param metadata_category: 数据源提供的艺术家类型等描述性分类
    :param classification: 分类规则的判定结果；未执行分类时为 None
    :param classification_facts: 来源提交的扩展分类事实，由宿主字段注册表校验
    :param relation: 关联艺术家场景下的关系文本，例如乐队成员、子团体
    :param image_url: 艺术家的展示图片链接
    :param detail_link: 来源站点的详情页链接；未知时为 None
    :param external_links: 外部站点链接，键为关系类型，值为地址
    :param album_count: 来源记录的专辑数量；未知时为 None
    :param raw_data: 来源返回的原始元数据，供来源特有字段读取
    """

    type: MediaType = field(default=MediaType.MUSIC, init=False)
    music_type: str = field(default=MUSIC_ENTITY_ARTIST, init=False)
    media_source: MediaSource | None = None
    media_id: str | None = None
    name: str | None = None
    sort_name: str | None = None
    disambiguation: str | None = None
    artist_type: str | None = None
    gender: str | None = None
    country: str | None = None
    area: str | None = None
    begin_date: str | None = None
    end_date: str | None = None
    ended: bool = False
    genres: list[str] = field(default_factory=list)
    tags: list[str] = field(default_factory=list)
    aliases: list[str] = field(default_factory=list)
    library_category: str = ""
    metadata_category: str = ""
    classification: ClassificationResult | None = None
    classification_facts: dict[str, ClassificationFactValue] = field(default_factory=dict)
    relation: str | None = None
    image_url: str | None = None
    detail_link: str | None = None
    external_links: dict[str, str] = field(default_factory=dict)
    album_count: int | None = None
    raw_data: dict[str, Any] = field(default_factory=dict)

    def __post_init__(self) -> None:
        """
        规范化媒体身份，并拆分艺术家描述分类与媒体库分类。
        """
        self.media_source, self.media_id = resolve_media_identity(media=self)
        self.library_category = str(self.library_category or "").strip()
        self.metadata_category = str(
            self.metadata_category or self.artist_type or ""
        ).strip()
        self.classification = _classification_result(self.classification)

    @property
    def category(self) -> str:
        """
        返回兼容字段，其语义始终等同媒体库分类。
        """
        return self.library_category

    @category.setter
    def category(self, value: str | None) -> None:
        """
        兼容旧调用方直接写 category，并同步媒体库分类。
        """
        self.set_library_category(value)

    def set_library_category(self, category: str | None) -> None:
        """
        设置媒体库分类，来源艺术家类型保持不变。
        """
        self.library_category = str(category or "").strip()

    @property
    def title(self) -> str | None:
        """
        返回兼容通用媒体展示组件的标题。
        """
        return self.name

    @property
    def life_span(self) -> str:
        """
        返回艺术家活跃时间区间文本。
        """
        if not self.begin_date and not self.end_date:
            return ""
        end = self.end_date or ("" if self.ended else "…")
        return f"{self.begin_date or '?'} - {end}" if end else (self.begin_date or "")

    @property
    def overview(self) -> str:
        """
        返回艺术家摘要，供卡片和详情页复用。
        """
        parts = [
            self.artist_type,
            self.disambiguation,
            self.area or self.country,
            self.life_span,
            " / ".join(self.genres[:3]),
        ]
        return " · ".join(part for part in parts if part)

    @property
    def poster_path(self) -> str | None:
        """
        返回兼容现有卡片的艺术家图片地址。
        """
        return self.image_url

    def to_dict(self) -> dict[str, Any]:
        """
        转换为可传输的字典。
        """
        payload = {key: value for key, value in asdict(self).items() if key != "type"}
        payload["classification"] = _classification_payload(self.classification)
        payload.update(
            {
                "type": self.type.value,
                "category": self.category,
                "title": self.title,
                "life_span": self.life_span,
                "overview": self.overview,
                "poster_path": self.poster_path,
                "media_source": (
                    self.media_source.value
                    if isinstance(self.media_source, MediaSource)
                    else self.media_source
                ),
            }
        )
        return payload

    def to_music_info(self) -> MusicInfo:
        """
        转换为统一搜索列表使用的音乐信息，但不赋予下载或订阅语义。
        """
        return MusicInfo(
            media_source=self.media_source,
            media_id=self.media_id,
            music_type=MUSIC_ENTITY_ARTIST,
            title=self.name,
            cover_url=self.image_url,
            version=self.disambiguation,
            library_category=self.library_category,
            metadata_category=self.metadata_category,
            classification=self.classification,
            classification_facts=dict(self.classification_facts),
            genres=list(self.genres),
            tags=list(self.tags),
            artist_country=self.country,
            names=[name for name in [self.name, *self.aliases] if name],
            title_aliases=list(self.aliases),
            detail_link=self.detail_link,
            raw_data=dict(self.raw_data),
        )

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> Self:
        """
        从字典恢复标准化艺术家信息。
        """
        _validate_music_type(data.get("type"))
        values = _music_init_values(cls, data)
        values["classification"] = _classification_result(values.get("classification"))
        values["media_source"] = normalize_media_source(values.get("media_source"))
        for key in ("genres", "tags", "aliases"):
            values[key] = _music_string_list(values.get(key))
        values["ended"] = bool(values.get("ended"))
        values["album_count"] = _music_optional_int(values.get("album_count"))
        values["external_links"] = {
            str(key): str(value)
            for key, value in (values.get("external_links") or {}).items()
            if value
        }
        values["raw_data"] = dict(values.get("raw_data") or {})
        values["library_category"] = str(
            data.get("library_category") or data.get("category") or ""
        )
        values["metadata_category"] = str(
            data.get("metadata_category") or ""
        )
        return cls(**values)
