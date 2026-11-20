import pytest
from etk_music import streaming_metadata as metadata, lyrics_search


def test_netease_album_drilldown_keeps_native_ids_and_reuses_requests(monkeypatch):
    calls=[]
    def read(url, params, label):
        calls.append(url)
        if 'search' in url:
            return {'result':{'albums':[{'id':1,'name':'现场版','artist':{'name':'歌手'},'picUrl':'https://example.com/cover.jpg'}],'albumCount':1}}
        return {'code':200,'album':{'id':1,'name':'现场版','artist':{'name':'歌手'},'picUrl':'https://example.com/cover.jpg','songs':[{'id':2,'name':'歌曲','artists':[{'name':'歌手'}],'album':{'id':1,'name':'现场版'},'no':3}]}}
    monkeypatch.setattr(metadata,'read_json',read)
    albums=lyrics_search.search({'source':'netease','query':'现场版','kind':'album'})
    assert albums['items'][0]['id']=='1'
    tracks=lyrics_search.search({'source':'netease','query':'现场版','kind':'album','album_id':'1'})
    assert tracks['items'][0]['id']=='2' and tracks['items'][0]['track_number']==3
    assert tracks['items'][0]['cover_url']=='https://example.com/cover.jpg'
    selected=metadata.detail('netease','2')
    assert selected['album_artist']=='歌手' and selected['total_tracks']==1
    metadata.detail('netease','1','album')
    assert len(calls)==2


@pytest.mark.parametrize('response',[{'code':0,'subcode':-10001,'message':'禁止跨域访问'},{}])
def test_provider_errors_are_not_cached_as_empty_results(monkeypatch,response):
    calls=[]
    def read(*args):
        calls.append(args)
        return response
    monkeypatch.setattr(metadata,'read_json',read)
    for _ in range(2):
        with pytest.raises(ValueError,match='暂时不可用|无效数据'):
            metadata.search('qqmusic','歌曲')
    assert len(calls)==2


def test_qq_album_song_identity_is_mid_and_lyrics_use_same_identity(monkeypatch):
    row={'songid':123,'songmid':'songMID','songname':'曲名','albummid':'albumMID','albumname':'专辑','singer':[{'name':'歌手'}],'interval':200,'belongCD':3}
    song=metadata.qq_track(row)
    assert song['media_id']=='songMID' and song['track_number']==3
    assert song['cover_url'].endswith('albumMID.jpg')
    def get(url, params):
        assert params['songmid']=='songMID'
        return {'lyric':'[00:01]原文','trans':'[00:01]译文'}
    monkeypatch.setattr(metadata,'get',get)
    assert len(lyrics_search.detail({'source':'qqmusic','id':'songMID'})['variants'])==2


def test_kuwo_album_tracks_retain_fractional_lyric_timing(monkeypatch):
    monkeypatch.setattr(lyrics_search,'read_json',lambda *_:{'data':{'lrclist':[{'time':'125.25','lineLyric':'歌词'}]}})
    assert lyrics_search.detail({'source':'kuwo','id':'2'})['variants'][0]['text']=='[02:05.25]歌词'


def test_kugou_cached_selection_preserves_album_cover_and_title():
    song=metadata.remember({'media_source':'kugou','music_type':'recording','media_id':'ABC123','title':'曲名','artists':['歌手'],'album':'专辑','cover_url':'https://example.com/cover.jpg'})
    assert metadata.detail('kugou','ABC123')==song
    with pytest.raises(ValueError,match='ID'):metadata.detail('kugou','../ABC')


def test_apple_does_not_return_preview_audio_or_confuse_album_track_ids(monkeypatch):
    monkeypatch.setattr(metadata,'get',lambda *_:{'results':[{'wrapperType':'collection','collectionId':1,'collectionName':'专辑','artistName':'歌手'},
        {'wrapperType':'track','kind':'song','trackId':2,'trackName':'曲名','collectionId':1,'artistName':'歌手','previewUrl':'https://example.com/audio','trackNumber':2}]})
    album=metadata.detail('apple','1','album')
    assert album['media_id']=='1' and album['tracks'][0]['media_id']=='2'
    assert 'previewUrl' not in album['tracks'][0]
