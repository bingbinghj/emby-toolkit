"""MP 默认音乐模板；只同步已选择歌曲的名称，保留做种目录。"""
import os
import re
from pathlib import Path
from . import library
from .audio import AudioMetadataHelper
from .lyrics import EXTENSIONS

# 对应 MP MUSIC_RENAME_FORMAT 的默认模板，曲序补两位、非法字符转全角。
def render(metadata, extension):
    def name(value):
        text = str(value or "").strip()
        text = text.translate(str.maketrans({char: chr(ord(char)+0xFEE0) for char in '\\/:*?"<>|'}))
        if any(ord(char) < 32 for char in text) or text in {".", ".."}:
            raise ValueError("音乐名称包含无效路径字符")
        return text
    artist = name(metadata.get("album_artist") or " / ".join(metadata.get("artists") or [])) or "Unknown Artist"
    album, year = name(metadata.get("album")) or "Unknown Album", metadata.get("year")
    if year:
        album = re.sub(r"(?:[\s\u3000]*[\(\[（【]\s*(?:19|20)\d{2}\s*[\)\]）】])+$", "", album) or album
        album += f" ({year})"
    title = name(metadata.get("title"))
    if not title:
        raise ValueError("同步名称需要曲名，请补充曲名后重新预览")
    parts = [artist, album]
    if (metadata.get("total_discs") or 0) > 1:
        parts.append(f"Disc {metadata.get('disc_number') or 1}")
    track = metadata.get("track_number")
    parts.append((f"{track:02d} - " if track else "") + title + extension)
    if any(len(part.encode("utf-8")) > 255 for part in parts):
        raise ValueError("同步后的文件或目录名称超过 255 字节，请缩短标签")
    return str(Path(*parts))


def projected(before, updates):
    """使用将实际写入的标签，遵守缺失补充/跳过策略和容器的曲序、年份语义。"""
    after = dict(before)
    for tag, field in {"title":"title", "artist":"artists", "album":"album", "albumartist":"album_artist"}.items():
        if tag in updates:
            after[field] = updates[tag] if field == "artists" else updates[tag][0]
    for tag, field, total in [("tracknumber","track_number","total_tracks"), ("discnumber","disc_number","total_discs")]:
        if tag in updates:
            after[field], after[total] = AudioMetadataHelper._number_pair(updates[tag][0])
    for tag, field in [("date", "release_year"), ("originaldate", "original_year")]:
        if tag in updates:
            after[field] = AudioMetadataHelper._year(updates[tag][0])
    after["year"] = after.get("release_year") or after.get("original_year") or after.get("year")
    return after


def safe_path(root, relative):
    relative = Path(relative)
    if relative.is_absolute() or ".." in relative.parts or not relative.parts:
        raise ValueError("同步名称的路径必须位于音乐库内")
    path = root / relative
    if path.parent.resolve() != path.parent:
        raise ValueError("同步名称的目录不能包含符号链接")
    parent = path.parent
    while not parent.exists():
        parent = parent.parent
    if not parent.is_dir() or parent.stat().st_dev != root.stat().st_dev:
        raise ValueError("同步名称需要同一文件系统内的本地音乐库目录")
    return path


def companions(path):
    return [path.with_suffix(ext) for ext in EXTENSIONS if os.path.lexists(path.with_suffix(ext))]


def preflight(root, source, destination):
    """预览和提交前均拒绝覆盖已有歌曲或孤立歌词。"""
    safe_path(root, str(destination.relative_to(root)))
    for candidate in [destination, *(destination.with_suffix(ext) for ext in EXTENSIONS)]:
        if os.path.lexists(candidate):
            raise ValueError(f"目标已存在，未覆盖：{candidate.relative_to(root)}")
    for candidate in companions(source):
        if candidate.is_symlink() or not candidate.is_file():
            raise ValueError(f"旁挂歌词不是普通文件，未移动：{candidate.name}")
    # 标签写入前可软链接到另一磁盘的做种文件；隔离副本落在库内原目录。
    if source.parent.stat().st_dev != root.stat().st_dev:
        raise ValueError("同步名称不能跨越音乐库挂载点")


