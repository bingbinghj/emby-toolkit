import pytest
from etk_music import streaming


def test_chart_cache_reuses_directory_across_pages(monkeypatch):
    calls = []
    def get(url, params=None, label=None):
        calls.append(url)
        if url.endswith('/toplist'):
            return {'list':[{'id':1,'name':'新歌榜'}]}
        return {'result':{'tracks':[{'id':n,'name':f'歌曲 {n}','artists':[{'name':'歌手'}],'album':{'name':'专辑','picUrl':'http://example.com/a.jpg'},'duration':1000} for n in range(65)]}}
    monkeypatch.setattr(streaming, 'read_json', get)
    first = streaming.explore({'source':'netease'})
    second = streaming.explore({'source':'netease','chart':'1','page':2})
    last = streaming.explore({'source':'netease','chart':'1','page':3})
    assert len(calls) == 2
    assert first['total'] == 65 and first['items'][0]['duration'] == 1
    assert second['items'][0]['media_id'] == '30'
    assert last['items'][-1]['chart_rank'] == 65 and not last['has_more']
    assert first['items'][0]['cover_url'] == 'https://example.com/a.jpg'


def test_native_qq_pagination_preserves_chart_period(monkeypatch):
    calls=[]
    def qq(method, params):
        calls.append(params)
        if method == 'GetAll': return {'group':[{'toplist':[{'topId':62,'title':'飙升榜','period':'2026-11-20'}]}]}
        return {'data':{'totalNum':61},'songInfoList':[{'id':31,'mid':'song-mid','title':'歌曲','singer':[{'name':'歌手'}],'album':{'mid':'a','pmid':'a_1','name':'专辑'},'interval':123}]}
    monkeypatch.setattr(streaming, '_qq', qq)
    result=streaming.explore({'source':'qqmusic','page':2})
    assert calls[-1] == {'topid':62,'period':'2026-11-20','offset':30,'num':30}
    assert result['items'][0]['duration']==123 and result['has_more']
    assert result['items'][0]['cover_url'].endswith('a_1.jpg')


def test_kuwo_uses_song_duration_not_days_on_chart(monkeypatch):
    monkeypatch.setattr(streaming,'_get',lambda url, params=None: {'child':[{'child':[{'sourceid':'16','name':'热歌榜','child':[]}]}]} if 'bang/list' in url else {'musiclist':[{'id':'1','name':'歌名','artist':'歌手','song_duration':'189','duration':'7'}],'num':'1'})
    result=streaming.explore({'source':'kuwo'})
    assert result['items'][0]['duration']==189 and not result['has_more']


def test_kugou_native_page_and_hash(monkeypatch):
    seen=[]
    def get(url, params=None):
        seen.append(params)
        return {'rank':{'list':[{'rankid':8888,'rankname':'TOP500'}]}} if url.endswith('/list') else {'songs':{'total':32,'list':[{'hash':'ABC','songname':'歌名','authors':[{'author_name':'歌手'}],'duration':90,'album_sizable_cover':'http://imge.kugou.com/{size}/a.jpg'}]}}
    monkeypatch.setattr(streaming,'_get',get)
    result=streaming.explore({'source':'kugou','page':2})
    assert seen[-1]['page']==2
    assert result['items'][0]['media_id']=='ABC'
    assert result['items'][0]['cover_url']=='https://imge.kugou.com/400/a.jpg'


def test_apple_region_is_restricted_and_paginated(monkeypatch):
    calls=[]
    def get(url, params=None):
        calls.append(url)
        return {'feed':{'results':[{'id':str(n),'name':'歌名','artistName':'歌手','url':'https://music.apple.com/a','genres':[]} for n in range(32)]}}
    monkeypatch.setattr(streaming,'_get',get)
    result=streaming.explore({'source':'apple','region':'jp','page':2})
    assert '/jp/' in calls[0] and len(result['items'])==2 and not result['has_more']
    with pytest.raises(ValueError,match='地区'):
        streaming.explore({'source':'apple','region':'../../config'})


def test_missing_or_stale_chart_is_an_error(monkeypatch):
    monkeypatch.setattr(streaming,'_get',lambda *a: {'list':[{'id':1,'name':'热歌榜'}]})
    with pytest.raises(ValueError,match='榜单已不可用'):
        streaming.explore({'source':'netease','chart':'999'})
    with pytest.raises(ValueError,match='榜单暂时不可用'):
        streaming.explore({'source':'netease'})


def test_provider_errors_are_not_cached_as_success(monkeypatch):
    replies=iter([{'code':503},{'code':200,'list':[{'id':1,'name':'热歌榜'}]}])
    monkeypatch.setattr(streaming,'read_json',lambda *args:next(replies))
    with pytest.raises(ValueError,match='暂时不可用'):
        streaming.charts('netease')
    assert streaming.charts('netease')==[{'value':'1','title':'热歌榜'}]
    assert streaming.charts('netease')==[{'value':'1','title':'热歌榜'}]
