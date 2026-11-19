"""有界 SQLite 音乐缓存，供独立来源进程复用，不保存 pickle。"""
import functools
from contextlib import contextmanager
import hashlib
import inspect
import json
import sqlite3
import time
from typing import Protocol
from .runtime import get_runtime_setting


class CacheControls(Protocol):
    def cache_clear(self): ...


@contextmanager
def connection():
    database = get_runtime_setting("CACHE_PATH") / "cache.sqlite3"
    db = sqlite3.connect(database, timeout=10)
    db.execute("CREATE TABLE IF NOT EXISTS entries(region TEXT,key TEXT,value TEXT,expires REAL,PRIMARY KEY(region,key))")
    db.execute("DELETE FROM entries WHERE expires <= ?", (time.time(),))
    try:
        with db:
            yield db
    finally:
        db.close()


class Cache:
    def __init__(self, maxsize=1024, ttl=3600, region="http"):
        self.maxsize, self.ttl, self.region = maxsize, ttl, region

    def get(self, key, region=None):
        with connection() as db:
            row = db.execute("SELECT value FROM entries WHERE region=? AND key=? AND expires>?", (region or self.region, key, time.time())).fetchone()
        return json.loads(row[0]) if row else None

    def set(self, key, value, ttl=None, region=None):
        region = region or self.region
        with connection() as db:
            db.execute("INSERT OR REPLACE INTO entries VALUES(?,?,?,?)", (region, key, json.dumps(value, ensure_ascii=False), time.time() + (ttl if ttl is not None else self.ttl)))
            db.execute("DELETE FROM entries WHERE region=? AND key IN (SELECT key FROM entries WHERE region=? ORDER BY expires DESC LIMIT -1 OFFSET ?)", (region, region, self.maxsize))

    def clear(self, region=None):
        with connection() as db:
            db.execute("DELETE FROM entries WHERE region=?", (region or self.region,))

    def delete(self, key):
        value = self.get(key)
        with connection() as db:
            db.execute("DELETE FROM entries WHERE region=? AND key=?", (self.region, key))
        return value

    def items(self):
        with connection() as db:
            rows = db.execute("SELECT key,value FROM entries WHERE region=? AND expires>?", (self.region, time.time())).fetchall()
        return [(key, json.loads(value)) for key, value in rows]


def cached(maxsize=1024, ttl=3600, region=None, skip_none=False, skip_empty=False, empty_ttl=None, empty_if=None, shared_key=None, **_):
    def decorate(function):
        cache = Cache(maxsize, ttl, region or shared_key or function.__module__ + ":" + function.__name__)
        def key_for(args, kwargs):
            values = [arg for arg in args if isinstance(arg, (str, int, float, bool, tuple, list, dict)) or arg is None]
            return hashlib.sha256(json.dumps([values, kwargs, [get_runtime_setting(key) for key in ("THEAUDIODB_API_KEY", "LRCLIB_BASE_URL", "AMLL_BASE_URL")]], ensure_ascii=False, sort_keys=True, default=str).encode()).hexdigest()
        def save(key, value):
            if (value is not None or not skip_none) and (value or not skip_empty):
                cache.set(key, {"value": value}, ttl=empty_ttl if empty_if and empty_if(value) else ttl)
        @functools.wraps(function)
        def sync(*args, **kwargs):
            key = key_for(args, kwargs)
            hit = cache.get(key)
            if hit is not None:
                return hit["value"]
            value = function(*args, **kwargs)
            save(key, value)
            return value
        @functools.wraps(function)
        async def asynchronous(*args, **kwargs):
            key = key_for(args, kwargs)
            hit = cache.get(key)
            if hit is not None:
                return hit["value"]
            value = await function(*args, **kwargs)
            save(key, value)
            return value
        wrapped = asynchronous if inspect.iscoroutinefunction(function) else sync
        wrapped.cache_clear = cache.clear
        return wrapped
    return decorate


class MusicBrainzCache:
    """识别键沿用 MP 的实体和证据隔离规则。"""
    def __init__(self):
        self.cache = Cache(512, 86400, "recognition")

    @staticmethod
    def key(meta, music_type=None):
        from .meta import music_credit_values
        source = str(meta.media_source) if meta.media_source else None
        identity = ["id", source, str(meta.media_id)] if source and meta.media_id else ["meta", source, meta.title, list(meta.artists or []), meta.album, meta.year, meta.version, meta.isrc]
        credits = {key: value for key, value in music_credit_values(meta).items() if value}
        if not meta.media_id and credits:
            identity.append(credits)
        return "[音乐:v4]" + hashlib.sha256(json.dumps([music_type, identity], ensure_ascii=False, separators=(",", ":")).encode()).hexdigest()

    def get(self, meta, music_type=None):
        from .models import MusicInfo
        value = self.cache.get(self.key(meta, music_type))
        return MusicInfo.from_dict(value) if value else None

    def update(self, meta, info, music_type=None):
        if not info or not meta:
            return
        if not info.media_id and info.raw_data.get("recognition", {}).get("status") in {"service_error", "budget_exhausted", "ambiguous", "conflict"}:
            return
        value = info.to_dict()
        value["raw_data"] = {key: value for key, value in value.get("raw_data", {}).items() if key == "isrcs"}
        self.cache.set(self.key(meta, music_type), value, ttl=86400 if info.media_id else 300)

    def list_items(self):
        return [{"key": key, **value} for key, value in self.cache.items()]

    def delete(self, key):
        return self.cache.delete(key) or {}

    def clear(self):
        self.cache.clear()

    def save(self, force=False):
        pass


def reserve_request_delay(interval):
    """多个音乐进程共用发送时刻，避免绕过 MusicBrainz 公共接口限速。"""
    import fcntl
    from .observation import music_wait_allowed
    path = get_runtime_setting("CACHE_PATH") / "musicbrainz-rate.lock"
    with path.open("a+") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        stream.seek(0)
        try:
            previous = float(stream.read() or "0")
        except ValueError:
            previous = 0
        now = time.time()
        scheduled = max(now, min(previous, now + 60) + interval)
        delay = max(0, scheduled - now)
        if not music_wait_allowed(delay):
            return -1
        stream.seek(0)
        stream.truncate()
        stream.write(str(scheduled))
        stream.flush()
        return delay
