"""本地音乐的刮削和标签编辑；仅更新用户选择的文件。"""
import base64
import hashlib
import fcntl
import json
import os
import re
import shutil
import time
from collections import defaultdict
from pathlib import Path
from dataclasses import asdict
from . import catalog, library, naming
from .audio import AudioMetadataHelper, capture_audio_metadata
from .models import MusicInfo, MusicLyrics
from .meta import MetaMusic
from .matching import music_tags_are_usable
from .album import recognize_group, _preserve_music_image_tags, _merge_music_album_metadata
from .evidence import _merge_music_audio_quality
from .observation import music_recognition_is_blocked
from .runtime import get_runtime_setting
from .http import RequestUtils
from .text import convert
from . import lyrics as local_lyrics

EDIT_FIELDS = {"title", "artists", "album", "album_artist", "track_number", "total_tracks", "disc_number", "total_discs", "year", "release_year", "original_year", "composers", "conductors", "orchestras", "performers", "isrc", "media_source", "media_id", "music_type", "musicbrainz_release_id", "musicbrainz_release_group_id", "musicbrainz_release_track_id"}
EDIT_FIELDS.update({"comment", "description"})
EMPTY_TAG_FIELDS = {"title": "title", "artists": "artist", "album": "album", "album_artist": "albumartist", "isrc": "isrc",
                    "composers": "composer", "conductors": "conductor", "orchestras": "orchestra", "performers": "performer"}

def uploaded_cover(value):
    if not isinstance(value, str) or len(value) > 1_400_000:
        raise ValueError("上传封面格式无效或超过 1 MiB")
    match = re.fullmatch(r"data:(image/(?:jpeg|png));base64,([A-Za-z0-9+/=]+)", value)
    if not match:
        raise ValueError("上传封面只支持 JPEG 或 PNG")
    data = base64.b64decode(match[2], validate=True)
    valid = data.startswith(b"\x89PNG\r\n\x1a\n") if match[1] == "image/png" else data.startswith(b"\xff\xd8\xff")
    if not valid or len(data) > 1024 * 1024:
        raise ValueError("上传封面内容无效或超过 1 MiB")
    return data, match[1]


def clear_comments_preview(config):
    """仅计划清空本地标签，不识别、刮削或改写其他字段。"""
    rows = []
    for relative in library.walk(config):
        try:
            path = library.target(config, relative)
            identity = library.signature(path)
            audio = AudioMetadataHelper._open_audio(path)
            if audio is None:
                raise ValueError("音频内容无法读取")
            info = MusicInfo(comment="", description="")
            updates = AudioMetadataHelper._tag_updates(audio, info, True)
            if library.signature(path) != identity:
                raise ValueError("音乐文件在读取时已变化")
            rows.append({"path": relative, "target": relative, "signature": identity,
                         "metadata": {"comment": "", "description": ""}, "success": True,
                         "skip": not updates})
        except Exception as error:
            rows.append({"path": relative, "success": False, "message": str(error)})
    return rows


def write_json(path, data):
    temporary = path.with_name(path.name+".new")
    with temporary.open("w", encoding="utf-8") as stream:
        json.dump(data, stream, ensure_ascii=False)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    flush_directory(path.parent)


