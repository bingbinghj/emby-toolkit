"""按 music-tag-web 使用 translators；只翻译正文，时间标签由本地代码保留。"""
import os
import re
import time
from .cache import cached

PROVIDERS = {"youdao": "有道翻译", "bing": "必应翻译", "google": "谷歌翻译"}
STAMP = re.compile(r"\[(?:\d+:\d{1,2}(?:[.:]\d+)?|[A-Za-z][\w-]*:[^\]\r\n]*)\]")
WORD = re.compile(r"<\d+:\d{1,2}(?:[.:]\d+)?>")


def _translate(text, provider):
    # 避免导入库时额外探测服务器地区；仅初始化并请求用户选中的渠道。
    os.environ.setdefault("translators_default_region", "EN")
    import translators
    return translators.translate_text(text, translator=provider, from_language="auto", to_language="zh",
                                     timeout=12, if_print_warning=False)


@cached(maxsize=256, ttl=86400, region="lyrics-translation")
def _batch(lines, provider):
    translated = _translate("\n".join(lines), provider)
    if not isinstance(translated, str):
        raise ValueError("翻译服务没有返回有效译文")
    result = translated.strip().splitlines()
    if len(result) != len(lines) or any(not line.strip() for line in result):
        raise ValueError("译文行数与原文不一致，未应用，请切换翻译服务后重试")
    return [line.strip() for line in result]


def translate(body):
    provider, text = body.get("provider", "youdao"), body.get("lyrics")
    if provider not in PROVIDERS:
        raise ValueError("请选择有效的翻译服务")
    if not isinstance(text, str) or not text.strip() or len(text) > 20000:
        raise ValueError("请输入不超过 20000 字的歌词")
    lines = text.split('\n')
    selected = body.get('line_indices', list(range(len(lines))))
    if not isinstance(selected, list) or not selected or any(type(index) is not int or not 0 <= index < len(lines) for index in selected):
        raise ValueError('请选择有效的歌词行')
    selected = set(selected)
    rows, unique = [], []
    for index, line in enumerate(lines):
        prefix = ""
        remaining = line
        while match := STAMP.match(remaining):
            prefix += match.group()
            remaining = remaining[match.end():]
        content = WORD.sub("", remaining).strip()
        rows.append((line, prefix, content))
        if index in selected and content and content not in unique:
            unique.append(content)
    if not unique:
        raise ValueError("没有可翻译的歌词正文")
    mapping, batch, length = {}, [], 0
    batches = []
    for line in unique:
        if len(line) > 900:
            raise ValueError("单行歌词超过 900 字，请先分行")
        if length + len(line) + 1 > 900:
            batches.append(batch)
            batch, length = [], 0
        batch.append(line)
        length += len(line) + 1
    if batch:
        batches.append(batch)
    started = time.monotonic()
    for batch in batches:
        if time.monotonic() - started > 75:
            raise ValueError("翻译等待超时，请稍后重试；已完成的译文会复用")
        try:
            mapping.update(zip(batch, _batch(batch, provider), strict=True))
        except Exception as error:
            if isinstance(error, ValueError):
                raise
            raise ValueError(f"{PROVIDERS[provider]}暂时不可用，请稍后重试或切换服务") from error
    translation, bilingual = [], []
    for index, (original, prefix, content) in enumerate(rows):
        translated = mapping.get(content) if index in selected else None
        translation.append(prefix + translated if translated is not None else original)
        bilingual.append(original)
        if translated is not None and translated != content:
            bilingual.append(prefix + translated)
    return {"translation": "\n".join(translation), "bilingual": "\n".join(bilingual), "provider": provider,
            "line_translations": [{"index": index, "text": mapping[content]} for index, (_, _, content) in enumerate(rows)
                                  if index in selected and content in mapping]}
