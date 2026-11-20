import pytest
from etk_music import lyrics_translate


@pytest.mark.parametrize('provider',['youdao','bing','google'])
def test_translation_preserves_line_timing_repeats_and_metadata(monkeypatch,provider):
    calls=[]
    monkeypatch.setattr(lyrics_translate,'_translate',lambda text,source:calls.append((text,source)) or '你好\n爱你')
    result=lyrics_translate.translate({'provider':provider,'lyrics':'[ar:歌手]\n[offset:100]\n[00:01.00][00:05.00]Hello\n[00:02.00]<00:02.00>Love <00:02.50>you\n[00:08.00]Hello'})
    assert calls==[('Hello\nLove you',provider)]
    assert result['translation']=='[ar:歌手]\n[offset:100]\n[00:01.00][00:05.00]你好\n[00:02.00]爱你\n[00:08.00]你好'
    assert '[00:02.00]<00:02.00>Love <00:02.50>you\n[00:02.00]爱你' in result['bilingual']
    lyrics_translate.translate({'provider':provider,'lyrics':'[00:01.00]Hello\n[00:02.00]Love you'})
    assert len(calls)==1


def test_bad_translation_never_gets_cached_or_applied(monkeypatch):
    monkeypatch.setattr(lyrics_translate,'_translate',lambda *args:'合成了一行')
    with pytest.raises(ValueError,match='行数'):lyrics_translate.translate({'lyrics':'Hello\nLove you'})
    monkeypatch.setattr(lyrics_translate,'_translate',lambda *args:'你好\n爱你')
    assert lyrics_translate.translate({'lyrics':'Hello\nLove you'})['translation']=='你好\n爱你'
    with pytest.raises(ValueError,match='翻译服务'):lyrics_translate.translate({'provider':'baidu','lyrics':'Hello'})
