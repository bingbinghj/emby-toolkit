import pytest
from etk_music import lyrics_search
from etk_music.cache import connection


def test_search_keeps_versions_and_fetches_only_selected_lyrics(monkeypatch):
    calls=[]
    def read(url, params, label):
        calls.append(params)
        if params['types']=='search':
            return [{'id':'1','lyric_id':'lyric-1','name':'Oh!','artist':['歌手'],'album':'现场版','source':'netease'},
                    {'id':'2','name':'Oh!','artist':['歌手'],'album':'录音室版','source':'netease'}]
        return {'lyric':'[00:13.00]原文','tlyric':'[00:13.00]译文'}
    monkeypatch.setattr(lyrics_search,'read_json',read)
    result=lyrics_search.search({'query':'Oh!','source':'netease'})
    assert len(result['items'])==2 and result['items'][0]['id']=='lyric-1'
    assert len(calls)==1
    selected=lyrics_search.detail({'source':'netease','id':'lyric-1'})
    assert [row['value'] for row in selected['variants']]==['original','translation']
    assert calls[-1]['id']=='lyric-1'
    lyrics_search.detail({'source':'netease','id':'lyric-1'})
    assert len(calls)==2


def test_gd_rate_limit_is_shared_and_does_not_wait(monkeypatch):
    for _ in range(45): lyrics_search._reserve_gd()
    with pytest.raises(ValueError,match='较频繁'): lyrics_search._reserve_gd()
    with connection() as db: assert db.execute('SELECT COUNT(*) FROM lyrics_requests').fetchone()[0]==45


def test_unavailable_source_invalid_id_and_no_lyric(monkeypatch):
    with pytest.raises(ValueError,match='来源'):lyrics_search.search({'source':'apple','query':'song'})
    with pytest.raises(ValueError,match='ID'):lyrics_search.detail({'source':'netease','id':'../1'})
    with pytest.raises(ValueError,match='请输入'):lyrics_search.search({'query':None})
    monkeypatch.setattr(lyrics_search,'_gd',lambda p:{'lyric':'','tlyric':''})
    with pytest.raises(ValueError,match='没有可用歌词'):lyrics_search.detail({'source':'netease','id':'1'})


def test_lrclib_manual_search_preserves_live_versions_and_omits_instrumentals(monkeypatch):
    rows=[{'id':i,'trackName':'Song','artistName':'Artist','albumName':'Live' if i else 'Studio','duration':199+i,'syncedLyrics':'[00:01]text'} for i in range(22)]
    rows.append({'id':23,'instrumental':True})
    monkeypatch.setattr(lyrics_search.LrclibModule,'_request_json',lambda *a,**k:rows)
    first=lyrics_search.search({'source':'lrclib','query':'song'})
    second=lyrics_search.search({'source':'lrclib','query':'song','page':2})
    assert first['items'][0]['album']=='Studio' and first['items'][1]['album']=='Live'
    assert first['has_more'] and len(second['items'])==2 and not second['has_more']