def plan(root, row):
    updates = row["tag_updates"]
    if not updates.keys() & {"title", "artist", "album", "albumartist", "date", "originaldate", "tracknumber", "discnumber"}:
        return
    after = projected(row["before"], updates)
    if not str(after.get("title") or "").strip():
        row["rename_note"] = "当前没有曲名，保留现有文件和目录名称；标签仍按预览保存"
        return
    destination = render(after, Path(row["path"]).suffix)
    try:
        if destination == render(row["before"], Path(row["path"]).suffix):
            return
    except ValueError:
        pass  # 修正原先不完整或无效的名称后，允许同步新名称。
    if destination == row["path"]:
        return
    source = root / row["path"]
    # CUE 的 FILE 引用必须和音频一起更新；保留目录结构，避免损坏整轨索引。
    # 即便关闭 CUE 识别，也不能把现存索引留成悬空引用。
    from .audio import _cue_candidates, _cue_file_references, _read_cue_text
    for cue in _cue_candidates(source):
        if source.name in _cue_file_references(_read_cue_text(cue)):
            row["rename_note"] = "关联 CUE 的音频保留现有文件和目录名称，标签仍按预览保存"
            return
    preflight(root, source, root/destination)
    row["target"] = destination
    row["rename_lyrics"] = {str(item.relative_to(root)): library.signature(item) for item in companions(source)}


def verify(root, row):
    source, destination = root/row["path"], safe_path(root, row["target"])
    if row["target"] != render(projected(row["before"], row["tag_updates"]), source.suffix):
        raise ValueError("同步名称与预览标签不一致，请重新预览")
    preflight(root, source, destination)
    actual = {str(item.relative_to(root)): library.signature(item) for item in companions(source)}
    if actual != row.get("rename_lyrics", {}):
        raise ValueError("歌词在预览后已变化，请重新预览")


def manifest(root, source, destination, config):
    """歌词提交后冻结实际文件身份；先建立全部新名称，再移除旧名称。"""
    preflight(root, source, destination)
    moves = [{"source":str(item.relative_to(root)), "target":str((destination if item == source else destination.with_name(destination.stem+item.name[len(source.stem):])).relative_to(root)),
              "signature":library.signature(item)} for item in [source, *companions(source)]]
    if source.parent != destination.parent:
        # 专辑封面为同目录歌曲共用；最后一首移走时才移走旧封面。
        shared = any(item != source and item.suffix.casefold() in library.AUDIO_EXTENSIONS for item in source.parent.iterdir())
        covers = set()
        for pattern in str(config.get("music_cover_file_patterns") or "cover.jpg,folder.jpg,front.jpg,cover.png,folder.png").split(","):
            pattern = pattern.strip()
            if pattern and "/" not in pattern and "\\" not in pattern and pattern not in {".", ".."}:
                covers.update(item for item in source.parent.glob(pattern) if item.is_file() and not item.is_symlink() and item.suffix.casefold() in {".jpg", ".jpeg", ".png", ".webp"})
        for item in sorted(covers):
            target = destination.parent/item.name
            if os.path.lexists(target) and (target.is_symlink() or not item.samefile(target)):
                continue
            moves.append({"source":str(item.relative_to(root)), "target":str(target.relative_to(root)), "signature":library.signature(item), "keep_source":shared, "existed":os.path.lexists(target)})
    return moves


def link_files(root, moves, matches, flush):
    for move in moves:
        source, destination = safe_path(root, move["source"]), safe_path(root, move["target"])
        if not matches(source, move["signature"]):
            raise ValueError("音乐或歌词在同步名称时已变化")
        if os.path.lexists(destination):
            if not matches(destination, move["signature"]):
                raise ValueError(f"目标已存在，未覆盖：{move['target']}")
        else:
            missing, parent = [], destination.parent
            while not parent.exists():
                missing.append(parent)
                parent = parent.parent
            for parent in reversed(missing):
                parent.mkdir(exist_ok=True)
                flush(parent.parent)
            os.link(source, destination, follow_symlinks=False)
        flush(destination.parent)


def finish(root, moves, matches, flush):
    for move in moves:
        source, destination = safe_path(root, move["source"]), safe_path(root, move["target"])
        if not matches(destination, move["signature"]):
            raise ValueError("同步后的音乐或歌词已变化，保留原文件")
        if not move.get("keep_source") and os.path.lexists(source):
            if not matches(source, move["signature"]):
                raise ValueError("原音乐或歌词已变化，未删除")
            source.unlink()
            flush(source.parent)
    # 只移除空目录，不删除封面、其他歌曲或用户文件。
    parent = (root/moves[0]["source"]).parent
    while parent != root:
        try:
            parent.rmdir()
        except OSError:
            break
        flush(parent.parent)
        parent = parent.parent


def rollback(root, moves, matches, flush):
    for move in moves:
        source, destination = safe_path(root, move["source"]), safe_path(root, move["target"])
        if not move.get("existed") and matches(source, move["signature"]) and matches(destination, move["signature"]):
            destination.unlink()
            flush(destination.parent)
