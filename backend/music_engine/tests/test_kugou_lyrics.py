import base64
import json
import zlib

import pytest

from etk_music import lyrics_search


HASH = '78A0E1AF0E5D7260F0FB881AF1C57616'
KEY = bytes([64, 71, 97, 119, 94, 50, 116, 71, 81, 54, 49, 45, 206, 210, 110, 105])


def encode_krc(text):
    compressed = zlib.compress(text.encode())
    return base64.b64encode(b'krc1' + bytes(value ^ KEY[index % 16] for index, value in enumerate(compressed))).decode()


def document(blocks):
    language = base64.b64encode(json.dumps({'content': blocks}, ensure_ascii=False).encode()).decode()
    return '[offset:150]\n[language:' + language + ']\n[1000,400]<0,400,0>Hello\n[2000,200]\n[3000,900]<0,300,0>My <300,600,0>world'


def upstream(monkeypatch, krc):
    calls = []
    def read(url, params, label):
        if url.endswith('/search'):
            return {'candidates': [{'id': '123', 'accesskey': 'key'}]}
        calls.append(params['fmt'])
        if params['fmt'] == 'krc':
            if isinstance(krc, Exception):
                raise krc
            return {'content': krc}
        return {'content': base64.b64encode(b'[00:01.00]Hello').decode()}
    monkeypatch.setattr(lyrics_search, 'read_json', read)
    return calls


def test_krc_exposes_translation_and_romanization_with_original_row_times(monkeypatch):
    blocks = [
        {'type': 0, 'language': 0, 'lyricContent': [['哈喽'], [''], ['麦 ', '沃尔德']]},
        {'type': 1, 'language': 0, 'lyricContent': [['你好'], [''], ['我的世界']]},
    ]
    calls = upstream(monkeypatch, encode_krc(document(blocks)))
    result = lyrics_search.detail({'source': 'kugou', 'id': HASH})
    variants = {row['value']: row['text'] for row in result['variants']}
    assert set(variants) == {'original', 'translation', 'romanized'}
    assert '[00:01.000]Hello' in variants['original']
    assert '[00:03.000]My world' in variants['original']
    assert variants['translation'] == '[offset:150]\n[00:01.000]你好\n[00:03.000]我的世界'
    assert variants['romanized'] == '[offset:150]\n[00:01.000]哈喽\n[00:03.000]麦 沃尔德'
    assert '[language:' not in variants['original'] and '<0,' not in variants['original']
    assert calls == ['krc']


@pytest.mark.parametrize('blocks', [
    [{'type': 1, 'lyricContent': [['错位译文']]}],
    [{'type': 1, 'lyricContent': [['你好'], None, ['世界']]}],
    [{'type': 1, 'lyricContent': [['你好\n[00:09]错位'], [''], ['世界']]}],
    [{'type': 1, 'lyricContent': [[''], [''], ['']]}],
    [{'type': 9, 'lyricContent': [['你好'], [''], ['世界']]}],
    None,
])
def test_invalid_or_empty_language_blocks_do_not_shift_lyrics(monkeypatch, blocks):
    calls = upstream(monkeypatch, encode_krc(document(blocks)))
    result = lyrics_search.detail({'source': 'kugou', 'id': HASH})
    assert [row['value'] for row in result['variants']] == ['original']
    assert '[00:03.000]My world' in result['variants'][0]['text']
    assert calls == ['krc']


@pytest.mark.parametrize('payload', [
    '', 'invalid base64!', base64.b64encode(b'krc1broken').decode(),
    encode_krc('x' * (1024 * 1024 + 1)),
    encode_krc('[ti:没有歌词行]'),
    ValueError('上游不提供 KRC'),
])
def test_unusable_krc_falls_back_to_lrc(monkeypatch, payload):
    calls = upstream(monkeypatch, payload)
    result = lyrics_search.detail({'source': 'kugou', 'id': HASH})
    assert result['variants'] == [{'value': 'original', 'title': '原文', 'text': '[00:01.00]Hello'}]
    assert calls == ['krc', 'lrc']


def test_invalid_language_json_keeps_original_krc(monkeypatch):
    calls = upstream(monkeypatch, encode_krc('[language:bad]\n[1000,100]<0,100,0>Hello'))
    result = lyrics_search.detail({'source': 'kugou', 'id': HASH})
    assert result['variants'] == [{'value': 'original', 'title': '原文', 'text': '[00:01.000]Hello'}]
    assert calls == ['krc']
