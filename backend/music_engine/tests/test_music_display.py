"""简体转换保留歌曲身份和来源缓存，不把不同艺术家串用。"""
from copy import deepcopy

from etk_music.catalog import serialize
from etk_music.models import MusicInfo
from etk_music.runtime import configure


def test_album_artist_uses_the_same_verified_chinese_alias(tmp_path):
    configure({}, tmp_path / 'cache')
    info = MusicInfo(title='Girls’ Generation', artists=['Girls’ Generation'],
                     album_artist='Girls’ Generation', artist_ids=['snsd'],
                     media_source='musicbrainz', media_id='recording',
                     raw_data={'artist-credit': [{'name': 'Girls’ Generation', 'artist': {
                         'id': 'snsd', 'name': 'Girls’ Generation',
                         'aliases': [{'name': '少女時代', 'locale': 'zh'}]}}]})
    before = deepcopy(info.to_dict())
    result = serialize(info)
    assert result['artists'] == ['少女时代']
    assert result['album_artist'] == '少女时代'
    assert result['title'] == 'Girls’ Generation'
    assert result['media_id'] == 'recording'
    assert info.to_dict() == before
    assert serialize(MusicInfo.from_dict(result)) == result


def test_album_artist_never_borrows_another_artist_alias(tmp_path):
    configure({}, tmp_path / 'cache')
    info = MusicInfo(artists=['Girls’ Generation'], artist_aliases=['少女時代'],
                     album_artist='Various Artists')
    assert serialize(info)['album_artist'] == 'Various Artists'
    configure({'music_metadata_to_simplified': False}, tmp_path / 'cache')
    info.album_artist = 'Girls’ Generation'
    assert serialize(info)['album_artist'] == 'Girls’ Generation'


def test_credit_conversion_keeps_roles_people_and_source_boundaries(tmp_path):
    configure({}, tmp_path / 'cache')
    info = MusicInfo(artists=['Girls’ Generation'], artist_ids=['snsd'], artist_aliases=['少女時代'],
                     composers=['羅大佑', 'Composer'], conductors=['심상원'], orchestras=['愛樂樂團'],
                     performers={'background vocals':['Girls’ Generation'], '電吉他':['김정배'],
                                 '电吉他':['張三'], 'strings':['K‐Strings']},
                     raw_data={'relations': [
                         {'type':'vocal', 'artist':{'id':'snsd', 'name':'Girls’ Generation'}},
                         {'type':'instrument', 'artist':{'id':'guitar', 'name':'김정배'}},
                         {'type':'performance', 'work':{'relations':[
                             {'type':'composer', 'artist':{'id':'composer', 'name':'Composer', 'aliases':[{'name':'作曲家中文名'}]}}
                         ]}},
                     ]})
    original = deepcopy(info.to_dict())
    result = serialize(info)
    assert result['composers'] == ['罗大佑', '作曲家中文名']
    assert result['conductors'] == ['심상원']
    assert result['orchestras'] == ['爱乐乐团']
    assert result['performers'] == {'background vocals':['少女时代'], '电吉他':['김정배','张三'], 'strings':['K‐Strings']}
    assert info.to_dict() == original
    configure({'music_metadata_to_simplified':False}, tmp_path / 'cache')
    assert serialize(info)['performers'] == original['performers']


def test_same_name_different_people_do_not_share_credit_aliases(tmp_path):
    configure({}, tmp_path / 'cache')
    info = MusicInfo(performers={'guitar':['Same Name']}, raw_data={'relations':[
        {'artist':{'id':'a', 'name':'Same Name', 'aliases':[{'name':'甲'}]}},
        {'artist':{'id':'b', 'name':'Same Name', 'aliases':[{'name':'乙'}]}},
    ]})
    assert serialize(info)['performers'] == {'guitar':['Same Name']}


def test_album_credits_keep_separate_people_and_names_with_slashes(tmp_path):
    from etk_music.musicbrainz import MusicBrainzModule
    configure({}, tmp_path / 'cache')
    def credit(identity, name, alias):
        return {'name':name, 'artist':{'id':identity, 'name':name, 'aliases':[{'name':alias,'locale':'zh'}]}}
    album_credits = [credit('a','Singer A','甲藝人'), credit('b','Band / Orchestra','乙樂團')]
    payload = {'id':'song','title':'Song','artist-credit':[credit('solo','Solo','獨唱藝人')],
               'releases':[{'id':'release','title':'Album','status':'Official', 'artist-credit':album_credits,
                            'release-group':{'id':'group','primary-type':'Album'}}]}
    info = MusicBrainzModule._recording_to_info(payload)
    result = serialize(info)
    assert result['artists'] == ['独唱艺人']
    assert result['album_artist'] == '甲艺人 / 乙乐团'
    album = MusicBrainzModule._release_to_album({'id':'release', 'release-group':{'id':'group'},
                                                'title':'Album','artist-credit':album_credits})
    track = MusicBrainzModule._track_to_info(album, {}, {'recording':payload})
    assert serialize(track)['album_artist'] == result['album_artist']
    assert payload['releases'][0]['artist-credit'] == album_credits
    # 没有专辑身份关系时，只有完整署名与歌曲艺术家列表相同才可以复用。
    fallback = MusicInfo(artists=['Singer A','Singer B'], artist_ids=['a','b'], album_artist='Singer A / Singer B',
                        raw_data={'artist-credit':[credit('a','Singer A','甲藝人'),credit('b','Singer B','乙藝人')]})
    assert serialize(fallback)['album_artist'] == '甲艺人 / 乙艺人'

    relation = {'type':'conductor', 'artist':{'id':'conductor','name':'Conductor',
                 'aliases':[{'name':'指揮家','locale':'zh'}]}}
    MusicBrainzModule._apply_selected_release(album, {'id':'release', 'relations':[relation]})
    assert serialize(album)['conductors'] == ['指挥家']