@capture_audio_metadata()
def preview(config, body):
    root, _ = library.roots(config)
    sources = config.get("music_sources") or ["musicbrainz"]
    operation = body.get("operation", "rescrape")
    if operation not in {"rescrape", "edit"}:
        raise ValueError("未知音乐操作")
    selected = body.get("selection")
    track_match = body.get("music_track_match") or "strict"
    if track_match not in {"strict", "ignore_duration", "sequential"}:
        raise ValueError("曲目对齐方式无效")
    music_type = body.get("music_type")
    if music_type not in {None, "recording", "album", "artist_collection"}:
        raise ValueError("音乐实体无效")
    if music_type == "artist_collection" and selected:
        raise ValueError("艺术家合集按各专辑和单曲分别识别，不能统一指定一份资料")
    rows = []
    paths = list(dict.fromkeys(body.get("paths") or []))
    if (body.get("metadata") or {}).get("cover_upload") and len(paths) != 1:
        raise ValueError("本地封面上传只用于当前歌曲")
    overrides = body.get("selections") or {}
    if selected and selected.get("music_type", "recording")=="recording" and len(paths)!=1:
        raise ValueError("单曲资料只能用于一个所选音频，请选择专辑")
    group_keys = {name: group for group, names in library.preview_groups(paths, music_type, body.get("collection_root")).items() for name in names}
    evidence, errors, groups, recognized, cue_digests = {}, {}, defaultdict(list), {}, {}
    invalid_cue = set()
    for relative in paths:
        try:
            path = library.target(config, relative)
            identity = library.signature(path)
            meta = AudioMetadataHelper.read(path)
            if AudioMetadataHelper._open_audio(path) is None:
                raise ValueError("音频内容无法读取，请检查文件实际格式")
            if meta.organization_error:
                if meta.music_layout == "cue_invalid":
                    invalid_cue.add(relative)
                raise ValueError(meta.organization_error)
            if meta.cue_filename:
                cue = path.parent / Path(meta.cue_filename).name
                cue_digests[relative] = hashlib.sha256(cue.read_bytes()).hexdigest()
            evidence[relative] = (path, identity, meta)
            groups[group_keys[relative]].append(relative)
        except Exception as error:
            errors[relative] = str(error)
    if operation!="edit":
        for directory, group in groups.items():
            choice = overrides.get(directory) or selected
            if not choice and (music_type == "recording" or music_type == "artist_collection" and library.collection_single(group[0], body.get("collection_root"))):
                continue
            if choice and choice.get("music_type", "album")!="album":
                continue
            try:
                files = [evidence[name][0] for name in group]
                matches, diagnostic = recognize_group(files, [evidence[name][2] for name in group], sources, choice, root/library.album_directory(group[0]), track_match)
                for name in group:
                    info = matches.get(str(evidence[name][0].resolve()))
                    if info:
                        recognized[name] = info
                    elif diagnostic.get("status") in {"ambiguous", "conflict"}:
                        errors[name] = diagnostic.get("message", "专辑识别需要确认")
            except Exception as error:
                for name in group:
                    errors[name] = str(error)
    for relative in paths:
        try:
            if relative in errors:
                raise ValueError(errors[relative])
            path, identity, meta = evidence[relative]
            if operation == "edit":
                # 手选专辑沿用读取时的对齐策略；纯手改标签无需在线识别。
                if selected:
                    catalog.recognize(path, sources, selected, track_match)
                # Manual edits start with stored tags; filename/folder guesses
                # must not rewrite audio when only sidecar lyrics are changed.
                info = MusicInfo.from_meta(AudioMetadataHelper.read_tags(path) or MetaMusic())
                values = body.get("metadata") or {}
                for key in ("comment", "description"):
                    value = values.get(key)
                    if value is not None and (not isinstance(value, str) or len(value) > 65536 or "\0" in value):
                        raise ValueError("备注和简介必须为不含空字符且不超过 65536 字符的文本")
                if values.get("signature") is not None and values["signature"] != identity and values["signature"] != {key: str(value) for key, value in identity.items()}:
                    raise ValueError("音乐文件在打开编辑器后已变化，请重新打开")
                identity_keys = ["musicbrainz_release_id", "musicbrainz_release_group_id", "musicbrainz_release_track_id"]
                if values.get("media_source", info.media_source) == "musicbrainz":
                    identity_keys.append("media_id")
                for key in identity_keys:
                    if values.get(key) and not AudioMetadataHelper._normalize_musicbrainz_id(values[key]):
                        raise ValueError("MusicBrainz 身份必须为有效 UUID")
                current = info.to_dict()
                current.pop("artist", None)  # 派生显示字段不能把已清空的 artists 补回来。
                info = MusicInfo.from_dict({**current, **{key: value for key, value in values.items() if key in EDIT_FIELDS}})
                manual_lyrics = local_lyrics.prepare_edit(path, values)
                cover_url = values.get("cover_url") or ""
                cover_upload = values.get("cover_upload") or ""
                if cover_upload:
                    uploaded_cover(cover_upload)
                    if cover_url:
                        raise ValueError("请只选择上传封面或封面地址之一")
                if cover_url and not re.match(r"^https?://[^/\s]+", cover_url):
                    raise ValueError("封面地址必须为 HTTP 或 HTTPS 地址")
            elif relative in recognized:
                info = recognized[relative]
            else:
                meta, info = catalog.recognize(path, sources, selected)
                if music_recognition_is_blocked(info):
                    raise ValueError(info.raw_data["recognition"].get("message", "音乐识别需要确认"))
                if not info.media_id and not music_tags_are_usable(meta):
                    raise ValueError("没有匹配到可靠音乐身份，请手动选择歌曲或专辑")
            if operation=="rescrape" and meta.music_layout=="image_cue":
                _preserve_music_image_tags(path, meta)
                album_info = info
                info = MusicInfo.from_meta(_merge_music_album_metadata(meta, album_info))
                info.cover_url = album_info.cover_url
                info.raw_data = album_info.raw_data
            if operation != "edit":
                info = _merge_music_audio_quality(info, meta)
                info = catalog.with_credit_aliases(info)
            info = catalog.simplified(info, config.get("music_metadata_to_simplified", True))
            if relative in cue_digests and hashlib.sha256((path.parent / Path(meta.cue_filename).name).read_bytes()).hexdigest()!=cue_digests[relative]:
                raise ValueError("CUE 在识别期间已变化，请重新预览")
            before = MusicInfo.from_meta(AudioMetadataHelper.read_tags(path) or MetaMusic()).to_dict()
            # 显式清空与资料缺失分开；仅在用户预览过的清空请求中删除原标签。
            clear_tags = [tag for field, tag in EMPTY_TAG_FIELDS.items()
                          if operation == "edit" and field in values and values[field] in (None, "", [], {})]
            rows.append({"path": relative, "target": relative, "signature": identity, "metadata": info.to_dict(), "before": before, "clear_tags": clear_tags, "tag_updates": tag_updates(path, info, config, operation, clear_tags), "success": True, "music_layout": meta.music_layout, "cue_filename": meta.cue_filename, "cue_digest":cue_digests.get(relative)})
            if operation == "edit":
                rows[-1]["lyrics_edit"] = manual_lyrics
                rows[-1]["cover_edit"] = cover_url
                rows[-1]["cover_upload"] = cover_upload
            try:
                if config.get("music_rename_on_save") is True:
                    naming.plan(root, rows[-1])
            except (ValueError, OSError) as error:
                rows[-1].update(success=False, message=str(error))
        except Exception as error:
            rows.append({"path": relative, "success": False, "message": str(error)})
    targets = defaultdict(list)
    for row in rows:
        if row["success"]:
            targets[row["target"]].append(row)
    for group in targets.values():
        if len(group) > 1:
            for row in group:
                row.update(success=False, message="所选歌曲同步后名称相同，请调整标签后重新预览")
    for row in rows:
        relative = row["path"]
        if not row["success"] and relative in invalid_cue:
            row.update(failure_stage="recognition", recovery_action="检查 CUE 文件内容，或关闭「识别 CUE」后重试")
        group = group_keys[relative]
        meta = evidence.get(relative, (None, None, None))[2]
        info = row.get("metadata") or (MusicInfo.from_meta(meta).to_dict() if meta else {})
        diagnostic = info.get("raw_data", {}).get("recognition", {})
        status = "manual" if operation=="edit" else diagnostic.get("status", "metadata")
        row["music_cue_enable"] = body.get("music_cue_enable", config.get("music_cue_enable", True))
        row.update(source=relative, source_storage="local", source_item={"storage":"local", "path":relative, "type":"file"})
        row["music"] = {**info, "group_id":hashlib.sha256((str(root)+"/"+group).encode()).hexdigest(), "group_directory":group,
            "file_role":"audio", "status":status, "online_confirmed":bool(status=="matched" and info.get("media_id")),
            "read_status":("tags" if meta and any(value in {"tag", "cue"} for value in meta.field_sources.values()) else "stream_only") if meta else "unreadable",
            "field_sources":info.get("field_sources") or {}, "candidates":diagnostic.get("candidates", [])}
    return {"operation": operation, "items": rows, "library_path":config.get("music_library_path"), "seed_path":config.get("music_seed_path"), "cue_enable":config.get("music_cue_enable", True), "write_policy":config.get("music_scraping_policies"), "simplified":config.get("music_metadata_to_simplified")}


