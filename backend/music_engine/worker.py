"""ETK Rust 后端通过 JSON 标准输入调用的本地音乐进程。"""
import json
import logging
import sys
import signal
import os
from etk_music.runtime import configure


def dispatch(request):
    config, body = request.get("config", {}), request.get("body", {})
    configure(config, request["cache_dir"])
    if request["action"] in {"edit_metadata", "preview"}:
        from etk_music.audio import use_music_cue
        enabled, track_match = body.get("music_cue_enable"), body.get("music_track_match") or "strict"
        if enabled is not None and not isinstance(enabled, bool):
            raise ValueError("识别 CUE 设置无效")
        if track_match not in {"strict", "ignore_duration", "sequential"}:
            raise ValueError("曲目对齐方式无效")
        with use_music_cue(enabled):
            return _dispatch(request)
    return _dispatch(request)


def _dispatch(request):
    """资料读取与预览共用本次识别策略，确认写入沿用已保存的预览。"""
    config, body = request.get("config", {}), request.get("body", {})
    action = request["action"]
    streaming_sources = {"netease", "qqmusic", "apple", "kugou", "kuwo"}
    manual_source = body.get("source")
    if action == "search":
        manual_source = (body.get("sources") or [None])[0]
    elif action == "edit_metadata":
        manual_source = (body.get("selection") or {}).get("media_source")
    if action in {"search", "detail", "edit_metadata"} and manual_source in streaming_sources | {"gd_netease", "gd_joox", "gd_bilibili"}:
        from etk_music.display import simplify_metadata_dict
        if manual_source.startswith("gd_"):
            from etk_music import gd_metadata as streaming_metadata
        else:
            from etk_music import streaming_metadata
        from etk_music.runtime import request_budget
        with request_budget(30):
            if action == "search":
                result = streaming_metadata.search(manual_source, body.get("query"), body.get("kind") or "recording", body.get("page",1), body.get("count",20))
                return {**result, "items": [simplify_metadata_dict(item) for item in result.get("items", [])]}
            if action == "edit_metadata":
                from etk_music import library
                from etk_music.audio import AudioMetadataHelper
                path = library.target(config, body["path"])
                meta = AudioMetadataHelper.read(path)
                if meta.organization_error:
                    raise ValueError(meta.organization_error)
                selected = body["selection"]
                if selected.get("music_type") == "album":
                    raise ValueError("请在专辑中选择对应歌曲")
                return simplify_metadata_dict(streaming_metadata.detail(manual_source, selected["media_id"]))
            return simplify_metadata_dict(streaming_metadata.detail(manual_source, body["id"], body.get("kind") or "recording"))
    if action in {"library", "selection", "scope", "scan", "scan_batch", "scan_finish", "inspect", "image", "preview_groups", "audio_path"}:
        from etk_music import library
    elif action in {"preview", "execute", "cleanup", "clear_comments_preview"}:
        from etk_music import operations
    elif action == "lyrics_translate":
        from etk_music.lyrics_translate import translate
        return translate(body)
    elif action in {"lyrics_search", "lyrics_detail"}:
        from etk_music import lyrics_search
        from etk_music.runtime import request_budget
        with request_budget(25):
            return (lyrics_search.search if action == "lyrics_search" else lyrics_search.detail)(body)
    elif action == "explore" and body.get("source") in {"netease", "qqmusic", "apple", "kugou", "kuwo"}:
        from etk_music import streaming
        return streaming.explore(body)
    else:
        from etk_music import catalog
    if action=="library":
        if body.get("directory") is not None and not body.get("query"):
            return library.browse(config, body["directory"], int(body.get("page", 1)), int(body.get("page_size", 50)))
        return library.listing(config, body.get("query", ""), int(body.get("page", 1)), int(body.get("page_size", 50)))
    if action=="selection":
        paths = body.get("paths")
        if not isinstance(paths, list) or not 1 <= len(paths) <= 1000 or any(not isinstance(path, str) for path in paths):
            raise ValueError("请选择音乐文件或目录")
        return library.selection(config, paths)
    if action=="scope":
        return {"paths": library.walk(config)}
    if action=="preview_groups":
        return library.preview_groups(body["paths"], body.get("music_type"), body.get("collection_root"))
    if action=="scan":
        return library.scan_one(config, body["path"], body["scan_id"])
    if action=="scan_batch":
        results, cue_cache = [], {}
        for path in body["paths"]:
            try:
                results.append({**library.scan_one(config, path, body["scan_id"], cue_cache), "success": True})
            except Exception as error:
                results.append({"path": path, "success": False, "message": str(error)})
        return results
    if action=="scan_finish":
        library.finish_scan(config, body["scan_id"])
        return {"success": True}
    if action=="inspect":
        return library.inspect(config, body["path"])
    if action=="image":
        return library.image(config, body["path"])
    if action=="audio_path":
        return {"path": str(library.target(config, body["path"]).resolve(strict=True))}
    if action=="search":
        sources = body.get("sources") or config.get("music_sources") or ["musicbrainz"]
        if not isinstance(sources,list) or not 1<=len(sources)<=3 or any(source not in {"musicbrainz","theaudiodb","doubanmusic"} for source in sources):
            raise ValueError("请选择有效音乐资料源")
        if body.get("kind") not in {None, "recording", "album", "artist"}:
            raise ValueError("音乐资料类型无效")
        query = body.get("query") or ""
        if not isinstance(query,str) or len(query)>500:
            raise ValueError("音乐搜索词过长")
        return catalog.search(query, sources, body.get("kind"), min(int(body.get("count", 30)),100))
    if action=="explore":
        return catalog.explore(body)
    if action=="detail":
        return catalog.serialize(catalog.with_credit_aliases(catalog.detail(body["source"], body["id"], body.get("kind", "recording"), body.get("release"))))
    if action=="edit_metadata":
        from etk_music import library
        path = library.target(config, body["path"])
        _, info = catalog.recognize(path, config.get("music_sources") or ["musicbrainz"], body["selection"], body.get("music_track_match") or "strict")
        return catalog.serialize(catalog.with_credit_aliases(info))
    if action=="related":
        return {"items": catalog.related(body["source"], body["id"], body["kind"], int(body.get("page",1)), body.get("album_type"))}
    if action=="preview":
        return operations.preview(config, body)
    if action=="clear_comments_preview":
        return operations.clear_comments_preview(config)
    if action=="cleanup":
        return operations.cleanup(config, body)
    if action=="execute":
        from etk_music.audio import use_music_cue
        with use_music_cue(body.get("item", {}).get("music_cue_enable")):
            return operations.execute(config, body)
    if action=="cache":
        module = catalog.provider("musicbrainz")
        if body.get("clear"):
            module.music_cache_clear()
        elif body.get("keys"):
            for key in body["keys"]:
                module.music_cache_delete(key)
        elif body.get("key"):
            if not module.music_cache_delete(body["key"]):
                raise ValueError("音乐识别缓存不存在")
        return {"items": module.music_cache_items()}
    raise ValueError("未知音乐操作")


if __name__=="__main__":
    def terminate(*_):
        # Rust 为本任务建立独立进程组；宿主退出时一并停止 fpcalc。
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        if hasattr(os, "getpgrp") and os.getpgrp() == os.getpid():
            os.killpg(os.getpgrp(), signal.SIGTERM)
        sys.exit(143)
    signal.signal(signal.SIGTERM, terminate)
    if sys.platform.startswith("linux"):
        # 宿主异常退出时终止音乐 worker，避免后台继续修改本地文件。
        import ctypes
        host = int(os.environ.get("ETK_MUSIC_HOST_PID", os.getppid()))
        if ctypes.CDLL(None).prctl(1, signal.SIGTERM) != 0:
            sys.exit("无法注册音乐进程退出通知")
        if os.getppid() != host:
            sys.exit(1)
    logging.basicConfig(stream=sys.stderr, level=logging.WARNING, format="%(message)s")
    try:
        result = dispatch(json.load(sys.stdin))
        json.dump({"success": True, "data": result}, sys.stdout, ensure_ascii=False, default=str)
    except Exception as error:
        logging.exception("音乐处理失败")
        json.dump({"success": False, "error": str(error)}, sys.stdout, ensure_ascii=False)
        sys.exit(1)
