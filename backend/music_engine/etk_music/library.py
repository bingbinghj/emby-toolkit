"""本地音乐库索引、路径边界与 MP 链接隔离写入。"""
import base64
from contextlib import contextmanager
import hashlib
import json
import os
import sqlite3
from pathlib import Path
from .audio import AudioMetadataHelper
from .models import MusicInfo
from .runtime import get_runtime_setting

AUDIO_EXTENSIONS = {".flac", ".mp3", ".m4a", ".mp4", ".aac", ".ape", ".wav", ".wave", ".aif", ".aiff", ".ogg", ".opus", ".wma", ".wv", ".tta", ".mpc", ".dsf", ".dff"}


def album_directory(relative):
    from .meta import MetaMusic
    directory = Path(relative).parent
    return str(directory.parent if MetaMusic.parse_disc_dir(directory.name) is not None else directory)


def preview_groups(paths):
    groups = {}
    for relative in paths:
        groups.setdefault(album_directory(relative), []).append(relative)
    return groups


def roots(config):
    raw_library, raw_seed = config.get("music_library_path", ""), config.get("music_seed_path", "")
    if not raw_library or not raw_seed:
        raise ValueError("请先在设置中配置本地音乐库和做种目录")
    library, seed = Path(raw_library).resolve(strict=True), Path(raw_seed).resolve(strict=True)
    if not library.is_dir() or not seed.is_dir():
        raise ValueError("音乐库或做种目录不可用，请检查本地目录挂载")
    if library == seed or library in seed.parents or seed in library.parents:
        raise ValueError("音乐库与做种目录必须独立，不能相同或互相包含")
    return library, seed


def target(config, relative):
    library, seed = roots(config)
    relative = Path(relative)
    if relative.is_absolute() or ".." in relative.parts or not relative.parts:
        raise ValueError("音乐路径必须位于配置的音乐库内")
    path = library / relative
    # 目录符号链接不允许把写入重定向到做种目录；末级音频软链接由 MP 隔离器处理。
    if path.parent.resolve(strict=True) != path.parent:
        raise ValueError("音乐库内的符号链接目录不能作为写入目标")
    real = path.resolve(strict=True)
    if not real.is_relative_to(library) and not real.is_relative_to(seed):
        raise ValueError("音乐文件指向配置目录之外")
    if path.suffix.casefold() not in AUDIO_EXTENSIONS:
        raise ValueError("所选文件不是支持的本地音频")
    if not real.is_file():
        raise ValueError("所选音乐不是普通文件")
    return path


def signature(path):
    info, link = path.stat(), path.lstat()
    return {"dev": info.st_dev, "ino": info.st_ino, "size": info.st_size, "mtime": info.st_mtime_ns, "ctime": info.st_ctime_ns, "link_ino": link.st_ino}


@contextmanager
def database():
    db = sqlite3.connect(get_runtime_setting("CACHE_PATH") / "library.sqlite3", timeout=30)
    db.execute("PRAGMA journal_mode=WAL")
    db.execute("CREATE TABLE IF NOT EXISTS tracks(root TEXT,path TEXT,signature TEXT,data TEXT,seen TEXT,PRIMARY KEY(root,path))")
    db.execute("CREATE INDEX IF NOT EXISTS tracks_seen ON tracks(root,seen)")
    try:
        with db:
            yield db
    finally:
        db.close()


def metadata(path):
    if AudioMetadataHelper._open_audio(path) is None:
        raise ValueError("音频内容无法读取，请检查文件实际格式")
    info = MusicInfo.from_meta(AudioMetadataHelper.read(path)).to_dict()
    info.update(size=path.stat().st_size, link_count=path.stat().st_nlink, symbolic_link=path.is_symlink())
    return info