def tag_updates(path, info, config, operation, clear_tags=()):
    policy = (config.get("music_scraping_policies") or {}).get("tags", "missingOnly")
    if operation != "edit" and policy == "skip":
        return {}
    return AudioMetadataHelper._tag_updates(AudioMetadataHelper._open_audio(path), info, operation=="edit" or policy=="overwrite", clear_tags)


def cover(info, path, config):
    """批次和跨请求复用封面 URL；网络不可用时使用专辑目录图片。"""
    from .cache import Cache
    if info.cover_url:
        cache = Cache(128, 86400, "covers")
        cached = cache.get(info.cover_url)
        if cached:
            return base64.b64decode(cached["data"]), cached["mime"]
        response = RequestUtils().get_res(info.cover_url, stream=True)
        if response is not None:
            try:
                mime = response.headers.get("Content-Type", "").split(";")[0]
                if response.status_code==200 and mime in {"image/jpeg", "image/png"}:
                    chunks, length = [], 0
                    for chunk in response.iter_content(65536):
                        length += len(chunk)
                        if length>15*1024*1024:
                            raise ValueError("封面超过 15 MB")
                        chunks.append(chunk)
                    data = b"".join(chunks)
                    cache.set(info.cover_url, {"data": base64.b64encode(data).decode(), "mime": mime})
                    return data, mime
            finally:
                response.close()
    if config.get("music_local_cover_enable", True):
        root, _ = library.roots(config)
        image = library.image(config, str(path.relative_to(root)))
        if image:
            return base64.b64decode(image["data"]), image["mime"]
    return None, "image/jpeg"


