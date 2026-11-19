"""手工歌词沿用 MP 的旁挂文件方式；保存前核对预览时的内容。"""
import hashlib
import re
from copy import deepcopy
from .audio import AudioMetadataHelper
from .models import MusicLyrics

EXTENSIONS = (".lrc", ".txt", ".lyricsfile.yaml")
TIMESTAMP = re.compile(r"\[\d+:\d{2}(?:[.:]\d+)?\]")


def read_lyrics(path):
    revisions, contents = {}, {}
    for extension in EXTENSIONS:
        candidate = path.with_suffix(extension)
        if candidate.is_file():
            if candidate.stat().st_size > 1024*1024:
                raise ValueError("旁挂歌词超过 1 MB，不能在标签编辑器中加载")
            data = candidate.read_bytes()
            revisions[extension] = hashlib.sha256(data).hexdigest()
            try:
                contents[extension] = data.decode("utf-8-sig")
            except UnicodeDecodeError:
                import chardet
                contents[extension] = data.decode(chardet.detect(data).get("encoding") or "utf-8", errors="strict")
        else:
            revisions[extension] = None
    for extension in (".lrc", ".txt"):
        if extension in contents:
            return {"lyrics": contents[extension], "lyrics_source": path.with_suffix(extension).name, "lyrics_revision": revisions}
    if ".lyricsfile.yaml" in contents:
        value = MusicLyrics(provider="local", lyricsfile=contents[".lyricsfile.yaml"])
        return {"lyrics": value.content or "", "lyrics_source": path.with_suffix(".lyricsfile.yaml").name, "lyrics_revision": revisions}
    value = AudioMetadataHelper.read_lyrics(path)
    return {"lyrics": value.content if value else "", "lyrics_source": "内嵌歌词" if value else "", "lyrics_revision": revisions}


def prepare_edit(path, values):
    if "lyrics" not in values:
        return None
    text = values["lyrics"]
    if not isinstance(text, str) or len(text.encode("utf-8")) > 1024*1024:
        raise ValueError("歌词必须为不超过 1 MB 的文本")
    current = read_lyrics(path)
    if values.get("lyrics_revision") is not None and values["lyrics_revision"] != current["lyrics_revision"]:
        raise ValueError("歌词在打开编辑器后已变化，请重新打开")
    if text.rstrip() == current["lyrics"].rstrip():
        return None
    if not text.strip():
        raise ValueError("歌词内容不能为空；清空输入不会删除已有歌词")
    edit = {"text": text, "extension": ".lrc" if TIMESTAMP.search(text) else ".txt", "revision": current["lyrics_revision"]}
    if current["lyrics_revision"].get(".lyricsfile.yaml"):
        edit["lyricsfile"] = update_lyricsfile(path.with_suffix(".lyricsfile.yaml").read_text(encoding="utf-8-sig"), text)
    return edit


def update_lyricsfile(existing, text):
    """同步已有 Lyricsfile；仅平移且正文相同的行保留逐字时间。"""
    from .amll_lyrics import _serialize_payload
    payload = MusicLyrics._parse_lyricsfile(existing)
    if not payload:
        raise ValueError("已有 Lyricsfile 无法解析，未覆盖歌词")
    old = payload.pop("lines", [])
    payload.pop("plain", None)
    offset = re.search(r"\[offset:([+-]?\d+)\]", text, re.I)
    offset = int(offset[1]) if offset else 0
    rows = []
    for line in text.splitlines():
        content = re.sub(r"<\d+:\d{2}(?:[.:]\d+)?>", "", TIMESTAMP.sub("", line)).strip()
        for stamp in TIMESTAMP.findall(line):
            minutes, seconds = stamp[1:-1].split(":", 1)
            milliseconds = round((int(minutes)*60 + float(seconds.replace(":", ".")))*1000)-offset
            if milliseconds < 0:
                raise ValueError("歌词 offset 产生负时间，请先校正时间标签")
            rows.append({"text": content, "start_ms": milliseconds})
    rows.sort(key=lambda row: row["start_ms"])
    if rows:
        for index, row in enumerate(rows):
            if len(old)==len(rows) and old[index]["text"].strip()==row["text"] and not re.search(r"<\d+:\d{2}", text):
                copied = deepcopy(old[index])
                delta = row["start_ms"]-copied["start_ms"]
                for part in [copied, *copied.get("words", [])]:
                    for key in ("start_ms", "end_ms"):
                        if part.get(key) is not None:
                            part[key] += delta
                if MusicLyrics._lyricsfile_line_is_valid(copied):
                    rows[index] = copied
        payload["lines"] = rows
    else:
        payload["plain"] = text
    payload["metadata"]["instrumental"] = False
    return _serialize_payload(payload)


def verify_edit(path, edit):
    if edit and read_lyrics(path)["lyrics_revision"] != edit["revision"]:
        raise ValueError("歌词在预览后已变化，请重新预览")


def apply_edit(path, edit, identifier, writer):
    target = path.with_suffix(edit["extension"])
    text = edit["text"].rstrip()+"\n"
    # 已原子提交但还未保存回执时，恢复任务只补清理旧格式。
    if not target.is_file() or target.read_bytes() != text.encode("utf-8"):
        verify_edit(path, edit)
        writer(target, text, identifier)
    if edit.get("lyricsfile"):
        destination = path.with_suffix(".lyricsfile.yaml")
        intended = (edit["lyricsfile"].rstrip()+"\n").encode("utf-8")
        present = destination.read_bytes() if destination.is_file() else b""
        if present != intended:
            if hashlib.sha256(present).hexdigest() != edit["revision"].get(".lyricsfile.yaml"):
                raise ValueError("Lyricsfile 在预览后已变化，未覆盖该文件")
            writer(destination, edit["lyricsfile"], identifier)
    for extension in (".lrc", ".txt"):
        if extension != edit["extension"]:
            candidate = path.with_suffix(extension)
            if candidate.is_file():
                if hashlib.sha256(candidate.read_bytes()).hexdigest() != edit["revision"].get(extension):
                    raise ValueError("另一格式歌词被外部修改，已保留该文件")
                candidate.unlink()
