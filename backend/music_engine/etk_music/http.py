"""音乐来源共用 HTTP 会话和超时；返回接口兼容 MP 来源模块。"""
import asyncio
import requests
from .runtime import request_timeout


class RequestUtils:
    def __init__(self, headers=None, ua=None, proxies=None, timeout=20, **_):
        self.session = requests.Session()
        self.session.headers.update(headers or {})
        if ua:
            self.session.headers["User-Agent"] = ua
        if proxies:
            self.session.proxies.update(proxies)
        self.timeout = timeout

    def get_res(self, url, **kwargs):
        timeout = request_timeout(kwargs.pop("timeout", None) or self.timeout)
        if timeout<=0:
            return None
        try:
            return self.session.get(url, timeout=timeout, **kwargs)
        except requests.RequestException:
            return None

    def post_res(self, url, **kwargs):
        timeout = request_timeout(kwargs.pop("timeout", None) or self.timeout)
        if timeout<=0:
            return None
        try:
            return self.session.post(url, timeout=timeout, **kwargs)
        except requests.RequestException:
            return None

    def close(self):
        self.session.close()


class AsyncRequestUtils(RequestUtils):
    async def get_res(self, url, **kwargs):
        response = await asyncio.to_thread(super().get_res, url, **kwargs)
        if response is not None:
            async def close():
                response.close()
            response.aclose = close
        return response

    async def post_res(self, url, **kwargs):
        response = await asyncio.to_thread(super().post_res, url, **kwargs)
        if response is not None:
            async def close():
                response.close()
            response.aclose = close
        return response