def test_explicit_japanese_alias_is_not_used_as_a_chinese_person_name(tmp_path):
    configure({}, tmp_path / 'cache')
    info = MusicInfo(artists=['Artist'], artist_ids=['person'], album_artist='Artist',
                     raw_data={'artist-credit':[{'artist':{'id':'person','name':'Artist','aliases':[
                         {'name':'日本別名','locale':'ja'}, {'name':'中文別名','locale':'zh-Hant'},
                     ]}}]})
    converted = serialize(info)
    assert converted['artists'] == ['中文别名']
    assert converted['album_artist'] == '中文别名'


def test_all_text_tags_and_verified_work_aliases_leave_ids_and_lyrics_intact(tmp_path):
    from etk_music.display import simplify_metadata_dict
    configure({}, tmp_path / 'cache')
    raw = {'media_source':'gd_netease', 'media_id':'opaque-id', 'music_type':'recording',
           'title':'Original title', 'title_aliases':['可信中文曲名'], 'album':'Original album',
           'album_aliases':['可信中文專輯'], 'artists':['樂團'], 'artist':'樂團', 'album_artist':'樂團',
           'composers':['作曲藝人'], 'conductors':['指揮藝人'], 'orchestras':['管弦樂團'],
           'performers':{'電吉他':['演奏藝人']}, 'version':'現場', 'comment':'備註', 'description':'專輯簡介',
           'lyrics':'繁體歌詞', 'cover_url':'https://example.invalid/圖片.jpg', 'isrc':'KRA300802931',
           'track_number':1, 'total_tracks':11, 'musicbrainz_release_id':'fixed-id', 'release_year':2007}
    before = deepcopy(raw)
    converted = simplify_metadata_dict(raw)
    for key, expected in {'title':'可信中文曲名','album':'可信中文专辑','artists':['乐团'],'artist':'乐团',
                          'album_artist':'乐团','composers':['作曲艺人'],'conductors':['指挥艺人'],
                          'orchestras':['管弦乐团'],'performers':{'电吉他':['演奏艺人']},'version':'现场',
                          'comment':'备注','description':'专辑简介'}.items():
        assert converted[key] == expected
    for key in ['media_source','media_id','lyrics','cover_url','isrc','track_number','total_tracks','musicbrainz_release_id','release_year']:
        assert converted[key] == raw[key]
    assert raw == before
    assert simplify_metadata_dict(converted) == converted


def test_credit_lookup_is_deduplicated_id_bound_and_optional(tmp_path, monkeypatch):
    from etk_music import catalog
    configure({}, tmp_path / 'cache')
    identity = '97960505-8dc3-4a42-a311-1767b12a1585'
    info = MusicInfo(media_source='musicbrainz', media_id='recording', artists=['Solo'], conductors=['Conductor'],
                     performers={'strings':['Conductor']}, raw_data={'relations':[
                         {'type':'conductor','artist':{'id':identity,'name':'Conductor'}},
                         {'type':'instrument','artist':{'id':identity,'name':'Conductor'}},
                     ]})
    calls = []
    class Provider:
        def _request_json(self, path, **kwargs):
            calls.append(path)
            return {'id':identity,'aliases':[{'name':'指揮家','locale':'zh'}]}
    monkeypatch.setattr(catalog, 'provider', lambda _:Provider())
    enriched = catalog.with_credit_aliases(info)
    assert calls == ['/artist/' + identity]
    assert serialize(enriched)['conductors'] == ['指挥家']
    assert serialize(enriched)['performers'] == {'strings':['指挥家']}
    assert 'aliases' not in info.raw_data['relations'][0]['artist']
    assert catalog.with_credit_aliases(enriched).to_dict() == enriched.to_dict()
    assert len(calls) == 1
    monkeypatch.setattr(Provider, '_request_json', lambda *a, **k:{'id':'wrong-person','aliases':[{'name':'错误人物'}]})
    assert serialize(catalog.with_credit_aliases(info))['conductors'] == ['Conductor']
    def fail(*a, **k):
        raise ValueError('source unavailable')
    monkeypatch.setattr(Provider, '_request_json', fail)
    assert serialize(catalog.with_credit_aliases(info))['performers'] == {'strings':['Conductor']}
