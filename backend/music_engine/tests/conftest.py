import pytest
from etk_music.runtime import configure

@pytest.fixture(autouse=True)
def music_runtime(tmp_path):
    configure({}, tmp_path / "cache")