def write_sidecar(target, text, identifier):
    temporary = target.with_name("."+target.name[:40]+".etk-lyrics-"+identifier)
    try:
        with temporary.open("w", encoding="utf-8") as stream:
            stream.write(text.rstrip()+"\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, target)
        flush_directory(target.parent)
    finally:
        temporary.unlink(missing_ok=True)


def sidecar(path, lyrics, policy, simplified, identifier):
    if not lyrics or not lyrics.content or lyrics.instrumental:
        return "未匹配歌词"
    existing = next((path.with_suffix(ext) for ext in (".lyricsfile.yaml", ".lrc", ".txt") if path.with_suffix(ext).exists()), None)
    quality = 4 if existing and existing.name.endswith(".lyricsfile.yaml") else 3 if existing and existing.suffix==".lrc" else 1 if existing else 0
    if existing and (quality>lyrics.quality_rank or policy=="missingOnly" or quality==lyrics.quality_rank and policy!="overwrite"):
        return "保留现有歌词"
    text = lyrics.content
    if simplified:
        # 沿用 MP：转换正文，保留 LRC 标签和逐字时间轴。
        parts = re.split(r"(\[(?:\d+:|[A-Za-z][\w-]*:)[^\]\r\n]*\]|<\d+:\d{2}(?:[.:]\d+)?>)", text)
        text = "".join(part if index % 2 else convert(part, "zh-hans") for index, part in enumerate(parts))
    # 先写播放器兼容歌词，再保留标准逐字 Lyricsfile。失败后同一恢复记录可继续。
    write_sidecar(path.with_suffix(lyrics.extension), text, identifier)
    if lyrics.lyricsfile:
        write_sidecar(path.with_suffix(".lyricsfile.yaml"), lyrics.lyricsfile, identifier)
    if policy=="overwrite":
        for extension in (".lrc", ".txt"):
            if extension!=lyrics.extension:
                path.with_suffix(extension).unlink(missing_ok=True)
    return "歌词已保存"


def existing_lyrics(path):
    return any(path.with_suffix(ext).is_file() for ext in (".lyricsfile.yaml", ".lrc", ".txt"))


def identity_matches(path, expected):
    if not path.exists() and not path.is_symlink():
        return False
    current = library.signature(path)
    return all(current[key] == expected[key] for key in ("dev", "ino", "size", "mtime", "link_ino"))


def flush_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def execute(config, body):
    """暂存副本和持久意图先落盘；原子替换后可凭 inode 回执恢复。"""
    row, operation, identifier = body["item"], body["operation"], body["execution_id"]
    if operation not in {"rescrape", "edit"}:
        raise ValueError("ETK 音乐仅支持刮削和标签编辑")
    if not identifier or any(char not in "0123456789abcdef-" for char in identifier):
        raise ValueError("音乐任务标识无效")
    directory = get_runtime_setting("CACHE_PATH")/"operations"
    directory.mkdir(exist_ok=True)
    receipt = directory/(identifier+".json")
    # 全音乐操作串行锁只占一个固定文件，异常退出自动释放。
    with (directory/"write.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        root, _ = library.roots(config)
        path = root/row["path"]
        saved = json.loads(receipt.read_text()) if receipt.exists() else None
        if saved and saved.get("state")=="completed":
            return saved["result"]
        if saved and saved["state"]=="staging":
            # 尚未提交的暂存不影响原文件，重启后在同一任务内重新准备。
            Path(saved["temporary"]).unlink(missing_ok=True)
            receipt.unlink(missing_ok=True)
            saved = None
        if not saved:
            path = library.target(config, row["path"])
            if row["target"] != row["path"]:
                if config.get("music_rename_on_save") is not True:
                    raise ValueError("音乐同步重命名设置已变化，请重新预览")
                naming.verify(root, row)
            if row.get("cue_digest"):
                cue = path.parent / Path(row["cue_filename"]).name
                if hashlib.sha256(cue.read_bytes()).hexdigest()!=row["cue_digest"]:
                    raise ValueError("CUE 在预览后已变化，请重新预览")
            if library.signature(path)!=row["signature"]:
                raise ValueError("音乐文件在预览后已变化，请重新预览")
            local_lyrics.verify_edit(path, row.get("lyrics_edit"))
            info = MusicInfo.from_dict(row["metadata"])
            policies = config.get("music_scraping_policies") or {"tags":"missingOnly","cover":"missingOnly","lyrics":"upgrade"}
            write_tags = operation=="edit" or policies.get("tags")!="skip"
            overwrite = operation=="edit" or policies.get("tags")=="overwrite"
            clear_tags = tuple(row.get("clear_tags", ())) if operation == "edit" else ()
            audio = AudioMetadataHelper._open_audio(path)
            cover_data, cover_mime = None, "image/jpeg"
            if operation=="edit" and row.get("cover_upload"):
                cover_data, cover_mime = uploaded_cover(row["cover_upload"])
            elif operation=="edit" and row.get("cover_edit"):
                info.cover_url = row["cover_edit"]
                cover_data, cover_mime = cover(info, path, {**config, "music_local_cover_enable": False})
                if not cover_data:
                    raise ValueError("指定封面无法读取，未修改音频")
            if operation!="edit" and policies.get("cover")!="skip" and (policies.get("cover")=="overwrite" or not AudioMetadataHelper._has_cover(audio)):
                cover_data, cover_mime = cover(info, path, config)
            started = time.monotonic()
            lyrics_enabled = operation!="edit" and row.get("music_layout")!="image_cue" and policies.get("lyrics")!="skip"
            keep_lyrics = lyrics_enabled and policies.get("lyrics")=="missingOnly" and existing_lyrics(path)
            lyric = catalog.lyrics(info, path, body.get("lyrics_budget", 120)) if lyrics_enabled and not keep_lyrics else None
            lyrics_elapsed = time.monotonic()-started
            if library.signature(path)!=row["signature"]:
                raise ValueError("音乐文件在读取资料时已变化，请重新预览")
            changed = (write_tags and AudioMetadataHelper._needs_tag_write(audio, info, overwrite, clear_tags)) or bool(cover_data) or (row["target"] != row["path"] and path.is_symlink())
            temporary = path.with_name("."+path.name[:40]+".etk-music-"+identifier)
            saved = {"state":"staging","path":row["path"],"target":row["target"],"original":row["signature"],"temporary":str(temporary),"changed":bool(changed),
                "lyrics_edit":row.get("lyrics_edit"),
                "lyric":asdict(lyric) if lyric else None,"lyrics_elapsed":lyrics_elapsed,"lyrics_policy":policies.get("lyrics"),"lyrics_simplified":config.get("music_lyrics_to_simplified",False),
                "lyrics_result":"保留现有歌词" if keep_lyrics else "未匹配歌词" if lyrics_enabled and not lyric else ""}
            write_json(receipt, saved)
            try:
                if changed:
                    with path.open("rb") as source, temporary.open("xb") as output:
                        shutil.copyfileobj(source, output, 1024*1024)
                        output.flush(); os.fsync(output.fileno())
                    temporary.chmod(path.stat().st_mode | 0o600)
                    if not AudioMetadataHelper.write(temporary, info, cover_data=cover_data, cover_mime=cover_mime, write_tags=write_tags, overwrite=overwrite, cover_overwrite=operation=="edit" or policies.get("cover")=="overwrite", clear_tags=clear_tags):
                        raise ValueError("音乐标签或封面写入失败")
                    if write_tags and operation == "edit":
                        remaining = AudioMetadataHelper._tag_updates(AudioMetadataHelper._open_audio(temporary), info, True, clear_tags)
                        if remaining.keys() & {"comment", "description", *clear_tags}:
                            raise ValueError("音乐标签未正确写入或清空，保留原音乐文件")
                    with temporary.open("rb") as stream: os.fsync(stream.fileno())
                    saved["prepared"] = library.signature(temporary)
                else:
                    saved["prepared"] = library.signature(path)
                if row["target"] != row["path"]:
                    actual = MusicInfo.from_meta(AudioMetadataHelper.read_tags(temporary if changed else path)).to_dict()
                    if naming.render(actual, path.suffix) != row["target"]:
                        raise ValueError("实际写入的名称标签与预览不一致，保留原音乐文件")
                saved["state"]="prepared"
                write_json(receipt,saved)
            except Exception:
                temporary.unlink(missing_ok=True); receipt.unlink(missing_ok=True)
                raise
        temporary = Path(saved["temporary"])
        destination = root/saved["target"]
        if saved["state"]=="prepared":
            library.target(config,row["path"])
            if row["target"] != row["path"]:
                naming.verify(root, row)
            local_lyrics.verify_edit(path, saved.get("lyrics_edit"))
            if saved["changed"]:
                if identity_matches(path,saved["prepared"]):
                    pass
                elif library.signature(path)==saved["original"] and identity_matches(temporary,saved["prepared"]):
                    os.replace(temporary,path); flush_directory(path.parent)
                else:
                    raise ValueError("音乐文件已变化，保留暂存数据并停止替换")
            elif not identity_matches(path,saved["prepared"]):
                raise ValueError("音乐文件已变化，已停止处理")
            saved["state"]="audio_done"; write_json(receipt,saved)
        if saved["state"]=="audio_done":
            if not identity_matches(path,saved["prepared"]):
                raise ValueError("音乐文件在写入后被其他程序替换")
            lyric = MusicLyrics.from_dict(saved["lyric"]) if saved["lyric"] else None
            if saved.get("lyrics_edit"):
                local_lyrics.apply_edit(path, saved["lyrics_edit"], identifier, write_sidecar)
                saved["lyrics_result"] = "旁挂歌词已保存"
            elif lyric:
                saved["lyrics_result"] = sidecar(path,lyric,saved["lyrics_policy"],saved["lyrics_simplified"],identifier)
            saved["state"]="lyrics_done"; write_json(receipt,saved)
        if saved["state"]=="lyrics_done" and destination != path:
            if not identity_matches(path, saved["prepared"]):
                raise ValueError("音乐文件在同步名称前已变化")
            saved["moves"] = naming.manifest(root, path, destination, config)
            saved["state"] = "moving"; write_json(receipt, saved)
        if saved["state"]=="moving":
            naming.link_files(root, saved["moves"], identity_matches, flush_directory)
            saved["state"] = "linked"; write_json(receipt, saved)
        if saved["state"]=="linked":
            naming.finish(root, saved["moves"], identity_matches, flush_directory)
        result={"path":row["path"],"target":row["target"],"success":True,"lyrics_elapsed":saved.get("lyrics_elapsed",0),"message":"音乐处理完成"+("，"+saved["lyrics_result"] if saved.get("lyrics_result") else "")}
        library.scan_one(config,row["target"],"updated")
        if destination != path:
            with library.database() as db:
                db.execute("DELETE FROM tracks WHERE root=? AND path=?", (str(root), row["path"]))
        write_json(receipt,{"state":"completed","result":result,"signature":library.signature(destination)})
        temporary.unlink(missing_ok=True)
        return result


def cleanup(config, body):
    """终止任务仅清理其暂存副本；已提交的音乐文件保留。"""
    identifier = body["task_id"]
    if not identifier or any(char not in "0123456789abcdef-" for char in identifier):
        raise ValueError("音乐任务标识无效")
    directory = get_runtime_setting("CACHE_PATH")/"operations"
    if not directory.exists():
        return {"success":True}
    with (directory/"write.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        for receipt in directory.glob(identifier+"-*.json"):
            saved = json.loads(receipt.read_text())
            if saved.get("state")!="completed":
                root,_=library.roots(config)
                if saved.get("state") == "moving":
                    naming.rollback(root, saved["moves"], identity_matches, flush_directory)
                elif saved.get("state") == "linked":
                    naming.finish(root, saved["moves"], identity_matches, flush_directory)
                    library.scan_one(config, saved["target"], "updated")
                    with library.database() as db:
                        db.execute("DELETE FROM tracks WHERE root=? AND path=?", (str(root), saved["path"]))
                path=root/saved["path"]
                if not path.is_relative_to(root) or ".." in path.parts or path.parent.resolve()!=path.parent:
                    raise ValueError("音乐暂存目录已变化，未清理")
                expected = path.with_name("."+path.name[:40]+".etk-music-"+receipt.stem)
                if str(expected)!=saved.get("temporary"):
                    raise ValueError("音乐暂存路径不符，未清理")
                expected.unlink(missing_ok=True)
                for extension in (".lrc", ".txt", ".lyricsfile.yaml"):
                    target=path.with_suffix(extension)
                    target.with_name("."+target.name[:40]+".etk-lyrics-"+receipt.stem).unlink(missing_ok=True)
            receipt.unlink()
            receipt.with_name(receipt.name+".new").unlink(missing_ok=True)
        for temporary_receipt in directory.glob(identifier+"-*.json.new"):
            temporary_receipt.unlink()
    return {"success":True}
