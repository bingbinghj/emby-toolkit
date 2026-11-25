from unittest.mock import Mock

import pytest

from etk_music import catalog
from etk_music.models import MusicInfo


MBID = '949fe498-457d-401d-a271-6dce887283d2'


@pytest.mark.parametrize('kind', ['album', 'recording'])
def test_musicbrainz_id_uses_exact_lookup_in_selected_entity(monkeypatch, kind):
    module = Mock()
    module.recognize_music.return_value = MusicInfo(
        media_source='musicbrainz', media_id=MBID, music_type=kind, title='Oh!', year=2010)
    monkeypatch.setattr(catalog, 'provider', lambda source: module)
    result = catalog.search(f'  {MBID.upper()}  ', ['musicbrainz'], kind)
    assert result['errors'] == {}
    assert [(item['media_id'], item['music_type'], item['title']) for item in result['items']] == [(MBID, kind, 'Oh!')]
    module.recognize_music.assert_called_once_with('musicbrainz', MBID, music_type=kind)
    module.search_music.assert_not_called()


def test_unknown_musicbrainz_id_does_not_fall_back_to_name_search(monkeypatch):
    module = Mock()
    module.recognize_music.return_value = None
    monkeypatch.setattr(catalog, 'provider', lambda source: module)
    assert catalog.search(MBID, ['musicbrainz'], 'album')['items'] == []
    module.search_music.assert_not_called()


def test_musicbrainz_id_lookup_reports_source_errors(monkeypatch):
    module = Mock()
    module.recognize_music.side_effect = ValueError('资料源暂时不可用')
    monkeypatch.setattr(catalog, 'provider', lambda source: module)
    result = catalog.search(MBID, ['musicbrainz'], 'album')
    assert result == {'items': [], 'errors': {'musicbrainz': '资料源暂时不可用'}}


@pytest.mark.parametrize('source,query', [('musicbrainz', 'Oh! (2010)'), ('musicbrainz', '1999'), ('theaudiodb', MBID)])
def test_names_and_other_sources_keep_their_search_path(monkeypatch, source, query):
    module = Mock()
    module.search_music.return_value = []
    monkeypatch.setattr(catalog, 'provider', lambda source: module)
    assert catalog.search(query, [source], 'album') == {'items': [], 'errors': {}}
    assert module.search_music.call_args.kwargs['music_types'] == ['album']
    module.recognize_music.assert_not_called()
