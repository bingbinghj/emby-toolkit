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


def test_selected_foreign_verse_preserves_chinese_and_unselected_repeats(monkeypatch):
    calls = []
    monkeypatch.setattr(lyrics_translate, '_translate', lambda text, provider: calls.append(text) or '请握住我的手')
    lyrics = '[ar:成龙 / 金喜善]\n[offset:150]\n[00:01.00]梦中人 熟悉的脸孔\n[00:08.00]<00:08.00>나의 <00:09.00>손을 잡아줘\n[00:12.00]나의 손을 잡아줘\n[00:16.00]万世沧桑 唯有爱是永远的神话\n'
    result = lyrics_translate.translate({'lyrics': lyrics, 'line_indices': [3]})
    assert calls == ['나의 손을 잡아줘']
    assert result['translation'] == lyrics.replace('[00:08.00]<00:08.00>나의 <00:09.00>손을 잡아줘', '[00:08.00]请握住我的手')
    assert result['bilingual'] == lyrics.replace('\n[00:12.00]', '\n[00:08.00]请握住我的手\n[00:12.00]')
    assert result['line_translations'] == [{'index': 3, 'text': '请握住我的手'}]


@pytest.mark.parametrize('indices', [[], [True], [-1], [2], ['1'], '1', None, [0]])
def test_invalid_or_empty_translation_selection_never_calls_provider(monkeypatch, indices):
    calls = []
    monkeypatch.setattr(lyrics_translate, '_translate', lambda *args: calls.append(args))
    with pytest.raises(ValueError, match='选择|正文'):
        lyrics_translate.translate({'lyrics': '[ar:歌手]\nHello', 'line_indices': indices})
    assert not calls


@pytest.mark.parametrize('foreign', ['Hold my hand', '私の手を握って', 'Возьми меня за руку', 'أمسك بيدي', 'จับมือฉัน', 'Toma mi mano', '夢'])
def test_translation_selection_is_language_independent(monkeypatch, foreign):
    calls = []
    monkeypatch.setattr(lyrics_translate, '_translate', lambda text, provider: calls.append((text, provider)) or '选中段落的译文')
    lyrics = f'[00:01]已有中文保持原样\n[00:05]{foreign}\n[00:10]{foreign}'
    result = lyrics_translate.translate({'lyrics': lyrics, 'line_indices': [1]})
    assert calls == [(foreign, 'youdao')]
    assert result['translation'] == f'[00:01]已有中文保持原样\n[00:05]选中段落的译文\n[00:10]{foreign}'
    assert result['line_translations'] == [{'index': 1, 'text': '选中段落的译文'}]