def walk(config):
    root, _ = roots(config)
    paths = []
    def failure(error):
        raise error
    for directory, folders, files in os.walk(root, followlinks=False, onerror=failure):
        folders[:] = sorted(name for name in folders if not name.startswith(".") and not (Path(directory)/name).is_symlink())
        for name in sorted(files):
            path = Path(directory)/name
            if not name.startswith(".") and path.suffix.lower() in AUDIO_EXTENSIONS:
                paths.append(str(path.relative_to(root)))
    return paths


def scan_one(config, relative, scan_id, cue_cache=None):
    root, _ = roots(config)
    path = target(config, relative)
    current = signature(path)
    # 音频未变时，旁挂 CUE 的内容及开关变化同样使索引失效。
    current["cue_enabled"] = bool(config.get("music_cue_enable", True))
    current["cues"] = []
    cue_cache = cue_cache if cue_cache is not None else {}
    if current["cue_enabled"]:
        directory = str(path.parent)
        if directory not in cue_cache:
            cues=[]
            for cue in sorted(path.parent.iterdir()):
                if cue.suffix.casefold()==".cue" and cue.is_file() and not cue.is_symlink():
                    with cue.open("rb") as stream:
                        digest=hashlib.sha256(stream.read(1024*1024+1)).hexdigest()
                    cues.append([cue.name,digest])
                    if len(cues)>=32:break
            cue_cache[directory]=cues
        current["cues"] = cue_cache[directory]
    with database() as db:
        previous = db.execute("SELECT signature,data FROM tracks WHERE root=? AND path=?", (str(root), relative)).fetchone()
        info = json.loads(previous[1]) if previous and json.loads(previous[0]) == current else metadata(path)
        db.execute("INSERT OR REPLACE INTO tracks VALUES(?,?,?,?,?)", (str(root), relative, json.dumps(current), json.dumps(info, ensure_ascii=False), scan_id))
    return {"path": relative, **info, "signature": current}


def finish_scan(config, scan_id):
    root, _ = roots(config)
    with database() as db:
        db.execute("DELETE FROM tracks WHERE root=? AND seen!=?", (str(root), scan_id))


def listing(config, query="", page=1, page_size=50):
    root, _ = roots(config)
    size = min(max(page_size, 1), 1000)
    # 参数化 LIKE，搜索只作用于音乐索引中的曲名、艺人、专辑和相对路径。
    pattern = "%"+(query or "").replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")+"%"
    clause = "root=? AND (path LIKE ? ESCAPE '\\' OR json_extract(data,'$.title') LIKE ? ESCAPE '\\' OR json_extract(data,'$.artists') LIKE ? ESCAPE '\\' OR json_extract(data,'$.album') LIKE ? ESCAPE '\\')"
    values = (str(root), pattern, pattern, pattern, pattern)
    with database() as db:
        total = db.execute("SELECT COUNT(*) FROM tracks WHERE "+clause, values).fetchone()[0]
        rows = db.execute("SELECT path,signature,data FROM tracks WHERE "+clause+" ORDER BY path LIMIT ? OFFSET ?", (*values, size, (max(page,1)-1)*size)).fetchall()
    return {"items": [{"path": path, **json.loads(data), "signature": json.loads(identity)} for path, identity, data in rows], "total": total, "page": page, "page_size": size}


def inspect(config, relative):
    path = target(config, relative)
    info = metadata(path)
    from .lyrics import read_lyrics
    return {"path": relative, **info, "signature": signature(path), **read_lyrics(path), "write_mode": "isolate" if path.is_symlink() or path.stat().st_nlink > 1 else "independent"}


def directory(config, relative=""):
    root, _ = roots(config)
    part = Path(relative or ".")
    if part.is_absolute() or ".." in part.parts:
        raise ValueError("音乐目录必须位于配置的音乐库内")
    path = root / part
    if path.resolve(strict=True) != path or not path.is_dir():
        raise ValueError("音乐目录不可用或包含符号链接")
    return root, path


