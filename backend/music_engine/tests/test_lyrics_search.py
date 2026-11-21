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
    result=lyrics_search.search({'query':'Oh!','source':'gd_netease'})
    assert len(result['items'])==2 and result['items'][0]['id']=='lyric-1'
    assert len(calls)==1
    selected=lyrics_search.detail({'source':'gd_netease','id':'lyric-1'})
    assert [row['value'] for row in selected['variants']]==['original','translation']
    assert calls[-1]['id']=='lyric-1'
    lyrics_search.detail({'source':'gd_netease','id':'lyric-1'})
    assert len(calls)==2


def test_gd_rate_limit_is_shared_and_does_not_wait(monkeypatch):
    for _ in range(45): lyrics_search._reserve_gd()
    with pytest.raises(ValueError,match='较频繁'): lyrics_search._reserve_gd()
    with connection() as db: assert db.execute('SELECT COUNT(*) FROM lyrics_requests').fetchone()[0]==45


def test_unavailable_source_invalid_id_and_no_lyric(monkeypatch):
    with pytest.raises(ValueError,match='来源'):lyrics_search.search({'source':'apple','query':'song'})
    with pytest.raises(ValueError,match='ID'):lyrics_search.detail({'source':'gd_netease','id':'../1'})
    with pytest.raises(ValueError,match='请输入'):lyrics_search.search({'query':None})
    monkeypatch.setattr(lyrics_search,'_gd',lambda p:{'lyric':'','tlyric':''})
    with pytest.raises(ValueError,match='没有可用歌词'):lyrics_search.detail({'source':'gd_netease','id':'1'})


def test_lrclib_manual_search_preserves_live_versions_and_omits_instrumentals(monkeypatch):
    rows=[{'id':i,'trackName':'Song','artistName':'Artist','albumName':'Live' if i else 'Studio','duration':199+i,'syncedLyrics':'[00:01]text'} for i in range(22)]
    rows.append({'id':23,'instrumental':True})
    monkeypatch.setattr(lyrics_search.LrclibModule,'_request_json',lambda *a,**k:rows)
    first=lyrics_search.search({'source':'lrclib','query':'song'})
    second=lyrics_search.search({'source':'lrclib','query':'song','page':2})
    assert first['items'][0]['album']=='Studio' and first['items'][1]['album']=='Live'
    assert first['has_more'] and len(second['items'])==2 and not second['has_more']


def test_direct_netease_and_gd_never_switch_channels(monkeypatch):
    from etk_music import streaming_metadata, gd_metadata
    calls=[]
    monkeypatch.setattr(streaming_metadata,'search',lambda source,*args: calls.append(source) or {'items':[{'media_id':'11','title':'歌曲'}]})
    monkeypatch.setattr(gd_metadata,'_gd',lambda params:calls.append(params) or [{'id':'22','name':'GD 歌曲','source':'netease'}])
    assert lyrics_search.search({'source':'netease','query':'歌曲'})['items'][0]['source']=='netease'
    assert lyrics_search.search({'source':'gd_netease','query':'歌曲'})['items'][0]['source']=='gd_netease'
    assert calls[0]=='netease' and calls[1]['source']=='netease'
    assert lyrics_search.search({'source':'gd_netease','kind':'album','query':'歌曲'})['items'] == []
    monkeypatch.setattr(lyrics_search,'read_json',lambda url,params,label: calls.append(url) or {'code':200,'lrc':{'lyric':'[00:01]官方'},'tlyric':{'lyric':'[00:01]译文'}})
    result=lyrics_search.detail({'source':'netease','id':'11'})
    assert result['variants'][0]['text']=='[00:01]官方' and 'music.163.com/api/song/lyric' in calls[-1]


def test_gd_tag_metadata_and_cover_use_only_gd(monkeypatch):
    from etk_music import gd_metadata
    calls=[]
    def response(params):
        calls.append(params)
        return [{'id':'11','name':'GD 曲名','artist':['歌手'],'album':'专辑','pic_id':'cover-1','source':'netease'}] if params['types']=='search' else {'url':'https://example.test/gd.jpg'}
    monkeypatch.setattr(gd_metadata,'_gd',response)
    item=gd_metadata.search('gd_netease','歌曲')['items'][0]
    result=gd_metadata.detail(item['media_source'],item['media_id'])
    assert result['title']=='GD 曲名' and result['cover_url']=='https://example.test/gd.jpg'
    assert calls==[{'types':'search','source':'netease','name':'歌曲','count':20,'pages':1},{'types':'pic','source':'netease','id':'cover-1','size':500}]
    with pytest.raises(ValueError,match='过期'):gd_metadata.detail('gd_joox','11')
    album=gd_metadata.search('gd_netease','专辑','album')['items'][0]
    assert gd_metadata.detail('gd_netease',album['media_id'],'album')['tracks'][0]['media_id']=='11'


@pytest.mark.parametrize('source,channel',[('gd_netease','netease'),('gd_joox','joox'),('gd_bilibili','bilibili')])
def test_gd_album_groups_and_lyric_ids_stay_with_selected_provider(monkeypatch,source,channel):
    from etk_music import gd_metadata
    calls=[]
    def response(params):
        calls.append(params)
        return [{'id':'song1','lyric_id':'lyric1','name':'第一首','artist':['歌手'],'album':'现场版','source':channel},
                {'id':'song2','name':'第二首','artist':['歌手'],'album':'现场版','source':channel},
                {'id':'song3','name':'其他版本','artist':['歌手'],'album':'录音室版','source':channel}]
    monkeypatch.setattr(gd_metadata,'_gd',response)
    albums=lyrics_search.search({'source':source,'query':'专辑','kind':'album'})['items']
    assert len(albums)==2
    selected=lyrics_search.search({'source':source,'query':'专辑','kind':'album','album_id':albums[0]['id']})
    assert [row['id'] for row in selected['items']]==['lyric1','song2']
    assert selected['track_scope']=='search_results'
    assert len(calls)==1 and calls[0]['source']==channel


def test_lrclib_album_search_groups_without_mixing_artists(monkeypatch):
    monkeypatch.setattr(lyrics_search.LrclibModule,'_request_json',lambda *args,**kwargs:[
        {'id':1,'trackName':'One','artistName':'A','albumName':'Live','plainLyrics':'Words'},
        {'id':2,'trackName':'Two','artistName':'B','albumName':'Live','plainLyrics':'Other'}])
    albums=lyrics_search.search({'source':'lrclib','query':'Live','kind':'album'})['items']
    assert len(albums)==2
    songs=lyrics_search.search({'source':'lrclib','query':'Live','album_id':albums[0]['id']})['items']
    assert [row['id'] for row in songs]==['1']
