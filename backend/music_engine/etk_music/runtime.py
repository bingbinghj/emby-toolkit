"""音乐模块的 ETK 宿主适配；不启动 MoviePilot 或连接其数据库。"""
import asyncio
import logging
import time
from contextvars import ContextVar
from contextlib import contextmanager
from pathlib import Path
from types import SimpleNamespace

logger = logging.getLogger("etk.music")
_settings = {
    "USER_AGENT": "EmbyToolkit/Music (https://github.com/hbq0405/emby-toolkit)",
    "CONF": SimpleNamespace(musicbrainz=512, theaudiodb=512, listenbrainz=128, meta=86400),
    "PROXY": None,
    "MUSIC_CUE_ENABLE": True,
    "MUSIC_METADATA_TO_SIMPLIFIED": True,
    "MUSIC_COVER_PROXY": "",
    "MUSIC_RELEASE_REGION_PRIORITY": "CN,TW,HK",
    "MUSIC_RELEASE_SCRIPT_PRIORITY": "Hans,Hant,Latn",
    "ACOUSTID_API_KEY": "b1auxfOzAg",
    "THEAUDIODB_API_KEY": "123",
    "LRCLIB_BASE_URL": "https://lrclib.net",
    "AMLL_BASE_URL": "https://api.amll.dev",
    "LYRICS_PROVIDER_RETRY_MAX_WAIT": 5,
    "NORMAL_USER_AGENT": "Mozilla/5.0",
}

_DEFAULTS = dict(_settings)


def configure(config, cache_dir):
    """每次请求使用 ETK 已保存的音乐配置。"""
    _settings.clear()
    _settings.update(_DEFAULTS)
    for key in _settings:
        value = config.get(key.lower())
        if value is not None and key not in {"CONF", "USER_AGENT", "PROXY"}:
            _settings[key] = value
    _settings["CACHE_PATH"] = Path(cache_dir)
    _settings["CACHE_PATH"].mkdir(parents=True, exist_ok=True)


def get_runtime_setting(key, default=None):
    return _settings.get(key, default)


def get_metainfo_accelerator():
    return None


async def run_in_threadpool(function, *args, **kwargs):
    return await asyncio.to_thread(function, *args, **kwargs)


class _ModuleBase:
    """来源模块所需的生命周期基类。"""

    def __init__(self):
        pass

    def init_module(self):
        pass


_http_deadline = ContextVar("music_http_deadline", default=None)

@contextmanager
def request_budget(seconds):
    deadline = time.monotonic() + max(float(seconds), 0)
    token = _http_deadline.set(min(deadline, _http_deadline.get() or deadline))
    try:
        yield
    finally:
        _http_deadline.reset(token)

def request_timeout(timeout):
    deadline = _http_deadline.get()
    return min(float(timeout), max(0, deadline-time.monotonic())) if deadline else timeout