def browse(config, relative="", page=1, page_size=50):
    """浏览 MP 已整理的真实目录，不依据标签重建或移动歌手/专辑。"""
    root, path = directory(config, relative)
    items = []
    children = list(path.iterdir())
    audio_names = {}
    for child in children:
        if child.suffix.lower() in AUDIO_EXTENSIONS and child.is_file():
            audio_names.setdefault(child.stem, []).append(str(child.relative_to(root)))
    for child in children:
        if child.name.startswith(".") or (child.is_symlink() and child.is_dir()):
            continue
        is_dir = child.is_dir()
        if not is_dir and not child.is_file():
            continue
        stat = child.stat()
        stem = child.name.removesuffix('.lyricsfile.yaml') if child.name.endswith('.lyricsfile.yaml') else child.stem if child.suffix.lower() in {'.lrc', '.txt'} else None
        companions = audio_names.get(stem, [])
        items.append({"path": str(child.relative_to(root)), "name": child.name,
                      "type": "dir" if is_dir else "file", "is_audio": not is_dir and child.suffix.lower() in AUDIO_EXTENSIONS,
                      "audio_path": companions[0] if not is_dir and len(companions) == 1 else None,
                      "size": None if is_dir else stat.st_size, "modify_time": stat.st_mtime})
    items.sort(key=lambda item: (item["type"] != "dir", item["name"].casefold()))
    size = min(max(page_size, 1), 1000)
    start = (max(page, 1)-1)*size
    return {"items": items[start:start+size], "total": len(items), "path": str(path.relative_to(root)), "page": page, "page_size": size}


def selection(config, paths):
    root, _ = roots(config)
    selected = set()
    for relative in paths:
        if (root / relative).is_dir():
            _, folder = directory(config, relative)
            def failure(error):
                raise error
            for parent, folders, files in os.walk(folder, followlinks=False, onerror=failure):
                folders[:] = sorted(name for name in folders if not name.startswith(".") and not (Path(parent)/name).is_symlink())
                for name in sorted(files):
                    if not name.startswith(".") and Path(name).suffix.lower() in AUDIO_EXTENSIONS:
                        value = str((Path(parent)/name).relative_to(root))
                        target(config, value)
                        selected.add(value)
                        if len(selected)>1000:
                            raise ValueError("一次最多选择 1000 首音乐，请缩小选择范围")
        else:
            target(config, relative)
            selected.add(relative)
    if not selected or len(selected)>1000:
        raise ValueError("请选择 1 至 1000 个音乐文件")
    return {"paths": sorted(selected)}


def image(config, relative):
    path = target(config, relative)
    audio = AudioMetadataHelper._open_audio(path)
    from mutagen.flac import FLAC
    from mutagen.mp4 import MP4
    pictures = []
    if isinstance(audio, FLAC):
        pictures = [(p.mime, p.data) for p in audio.pictures]
    elif isinstance(audio, MP4):
        pictures = [("image/png" if bytes(p).startswith(b"\x89PNG") else "image/jpeg", bytes(p)) for p in (audio.tags or {}).get("covr", [])]
    elif hasattr(getattr(audio,"tags",None), "getall"):
        pictures = [(p.mime, p.data) for p in audio.tags.getall("APIC")]
    if pictures:
        mime, data = pictures[0]
        if mime in {"image/png", "image/jpeg", "image/webp"} and len(data) <= 15*1024*1024:
            return {"mime": mime, "data": base64.b64encode(data).decode()}
    for name in str(config.get("music_cover_file_patterns") or "cover.jpg,folder.jpg,front.jpg,cover.png,folder.png").split(","):
        name = name.strip()
        if not name or "/" in name or "\\" in name or name in {".", ".."}:
            continue
        for candidate in sorted(path.parent.glob(name)):
            if candidate.is_file() and not candidate.is_symlink() and candidate.stat().st_size <= 15*1024*1024:
                mime = "image/png" if candidate.suffix.lower()==".png" else "image/jpeg"
                return {"mime": mime, "data": base64.b64encode(candidate.read_bytes()).decode()}
    return None
