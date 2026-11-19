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
    action = request["action"]
    if action in {"library", "scope", "scan", "scan_batch", "scan_finish", "inspect", "image", "preview_groups"}:
        from etk_music import library
    elif action in {"preview", "execute", "cleanup"}:
        from etk_music import operations
    else:
        from etk_music import catalog
    if action=="library":
        return library.listing(config, body.get("query", ""), int(body.get("page", 1)), int(body.get("page_size", 50)))
    if action=="scope":
        return {"paths": library.walk(config)}
    if action=="preview_groups":
        return library.preview_groups(body["paths"])
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
        return catalog.serialize(catalog.detail(body["source"], body["id"], body.get("kind", "recording"), body.get("release")))
    if action=="related":
        return {"items": catalog.related(body["source"], body["id"], body["kind"], int(body.get("page",1)), body.get("album_type"))}
    if action=="preview":
        return operations.preview(config, body)
    if action=="cleanup":
        return operations.cleanup(config, body)
    if action=="execute":
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
