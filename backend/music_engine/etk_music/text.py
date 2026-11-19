"""与 MP 一致的中文字形转换。"""
import warnings
with warnings.catch_warnings():
    # zhconv 仍使用已锁定 setuptools 提供的资源读取接口；避免每个任务重复记录依赖弃用提示。
    warnings.filterwarnings("ignore", message="pkg_resources is deprecated as an API.*", category=UserWarning)
    from zhconv import convert as convert

__all__ = ["convert"]
