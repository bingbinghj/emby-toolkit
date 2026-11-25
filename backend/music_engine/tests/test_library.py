"""真实音频验证做种保护、选中范围、无变化跳过及重启边界。"""
import os
import json
import subprocess
import shutil
import wave
import pytest
from etk_music import library, operations
from etk_music.audio import AudioMetadataHelper
from etk_music.meta import MetaMusic
from etk_music.runtime import configure


@pytest.fixture
def fixture(tmp_path):
    seed, root = tmp_path/"seed", tmp_path/"library"
    seed.mkdir(); root.mkdir()
    configure({}, tmp_path/"cache")
    config = {"music_library_path":str(root),"music_seed_path":str(seed),"music_scraping_policies":{"tags":"missingOnly","cover":"skip","lyrics":"skip"},"music_metadata_to_simplified":False}
    source = seed/"original.wav"
    with wave.open(str(source),"wb") as stream:
        stream.setnchannels(2);stream.setsampwidth(2);stream.setframerate(44100);stream.writeframes(b"\0"*176400)
    assert AudioMetadataHelper.write(source,MetaMusic(title="原曲名",artists=["歌手"],album="专辑",track_number=1))
    return config,source,root


def plan(config,path,title="新曲名"):
    result=operations.preview(config,{"operation":"edit","paths":[path],"metadata":{"title":title}})
    assert result["items"][0]["success"],result
    return result["items"][0]


def test_editor_receives_saved_search_type_without_changing_file_identity(fixture):
    config, source, root = fixture
    os.link(source, root / 'song.wav')
    original = source.read_bytes()
    for setting, expected in [(None, 'album'), ('recording', 'recording'), ('album', 'album')]:
        if setting is not None:
            config['music_search_type'] = setting
        inspected = library.inspect(config, 'song.wav')
        assert inspected['music_search_type'] == expected
        assert inspected['title'] == '原曲名'
        assert (root / 'song.wav').samefile(source)
        assert source.read_bytes() == original


def test_invalid_cue_preview_explains_recovery_without_writes(fixture):
    config, source, root = fixture
    os.link(source, root / "song.wav")
    (root / "song.cue").write_text('FILE "song.wav" WAVE\n TRACK 01 AUDIO\n TITLE "损坏 CUE"\n', encoding="utf8")
    before = source.read_bytes()
    row = operations.preview(config, {"operation": "edit", "paths": ["song.wav"], "metadata": {"title": "新标题"}})["items"][0]
    assert row["success"] is False
    assert "CUE" in row["message"]
    assert row["failure_stage"] == "recognition"
    assert row["recovery_action"] == "检查 CUE 文件内容，或关闭「识别 CUE」后重试"
    assert (root / "song.wav").samefile(source)
    assert source.read_bytes() == before


def test_missing_path_is_not_misclassified_as_cue_error(fixture):
    config, _, _ = fixture
    row = operations.preview(config, {"operation": "edit", "paths": ["CUE/song.wav"]})["items"][0]
    assert row["success"] is False
    assert "recovery_action" not in row


@pytest.mark.parametrize("kind",["hardlink","symlink","copy"])
def test_write_preserves_seed_and_independent_copy_can_be_edited_again(fixture,kind):
    config,source,root=fixture
    target=root/"renamed.wav"
    if kind=="hardlink":os.link(source,target)
    elif kind=="symlink":target.symlink_to(source)
    else:target.write_bytes(source.read_bytes())
    before=source.read_bytes()
    row=plan(config,"renamed.wav")
    result=operations.execute(config,{"item":row,"operation":"edit","execution_id":"abcd-1"})
    assert result["success"]
    assert source.read_bytes()==before
    assert result["target"] == "renamed.wav"
    target = root/result["target"]
    assert AudioMetadataHelper.read(target).title=="新曲名"
    assert not target.samefile(source) and not target.is_symlink()
    second = operations.execute(config,{"item":plan(config,result["target"],"再次编辑"),"operation":"edit","execution_id":"abcd-2"})
    assert AudioMetadataHelper.read(root/second["target"]).title=="再次编辑"
    assert source.read_bytes()==before


def test_unchanged_metadata_keeps_hardlink(fixture):
    config,source,root=fixture
    os.link(source,root/"song.wav")
    row=plan(config,"song.wav","原曲名")
    operations.execute(config,{"item":row,"operation":"edit","execution_id":"abcd-3"})
    assert source.samefile(root/"song.wav")
    assert not list(root.glob(".*etk-music*"))


@pytest.mark.parametrize("container", ["mp3", "flac", "wav", "m4a", "alac", "wma", "ogg", "opus", "aiff", "wv", "tta", "aac", "dsf", "dff", "ape", "mpc"])
@pytest.mark.skipif(shutil.which('ffmpeg') is None, reason='扩展格式及音频流哈希验证需要 ffmpeg')
def test_comment_description_preview_edit_clear_preserves_seed(fixture, container):
    from mutagen import File
    from mutagen.id3 import COMM
    config, original, root = fixture
    source = original.with_suffix('.' + container)
    structural = {'dsf', 'dff', 'ape', 'mpc'}
    if container in structural:
        from test_audio_containers import _write_dsf, _write_dff, _write_ape, _write_mpc
        {'dsf':_write_dsf, 'dff':_write_dff, 'ape':_write_ape, 'mpc':_write_mpc}[container](source)
    elif container == 'alac':
        subprocess.run(['ffmpeg', '-v', 'error', '-i', str(original), '-c:a', 'alac', '-f', 'ipod', str(source)], check=True)
    elif container != 'wav':
        subprocess.run(['ffmpeg', '-v', 'error', '-i', str(original), str(source)], check=True)
    assert AudioMetadataHelper.write(source, MetaMusic(comment='Ripped by test', description='现场版\n专辑说明'))
    if container == 'wav':
        audio = File(source)
        audio.tags.add(COMM(encoding=3, lang='eng', desc='iTunNORM', text=['keep normalization']))
        audio.save()
    target = root / ('song.' + container)
    os.link(source, target)
    original_bytes = source.read_bytes()
    def audio_hash(path):
        return subprocess.check_output(['ffmpeg','-v','error','-i',str(path),'-map','0:a:0','-c:a','copy','-f','hash','-'])
    original_audio = audio_hash(source) if container not in structural else None
    inspected = library.inspect(config, target.name)
    assert inspected['comment'] == 'Ripped by test'
    assert inspected['description'] == '现场版\n专辑说明'
    for index, text in enumerate(['新的备注\n第二行', '']):
        row = operations.preview(config, {'operation': 'edit', 'paths': [target.name], 'metadata': {
            'signature': library.inspect(config, target.name)['signature'], 'comment': text, 'description': text,
        }})['items'][0]
        assert row['success'], row
        assert row['before']['comment'] == ('Ripped by test' if index == 0 else '新的备注\n第二行')
        assert row['metadata']['comment'] == text
        assert 'comment' in row['tag_updates'] and 'description' in row['tag_updates']
        assert library.inspect(config, target.name)['comment'] == row['before']['comment']
        assert operations.execute(config, {'item': row, 'operation': 'edit', 'execution_id': f'cafe-{index}'})['success']
        result = library.inspect(config, target.name)
        assert (result['comment'] or '') == (result['description'] or '') == text
        assert source.read_bytes() == original_bytes
        if original_audio:
            assert audio_hash(target) == original_audio
        if container == 'wav':
            assert File(target).tags.getall('COMM:iTunNORM')[0].text == ['keep normalization']


@pytest.mark.parametrize('kind', ['hardlink', 'symlink', 'copy'])
def test_clear_comments_task_scope_noop_replay_and_seed_protection(fixture, kind):
    config, source, root = fixture
    assert AudioMetadataHelper.write(source, MetaMusic(comment='抓轨备注', description='简介'))
    target = root / 'song.wav'
    if kind == 'hardlink': os.link(source, target)
    elif kind == 'symlink': target.symlink_to(source)
    else: target.write_bytes(source.read_bytes())
    before = source.read_bytes()
    untouched = root / 'untouched.wav'
    untouched.write_bytes(before)
    assert AudioMetadataHelper.write(untouched, MetaMusic(comment='', description=''))
    untouched_identity = library.signature(untouched)
    (root / 'broken.flac').write_bytes(b'broken audio')
    (root / 'escape').symlink_to(source.parent, target_is_directory=True)
    (root / 'cover.jpg').write_bytes(b'not a song')
    rows = {row['path']: row for row in operations.clear_comments_preview(config)}
    assert set(rows) == {'song.wav', 'untouched.wav', 'broken.flac'}
    assert not rows['broken.flac']['success']
    assert rows['untouched.wav']['skip']
    assert rows['song.wav']['metadata'] == {'comment': '', 'description': ''}
    assert target.read_bytes() == source.read_bytes() == before
    body = {'item': rows['song.wav'], 'operation': 'edit', 'execution_id': 'cafe-3'}
    assert operations.execute(config, body)['success']
    saved_identity = library.signature(target)
    assert operations.execute(config, body)['success']
    assert library.signature(target) == saved_identity
    assert library.signature(untouched) == untouched_identity
    meta = AudioMetadataHelper.read(target)
    assert not meta.comment and not meta.description
    assert (meta.title, meta.artists, meta.album) == ('原曲名', ['歌手'], '专辑')
    assert source.read_bytes() == before and not target.samefile(source)


def test_unchanged_multiple_comments_do_not_rewrite_hardlink(fixture):
    from mutagen.wave import WAVE
    from mutagen.id3 import COMM
    config, source, root = fixture
    audio = WAVE(source)
    audio.tags.add(COMM(encoding=3, lang='eng', desc='', text=['First', 'Second']))
    audio.save()
    os.link(source, root/'song.wav')
    data = library.inspect(config, 'song.wav')
    assert data['comment'] == 'First\nSecond'
    row = operations.preview(config, {'operation':'edit','paths':['song.wav'],'metadata':data})['items'][0]
    assert row['success'] and not row['tag_updates'], row
    assert operations.execute(config, {'item':row,'operation':'edit','execution_id':'cafe-4'})['success']
    assert source.samefile(root/'song.wav')


def test_clear_legacy_id3_comment_alias(fixture):
    from mutagen.wave import WAVE
    from mutagen.id3 import TXXX
    config, source, root = fixture
    audio = WAVE(source)
    audio.tags.add(TXXX(encoding=3, desc='COMMENT', text=['legacy comment']))
    audio.save()
    os.link(source, root/'song.wav')
    before = source.read_bytes()
    assert library.inspect(config, 'song.wav')['comment'] == 'legacy comment'
    row = operations.clear_comments_preview(config)[0]
    assert not row['skip']
    assert operations.execute(config, {'item':row,'operation':'edit','execution_id':'cafe-5'})['success']
    assert not library.inspect(config, 'song.wav')['comment']
    assert source.read_bytes() == before


def test_clear_comments_refuses_silent_tag_write_failure(fixture, monkeypatch):
    config, source, root = fixture
    assert AudioMetadataHelper.write(source, MetaMusic(comment='保留原备注'))
    os.link(source, root/'song.wav')
    original = source.read_bytes()
    row = operations.clear_comments_preview(config)[0]
    monkeypatch.setattr(AudioMetadataHelper, 'write', lambda *_args, **_kwargs: True)
    with pytest.raises(ValueError, match='未正确写入'):
        operations.execute(config, {'item':row,'operation':'edit','execution_id':'cafe-6'})
    assert (root/'song.wav').samefile(source)
    assert source.read_bytes() == original
    assert not list(root.glob('.*etk-music*'))


def test_uploaded_cover_uses_existing_atomic_write_without_touching_seed(fixture):
    import base64
    config,source,root=fixture
    os.link(source,root/'song.wav')
    original=source.read_bytes()
    png=base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/lS0AAAAASUVORK5CYII=')
    value='data:image/png;base64,'+base64.b64encode(png).decode()
    row=operations.preview(config,{'operation':'edit','paths':['song.wav'],'metadata':{'cover_upload':value}})['items'][0]
    assert row['success'],row
    assert source.samefile(root/'song.wav') and source.read_bytes()==original
    operations.execute(config,{'item':row,'operation':'edit','execution_id':'abcd-19'})
    assert source.read_bytes()==original and not source.samefile(root/'song.wav')
    assert base64.b64decode(library.image(config,'song.wav')['data'])==png
    assert AudioMetadataHelper.read(root/'song.wav').title=='原曲名'
    for bad in ['data:image/svg+xml;base64,YQ==','data:image/png;base64,YQ==']:
        result=operations.preview(config,{'operation':'edit','paths':['song.wav'],'metadata':{'cover_upload':bad}})
        assert not result['items'][0]['success']


def test_rescrape_renames_only_selected_track(fixture,monkeypatch):
    config,source,root=fixture
    config["music_rename_on_save"] = True
    os.link(source,root/"first.wav");os.link(source,root/"second.wav")
    original=source.read_bytes()
    from etk_music.models import MusicInfo
    monkeypatch.setattr(operations.catalog,"recognize",lambda *_:(AudioMetadataHelper.read(source),MusicInfo(title="刮削标题",artists=["歌手"],album="专辑",media_source="musicbrainz",media_id="a")))
    config["music_scraping_policies"]["tags"]="overwrite"
    preview=operations.preview(config,{"operation":"rescrape","paths":["first.wav"]})
    assert len(preview["items"])==1 and preview["items"][0]["target"]=="歌手/专辑/01 - 刮削标题.wav"
    operations.execute(config,{"item":preview["items"][0],"operation":"rescrape","execution_id":"abcd-4"})
    assert AudioMetadataHelper.read(root/preview["items"][0]["target"]).title=="刮削标题"
    assert source.read_bytes()==(root/"second.wav").read_bytes()==original


def test_seed_directory_traversal_and_symlink_directory_are_rejected(fixture):
    config,source,root=fixture
    with pytest.raises(ValueError):library.target(config,str(source))
    with pytest.raises(ValueError):library.target(config,"../seed/original.wav")
    (root/"escape").symlink_to(source.parent,target_is_directory=True)
    with pytest.raises(ValueError):library.target(config,"escape/original.wav")
    assert library.walk(config)==[]


def test_changed_file_is_rejected_before_tag_write(fixture):
    config,source,root=fixture
    os.link(source,root/"song.wav")
    row=plan(config,"song.wav")
    (root/"song.wav").unlink();(root/"song.wav").write_bytes(source.read_bytes())
    with pytest.raises(ValueError,match="已变化"):operations.execute(config,{"item":row,"operation":"edit","execution_id":"abcd-5"})
    assert AudioMetadataHelper.read(source).title=="原曲名"


def test_failure_during_tag_write_preserves_library_link(fixture,monkeypatch):
    config,source,root=fixture
    os.link(source,root/"song.wav");row=plan(config,"song.wav")
    original=source.read_bytes()
    monkeypatch.setattr(AudioMetadataHelper,"write",lambda *_a,**_k:False)
    with pytest.raises(ValueError,match="写入失败"):operations.execute(config,{"item":row,"operation":"edit","execution_id":"abcd-6"})
    assert source.samefile(root/"song.wav") and source.read_bytes()==original
    assert not list(root.glob(".*etk-music*"))


def test_resume_after_atomic_replacement_does_not_write_again(fixture,monkeypatch):
    config,source,root=fixture
    os.link(source,root/"song.wav");row=plan(config,"song.wav")
    write=operations.write_json
    def crash(path,data):
        if data.get("state")=="audio_done":raise RuntimeError("模拟替换后进程退出")
        write(path,data)
    monkeypatch.setattr(operations,"write_json",crash)
    body={"item":row,"operation":"edit","execution_id":"abcd-7"}
    with pytest.raises(RuntimeError):operations.execute(config,body)
    changed=(root/"song.wav").stat().st_ino
    monkeypatch.setattr(operations,"write_json",write)
    assert operations.execute(config,body)["success"]
    assert (root/row["target"]).stat().st_ino==changed
    assert AudioMetadataHelper.read(source).title=="原曲名"


def test_local_search_uses_only_library_index(fixture):
    config,source,root=fixture
    os.link(source,root/"song.wav")
    library.scan_one(config,"song.wav","scan")
    assert library.listing(config,"歌手")["total"]==1
    assert library.listing(config,"original.wav")["total"]==0
    assert library.listing(config,"%_")["total"]==0


def test_browse_uses_real_artist_album_disc_tree_and_deduplicates_selection(fixture):
    config, source, root = fixture
    album = root/'歌手'/'专辑'/'Disc 1'
    album.mkdir(parents=True)
    os.link(source, album/'01.wav')
    (album/'01.lrc').write_text('[00:01.00]歌词')
    (root/'不允许的目录').symlink_to(source.parent, target_is_directory=True)
    assert [item['name'] for item in library.browse(config)['items']] == ['歌手']
    assert [item['name'] for item in library.browse(config, '歌手')['items']] == ['专辑']
    songs = library.browse(config, '歌手/专辑/Disc 1')['items']
    assert len(songs) == 1 and songs[0]['name'] == '01.wav'
    assert library.selection(config, ['歌手','歌手/专辑/Disc 1/01.wav'])['paths'] == ['歌手/专辑/Disc 1/01.wav']
    with pytest.raises(ValueError): library.browse(config, '../seed')
    with pytest.raises(ValueError): library.selection(config, ['不允许的目录'])


def test_folder_displays_all_tracks_reuses_tags_and_hides_sidecars(fixture, monkeypatch):
    config, source, root = fixture
    for name in ['01.wav', '02.wav']:
        os.link(source, root/name)
    (root/'cover.jpg').write_bytes(b'cover')
    (root/'01.lrc').write_text('[00:01.00]歌词')
    first = library.browse(config, page_size=1)
    assert first['total'] == 2 and len(first['items']) == 1
    assert [row['title'] for row in first['tracks']] == ['原曲名', '原曲名']
    read = AudioMetadataHelper.read_tags
    monkeypatch.setattr(AudioMetadataHelper, 'read_tags', lambda *_: pytest.fail('未变化的标签不应重复读取'))
    assert library.browse(config, page=2, page_size=1)['tracks'] == first['tracks']
    monkeypatch.setattr(AudioMetadataHelper, 'read_tags', read)
    assert AudioMetadataHelper.write(root/'01.wav', MetaMusic(title='改后标题'), overwrite=True)
    assert library.browse(config)['tracks'][0]['title'] == '改后标题'


def test_manual_lyrics_atomic_save_preserves_seed_audio_and_linked_lyrics(fixture):
    config, source, root = fixture
    os.link(source, root/'song.wav')
    seed_lyrics = source.with_suffix('.lrc')
    seed_lyrics.write_text('[00:01.00]原歌词\n')
    os.link(seed_lyrics, root/'song.lrc')
    original = source.read_bytes()
    editor = library.inspect(config, 'song.wav')
    assert editor['lyrics'] == seed_lyrics.read_text()
    editor['lyrics'] = '[00:03.50]修改后的歌词\n'
    row = operations.preview(config, {'operation':'edit','paths':['song.wav'],'metadata':editor})['items'][0]
    assert row['success'], row
    result = operations.execute(config, {'item':row,'operation':'edit','execution_id':'aacd-1'})
    assert result['success']
    assert (root/'song.lrc').read_text() == editor['lyrics']
    assert seed_lyrics.read_text() == '[00:01.00]原歌词\n'
    assert source.read_bytes() == original and source.samefile(root/'song.wav')
    assert not seed_lyrics.samefile(root/'song.lrc')


def test_manual_lyrics_changed_after_preview_rejects_before_audio_changes(fixture):
    config, source, root = fixture
    os.link(source, root/'song.wav')
    (root/'song.lrc').write_text('[00:01.00]原歌词')
    editor = library.inspect(config, 'song.wav')
    editor.update(title='新的标题', lyrics='[00:03.00]新歌词')
    row = operations.preview(config, {'operation':'edit','paths':['song.wav'],'metadata':editor})['items'][0]
    (root/'song.lrc').write_text('[00:02.00]外部修改')
    with pytest.raises(ValueError, match='歌词在预览后已变化'):
        operations.execute(config, {'item':row,'operation':'edit','execution_id':'aacd-2'})
    assert source.samefile(root/'song.wav')
    assert (root/'song.lrc').read_text() == '[00:02.00]外部修改'


def test_editor_signature_survives_browser_json_and_still_detects_file_changes(fixture):
    config, source, root = fixture
    target = root/'song.wav'
    os.link(source, target)
    os.utime(target, ns=(1794406299898650601, 1794406299898650601))
    before = source.read_bytes()
    inspected = library.inspect(config, 'song.wav')
    # 实际经过 JavaScript JSON.parse/stringify，覆盖超过 2^53 的纳秒时间。
    editor = json.loads(subprocess.run(
        ['node', '-e', 'process.stdout.write(JSON.stringify(JSON.parse(require("fs").readFileSync(0,"utf8"))))'],
        input=json.dumps(inspected), text=True, capture_output=True, check=True,
    ).stdout)
    editor['lyrics'] = '[00:03.50]浏览器修改后的歌词'
    row = operations.preview(config, {'operation':'edit','paths':['song.wav'],'metadata':editor})['items'][0]
    assert row['success'], row
    assert not target.with_suffix('.lrc').exists()
    assert source.read_bytes() == before
    operations.execute(config, {'item':row,'operation':'edit','execution_id':'aacd-b0'})
    assert target.with_suffix('.lrc').read_text().rstrip() == editor['lyrics'].rstrip()
    assert source.read_bytes() == before
    os.utime(target, ns=(1794406299898650602, 1794406299898650602))
    rejected = operations.preview(config, {'operation':'edit','paths':['song.wav'],'metadata':editor})['items'][0]
    assert not rejected['success'] and '已变化' in rejected['message']


def test_manual_edit_all_mp_identity_and_credit_tags(fixture):
    config, source, root = fixture
    os.link(source, root/'song.wav')
    ids = ['12345678-1234-1234-1234-123456789ab'+digit for digit in 'cdef']
    values = dict(media_source='musicbrainz',music_type='recording',media_id=ids[0],musicbrainz_release_id=ids[1],musicbrainz_release_group_id=ids[2],musicbrainz_release_track_id=ids[3], performers={'guitar':['演奏者']})
    row = operations.preview(config, {'operation':'edit','paths':['song.wav'],'metadata':values})['items'][0]
    assert row['success'], row
    operations.execute(config, {'item':row,'operation':'edit','execution_id':'aacd-3'})
    result = library.inspect(config,'song.wav')
    for key in values:
        assert result[key] == values[key], key


@pytest.mark.parametrize("container", ["flac", "mp3", "wav", "m4a", "wma", "ogg", "opus", "aiff", "wv", "tta", "aac"])
def test_explicit_empty_labels_are_previewed_cleared_and_isolated(fixture, container):
    config, original, root = fixture
    source = original.with_suffix('.' + container)
    if container != 'wav':
        subprocess.run(['ffmpeg', '-v', 'error', '-i', str(original), str(source)], check=True)
    labels = dict(composers=['? ??'], conductors=['指挥'], orchestras=['乐团'], performers={'guitar':['演奏者']})
    assert AudioMetadataHelper.write(source, MetaMusic(**labels))
    target = root / ('song.' + container)
    os.link(source, target)
    original_bytes = source.read_bytes()
    audio_hash = lambda path: subprocess.check_output(['ffmpeg','-v','error','-i',str(path),'-map','0:a:0','-c:a','copy','-f','hash','-'])
    original_audio = audio_hash(source)
    empty = dict(composers=[], conductors=None, orchestras=[], performers={})
    row = operations.preview(config, {'operation':'edit','paths':[target.name],'metadata':empty})['items'][0]
    assert row['success'], row
    assert all(row['tag_updates'][key] == [] for key in ['composer','conductor','orchestra','performer'])
    assert row['before']['composers'] == ['? ??']
    assert target.samefile(source)  # 预览不改变硬链接或标签。
    result = operations.execute(config, {'item':row,'operation':'edit','execution_id':'aacd-e0'})
    assert result['success']
    after = library.inspect(config, target.name)
    assert all(not after[key] for key in empty)
    assert AudioMetadataHelper.read_tags(target).title == row['before']['title']
    assert source.read_bytes() == original_bytes and not target.samefile(source)
    assert audio_hash(target) == original_audio
    assert operations.execute(config, {'item':row,'operation':'edit','execution_id':'aacd-e0'}) == result
    second = operations.preview(config, {'operation':'edit','paths':[target.name],'metadata':empty})['items'][0]
    assert second['tag_updates'] == {}


def test_missing_credit_is_preserved_and_empty_title_does_not_rename(fixture):
    config, source, root = fixture
    assert AudioMetadataHelper.write(source, MetaMusic(composers=['可信作曲']))
    os.link(source, root/'song.wav')
    audio = AudioMetadataHelper._open_audio(source)
    assert AudioMetadataHelper._tag_updates(audio, MetaMusic(), True) == {}
    config['music_rename_on_save'] = True
    row = operations.preview(config, {'operation':'edit','paths':['song.wav'],'metadata':{'title':None}})['items'][0]
    assert row['success'] and row['target'] == 'song.wav', row
    assert row['tag_updates'] == {'title':[]}
    assert '保留' in row['rename_note']
    operations.execute(config, {'item':row,'operation':'edit','execution_id':'aacd-e1'})
    assert AudioMetadataHelper.read_tags(root/'song.wav').title is None
    assert library.inspect(config,'song.wav')['composers'] == ['可信作曲']


def test_failed_credit_clear_preserves_seed_and_library(fixture, monkeypatch):
    config, source, root = fixture
    assert AudioMetadataHelper.write(source, MetaMusic(composers=['? ??']))
    os.link(source, root/'song.wav')
    row = operations.preview(config, {'operation':'edit','paths':['song.wav'],'metadata':{'composers':[]}})['items'][0]
    monkeypatch.setattr(AudioMetadataHelper, 'write', lambda *args, **kwargs: True)
    with pytest.raises(ValueError, match='未正确写入或清空'):
        operations.execute(config, {'item':row,'operation':'edit','execution_id':'aacd-e2'})
    assert source.samefile(root/'song.wav')
    assert library.inspect(config,'song.wav')['composers'] == ['? ??']


@pytest.mark.parametrize('container', ['wav','flac','m4a','wma','wv'])
def test_explicit_empty_name_fields_clear_without_derived_artist_fallback(fixture, container):
    config, original, root = fixture
    source = original.with_suffix('.' + container)
    if container != 'wav':
        subprocess.run(['ffmpeg','-v','error','-i',str(original),str(source)],check=True)
    assert AudioMetadataHelper.write(source, MetaMusic(title='标题',artists=['????'],album='专辑',album_artist='????',isrc='KRA340918953'))
    os.link(source, root/('song.'+container))
    fields = dict(title='',artists=[],album=None,album_artist=None,isrc='')
    row = operations.preview(config, {'operation':'edit','paths':['song.'+container],'metadata':fields})['items'][0]
    assert row['success'],row
    assert row['tag_updates'] == {key:[] for key in ['title','artist','album','albumartist','isrc']}
    operations.execute(config, {'item':row,'operation':'edit','execution_id':'aacd-e3'})
    after = AudioMetadataHelper.read_tags(root/row['target'])
    assert all(not getattr(after,key) for key in fields)
    assert AudioMetadataHelper.read_tags(source).artists == ['????']


def test_manual_lyrics_resume_after_commit_keeps_new_file_and_seed(fixture, monkeypatch):
    config, source, root = fixture
    os.link(source, root/'song.wav')
    seed_lyrics = source.with_suffix('.lrc')
    seed_lyrics.write_text('[00:01.00]歌词')
    os.link(seed_lyrics, root/'song.lrc')
    row = operations.preview(config, {'operation':'edit','paths':['song.wav'],'metadata':{'lyrics':'[00:02.00]歌词'}})['items'][0]
    body = {'item':row,'operation':'edit','execution_id':'aacd-4'}
    write = operations.write_json
    def crash(path, data):
        if data.get('state') == 'lyrics_done': raise RuntimeError('歌词提交后退出')
        write(path,data)
    monkeypatch.setattr(operations,'write_json',crash)
    with pytest.raises(RuntimeError): operations.execute(config,body)
    inode = (root/'song.lrc').stat().st_ino
    monkeypatch.setattr(operations,'write_json',write)
    assert operations.execute(config,body)['success']
    assert (root/'song.lrc').stat().st_ino == inode
    assert seed_lyrics.read_text() == '[00:01.00]歌词'


def test_manual_offset_updates_existing_lyricsfile_and_preserves_word_timings(fixture):
    import yaml
    config, source, root = fixture
    os.link(source, root/'song.wav')
    (root/'song.lrc').write_text('[00:01.00]歌词')
    seed_yaml = source.with_suffix('.lyricsfile.yaml')
    payload = {'version':'1.0','metadata':{'title':'歌曲','artist':'歌手'},'lines':[{'text':'歌词','start_ms':1000,'end_ms':2000,'words':[{'text':'歌词','start_ms':1100,'end_ms':1800}]}]}
    seed_yaml.write_text(yaml.safe_dump(payload,allow_unicode=True))
    os.link(seed_yaml,root/'song.lyricsfile.yaml')
    row=operations.preview(config,{'operation':'edit','paths':['song.wav'],'metadata':{'lyrics':'[00:03.50]歌词'}})['items'][0]
    assert row['success'],row
    assert operations.execute(config,{'item':row,'operation':'edit','execution_id':'aacd-5'})['success']
    actual=yaml.safe_load((root/'song.lyricsfile.yaml').read_text())
    assert actual['lines'][0]['start_ms']==3500
    assert actual['lines'][0]['words'][0]['start_ms']==3600
    assert yaml.safe_load(seed_yaml.read_text())==payload
    assert not seed_yaml.samefile(root/'song.lyricsfile.yaml')


def test_reorganize_is_not_supported(fixture):
    config, source, root = fixture
    os.link(source, root/'song.wav')
    with pytest.raises(ValueError, match='未知音乐操作'):
        operations.preview(config, {'operation':'reorganize', 'paths':['song.wav']})
    row = plan(config, 'song.wav')
    row['target'] = 'moved.wav'
    with pytest.raises(ValueError, match='同步重命名设置已变化'):
        operations.execute(config, {'item':row, 'operation':'edit', 'execution_id':'abcd-8'})
    config['music_rename_on_save'] = True
    with pytest.raises(ValueError, match='同步名称与预览标签不一致'):
        operations.execute(config, {'item':row, 'operation':'edit', 'execution_id':'abcd-8'})
    assert source.samefile(root/'song.wav')


def test_resume_after_interrupted_staging_retries_automatically(fixture, monkeypatch):
    config, source, root = fixture
    os.link(source, root/'song.wav')
    row = plan(config, 'song.wav')
    write = operations.write_json
    def crash(path, data):
        write(path, data)
        if data.get('state') == 'staging':
            raise SystemExit('模拟中断')
    monkeypatch.setattr(operations, 'write_json', crash)
    body = {'item':row, 'operation':'edit', 'execution_id':'abcd-9'}
    with pytest.raises(SystemExit): operations.execute(config, body)
    monkeypatch.setattr(operations, 'write_json', write)
    assert operations.execute(config, body)['success']
    assert AudioMetadataHelper.read(root/row['target']).title == '新曲名'
    assert AudioMetadataHelper.read(source).title == '原曲名'


def test_manual_album_does_not_assign_two_files_to_one_track(fixture, monkeypatch):
    config, source, root = fixture
    os.link(source, root/'first.wav'); os.link(source, root/'second.wav')
    from etk_music.models import MusicAlbumInfo, MusicInfo
    album = MusicAlbumInfo(title='专辑', media_source='musicbrainz', media_id='album', tracks=[MusicInfo(title='原曲名', artists=['歌手'], track_number=1, duration=1, media_source='musicbrainz', media_id='song')])
    monkeypatch.setattr(operations.catalog, 'detail', lambda *_:album)
    result = operations.preview(config, {'paths':['first.wav','second.wav'], 'selection':{'media_source':'musicbrainz','media_id':'album','music_type':'album'}})
    assert len(result['items'])==2
    assert all(not row['success'] and '唯一对位' in row['message'] for row in result['items'])
    assert source.samefile(root/'first.wav') and source.samefile(root/'second.wav')


def test_single_song_identity_cannot_scrape_multiple_files(fixture):
    config, source, root = fixture
    with pytest.raises(ValueError, match='单曲资料'):
        operations.preview(config, {'paths':['a.wav','b.wav'], 'selection':{'media_source':'musicbrainz','media_id':'song','music_type':'recording'}})


def test_cover_globs_cannot_leave_audio_directory(fixture):
    config, source, root = fixture
    os.link(source, root/'song.wav')
    (root.parent/'cover.jpg').write_bytes(b'private')
    config['music_cover_file_patterns']='../cover.jpg'
    assert library.image(config, 'song.wav') is None


def test_worker_scan_batch_records_failures_and_library_accepts_null_query(fixture, tmp_path):
    config, source, root = fixture
    os.link(source, root/'song.wav')
    from worker import dispatch
    request={'config':config, 'cache_dir':str(tmp_path/'worker'), 'action':'scan_batch', 'body':{'paths':['song.wav','missing.wav'], 'scan_id':'1'}}
    result=dispatch(request)
    assert result[0]['success'] and not result[1]['success']
    request.update(action='library', body={'query':None})
    assert dispatch(request)['total']==1


def test_lyricsfile_and_lrc_preserve_source_links_and_timestamp_tags(fixture):
    config, source, root=fixture
    from etk_music.amll_lyrics import parse_lyrics
    document='<tt xmlns="http://www.w3.org/ns/ttml"><body><div><p begin="1" end="3"><span begin="1" end="2">聲</span><span begin="2" end="3">音</span></p></div></body></tt>'
    lyrics=parse_lyrics(document,title='歌曲',artist='歌手',provider_id='a',match_score=95)
    assert lyrics and lyrics.lyricsfile
    original=source.parent/'source.lrc';original.write_text('原始歌词',encoding='utf-8')
    os.link(original,root/'song.lrc')
    operations.sidecar(root/'song.wav',lyrics,'overwrite',True,'abcd-10')
    assert original.read_text()=='原始歌词'
    assert (root/'song.lrc').read_text().strip()=='[00:01.00]声音'
    assert (root/'song.lyricsfile.yaml').read_text().strip()==lyrics.lyricsfile.strip()
    assert not list(root.glob('.*etk-lyrics*'))


def test_cancel_cleanup_preserves_audio_and_removes_only_own_staging(fixture,monkeypatch):
    config,source,root=fixture
    os.link(source,root/'song.wav'); row=plan(config,'song.wav')
    write=operations.write_json
    def crash(path,data):
        write(path,data)
        if data['state']=='prepared':raise SystemExit('准备完成后中断')
    monkeypatch.setattr(operations,'write_json',crash)
    body={'item':row,'operation':'edit','execution_id':'abcd-11'}
    with pytest.raises(SystemExit):operations.execute(config,body)
    assert list(root.glob('.*etk-music*'))
    unrelated=root/'.other.etk-music-abcd-12';unrelated.write_text('别的任务')
    operations.cleanup(config,{'task_id':'abcd'})
    assert source.samefile(root/'song.wav') and unrelated.exists()
    assert len(list(root.glob('.*etk-music*')))==1


def test_cue_image_does_not_become_first_recording(fixture,monkeypatch):
    config,source,root=fixture
    os.link(source,root/'song.wav')
    (root/'song.cue').write_text('PERFORMER "歌手"\nTITLE "整轨专辑"\nFILE "song.wav" WAVE\n  TRACK 01 AUDIO\n    TITLE "第一首"\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    TITLE "第二首"\n    INDEX 01 00:00:40\n',encoding='utf-8')
    from etk_music.models import MusicInfo
    info=MusicInfo(title='线上专辑',album='线上专辑',artists=['歌手'],music_type='album',media_source='musicbrainz',media_id='album')
    monkeypatch.setattr(operations,'recognize_group',lambda *_:({str((root/'song.wav').resolve()):info},{}))
    preview=operations.preview(config,{'paths':['song.wav']})
    row=preview['items'][0]
    assert row['success'],row
    assert row['music_layout']=='image_cue'
    assert row['metadata']['title']!='第一首'
    assert row['metadata']['media_id']!='album' or row['metadata']['music_type']=='album'
    config['music_scraping_policies']['tags']='overwrite'
    operations.execute(config,{'item':row,'operation':'rescrape','execution_id':'abcd-12'})
    assert AudioMetadataHelper.read_tags(source).title=='原曲名'
    assert AudioMetadataHelper.read_tags(root/'song.wav').title!='第一首'


def test_fingerprint_cache_survives_new_provider_instance(fixture):
    from etk_music.acoustid import AcoustIdModule
    from etk_music.observation import capture_music_recognition
    with capture_music_recognition() as observation:
        module=AcoustIdModule()
        module._set_cached(('song',123,456),'recording',[{'recording_id':'recording','score':1.0}])
        next_instance=AcoustIdModule()
        assert next_instance._get_cached(('song',123,456))==(True,'recording')
        assert observation.fingerprint_candidates[0]['recording_id']=='recording'
        assert next_instance._get_cached(('song',123,457))==(False,None)


def test_inflight_source_change_does_not_write_sidecar_to_new_media(fixture,monkeypatch):
    config,source,root=fixture
    os.link(source,root/'song.wav')
    row=plan(config,'song.wav')
    config['music_scraping_policies']={'tags':'skip','cover':'skip','lyrics':'overwrite'}
    from etk_music.models import MusicLyrics
    def changed(*_):
        (root/'song.wav').unlink(); (root/'song.wav').write_bytes(source.read_bytes())
        return MusicLyrics(provider='local',plain_lyrics='不应写入')
    monkeypatch.setattr(operations.catalog,'lyrics',changed)
    with pytest.raises(ValueError,match='读取资料时已变化'):
        operations.execute(config,{'item':row,'operation':'rescrape','execution_id':'abcd-13'})
    assert not (root/'song.txt').exists()


def test_ambiguous_recognition_is_not_cached_as_not_found(fixture):
    from etk_music.cache import MusicBrainzCache
    from etk_music.models import MusicInfo
    meta=MetaMusic(title='同名歌曲',artists=['歌手'])
    cache=MusicBrainzCache()
    info=MusicInfo(title='同名歌曲',raw_data={'recognition':{'status':'ambiguous','message':'请人工选择'}})
    cache.update(meta,info)
    assert cache.get(meta) is None


def test_cue_edit_invalidates_unchanged_audio_index(fixture):
    config,source,root=fixture
    os.link(source,root/'song.wav')
    cue=root/'song.cue'
    content='PERFORMER "歌手"\nTITLE "原来专辑"\nFILE "song.wav" WAVE\n TRACK 01 AUDIO\n TITLE "第一首"\n INDEX 01 00:00:00\n TRACK 02 AUDIO\n TITLE "第二首"\n INDEX 01 00:00:40\n'
    cue.write_text(content,encoding='utf-8')
    first=library.scan_one(config,'song.wav','1')
    cue.write_text(content.replace('原来专辑','新的专辑'),encoding='utf-8')
    second=library.scan_one(config,'song.wav','2')
    assert first['signature']['mtime']==second['signature']['mtime']
    assert first['title']=='原来专辑' and second['title']=='新的专辑'


def test_source_failure_is_exposed_in_search(fixture,monkeypatch):
    from etk_music.observation import report_music_recognition
    class Source:
        def search_music(self,*args,**kwargs):
            report_music_recognition('service_error','服务暂不可用')
            return []
    monkeypatch.setattr(operations.catalog,'provider',lambda _:Source())
    result=operations.catalog.search('歌曲',['musicbrainz'])
    assert result['items']==[] and result['errors']=={'musicbrainz':'服务暂不可用'}


def test_display_conversion_retains_original_names_identities_and_lyrics(fixture):
    from etk_music.models import MusicAlbumInfo, MusicInfo
    from etk_music.display import MusicDisplay
    album=MusicAlbumInfo(title='歲月',artists=['優里'],artist_ids=['artist'],media_source='musicbrainz',media_id='album',tracks=[MusicInfo(title='後來',artists=['優里'],album='歲月',lyrics='後來的歌詞',media_source='musicbrainz',media_id='song')])
    converted=MusicDisplay._simplify_recognized_music_info(album)
    assert converted.title=='岁月' and converted.tracks[0].title=='后来'
    assert converted.media_id=='album' and converted.tracks[0].media_id=='song'
    assert converted.tracks[0].lyrics=='後來的歌詞'
    assert album.title=='歲月' and album.tracks[0].title=='後來'
    assert '歲月' in converted.title_aliases


def test_non_audio_content_is_not_marked_ready_for_tag_edit(fixture):
    config, source, root=fixture
    (root/'broken.mp3').write_bytes(b'not audio')
    result=operations.preview(config,{'operation':'edit','paths':['broken.mp3'],'metadata':{'title':'标题'}})
    assert not result['items'][0]['success']


def test_manual_edit_simplifies_text_in_both_preview_and_write(fixture):
    config, source, root = fixture
    config['music_metadata_to_simplified'] = True
    os.link(source, root/'song.wav')
    original = source.read_bytes()
    metadata = {'title':'歲月與聲音', 'artists':['藝人'], 'album':'專輯', 'album_artist':'樂團',
                'composers':['作曲藝人'], 'conductors':['指揮藝人'], 'orchestras':['管弦樂團'],
                'performers':{'電吉他':['演奏藝人']}, 'comment':'備註', 'description':'專輯簡介'}
    expected = {'title':'岁月与声音', 'artists':['艺人'], 'album':'专辑', 'album_artist':'乐团',
                'composers':['作曲艺人'], 'conductors':['指挥艺人'], 'orchestras':['管弦乐团'],
                'performers':{'电吉他':['演奏艺人']}, 'comment':'备注', 'description':'专辑简介'}
    row = operations.preview(config, {'operation':'edit', 'paths':['song.wav'], 'metadata':metadata})['items'][0]
    assert row['success'], row
    assert {key:row['metadata'][key] for key in expected} == expected
    operations.execute(config, {'item':row, 'operation':'edit', 'execution_id':'abcd-14'})
    stored = AudioMetadataHelper.read_tags(root/row['target'])
    assert {key:getattr(stored, key) for key in expected} == expected
    assert source.read_bytes() == original
    assert not source.samefile(root/row['target'])


@pytest.mark.parametrize('policy', ['missingOnly', 'overwrite'])
def test_simplified_alias_write_respects_policy_and_protects_seed(fixture, monkeypatch, policy):
    from etk_music.models import MusicInfo
    config, source, root = fixture
    AudioMetadataHelper.write(source, MetaMusic(title='Girls’ Generation', artists=['Girls’ Generation'],
                                               album_artist='Girls’ Generation', album='Girls’ Generation'), overwrite=True)
    original = source.read_bytes()
    os.link(source, root / 'song.wav')
    config['music_metadata_to_simplified'] = True
    config['music_scraping_policies']['tags'] = policy
    monkeypatch.setattr(operations.catalog, 'recognize', lambda *_: (AudioMetadataHelper.read(source), MusicInfo(
        title='Girls’ Generation', artists=['Girls’ Generation'], album_artist='Girls’ Generation',
        artist_aliases=['少女時代'], media_source='musicbrainz', media_id='recording')))
    row = operations.preview(config, {'paths':['song.wav']})['items'][0]
    assert row['success'], row
    assert row['metadata']['artists'] == ['少女时代']
    assert row['metadata']['album_artist'] == '少女时代'
    operations.execute(config, {'item':row, 'operation':'rescrape', 'execution_id':'abcd-21'})
    stored = AudioMetadataHelper.read_tags(root / row['target'])
    expected = '少女时代' if policy == 'overwrite' else 'Girls’ Generation'
    assert stored.artists == [expected] and stored.album_artist == expected
    assert source.read_bytes() == original


def test_missing_only_lyrics_does_not_query_existing_sidecar(fixture, monkeypatch):
    config, source, root = fixture
    os.link(source, root/'song.wav')
    (root/'song.lrc').write_text('[00:00.00]已有歌词', encoding='utf-8')
    row = plan(config, 'song.wav', '原曲名')
    config['music_scraping_policies'] = {'tags':'skip', 'cover':'skip', 'lyrics':'missingOnly'}
    monkeypatch.setattr(operations.catalog, 'lyrics', lambda *_: pytest.fail('已有歌词不应请求资料源'))
    result = operations.execute(config, {'item':row, 'operation':'rescrape', 'execution_id':'abcd-15'})
    assert '保留现有歌词' in result['message']
    assert source.samefile(root/'song.wav')


def test_detail_serialization_honors_simplified_setting(fixture, tmp_path):
    from etk_music.models import MusicInfo
    configure({'music_metadata_to_simplified':False}, tmp_path/'cache')
    assert operations.catalog.serialize(MusicInfo(title='歲月'))['title'] == '歲月'


def test_scan_rejects_non_audio_instead_of_indexing_filename(fixture):
    config, source, root = fixture
    (root/'broken.mp3').write_bytes(b'not audio')
    with pytest.raises(Exception):
        library.scan_one(config, 'broken.mp3', 'scan')
    assert library.listing(config)['total'] == 0


@pytest.mark.parametrize('policy', ['skip', 'missingOnly', 'overwrite'])
def test_preview_shows_actual_tag_policy(fixture, monkeypatch, policy):
    config, source, root = fixture
    os.link(source, root/'song.wav')
    from etk_music.models import MusicInfo
    monkeypatch.setattr(operations.catalog, 'recognize', lambda *_: (AudioMetadataHelper.read(source), MusicInfo(title='在线曲名', artists=['歌手'], media_source='musicbrainz', media_id='a')))
    config['music_scraping_policies']['tags'] = policy
    row = operations.preview(config, {'paths':['song.wav']})['items'][0]
    assert row['success'], row
    assert row['before']['title'] == '原曲名'
    assert ('title' in row['tag_updates']) == (policy == 'overwrite')


def test_cue_changed_during_recognition_requires_new_preview(fixture, monkeypatch):
    config, source, root = fixture
    os.link(source, root/'song.wav')
    cue = root/'song.cue'
    cue.write_text('PERFORMER "歌手"\nTITLE "专辑"\nFILE "song.wav" WAVE\n TRACK 01 AUDIO\n TITLE "曲名"\n INDEX 01 00:00:00\n', encoding='utf-8')
    from etk_music.models import MusicInfo
    def recognize(*_):
        cue.write_text(cue.read_text().replace('曲名','新曲名'), encoding='utf-8')
        return {str((root/'song.wav').resolve()):MusicInfo(title='原曲名')}, {}
    monkeypatch.setattr(operations, 'recognize_group', recognize)
    row = operations.preview(config, {'paths':['song.wav']})['items'][0]
    assert not row['success'] and 'CUE 在识别期间已变化' in row['message']


def test_truncated_fingerprint_candidates_require_confirmation(fixture, monkeypatch):
    config, source, root = fixture
    from etk_music.models import MusicInfo
    from etk_music.observation import report_music_fingerprints
    catalog = operations.catalog
    class Fingerprint:
        def init_module(self): pass
        def identify_music_by_fingerprint(self, path):
            report_music_fingerprints([{'recording_id':'song', 'score':1.0, 'duration':1, 'truncated':True}])
    monkeypatch.setattr(catalog, 'AcoustIdModule', Fingerprint)
    monkeypatch.setattr(catalog, 'detail', lambda *_:MusicInfo(title='原曲名', artists=['歌手'], media_source='musicbrainz', media_id='song'))
    monkeypatch.setattr(catalog, '_fingerprint_info_matches_evidence', lambda *_a, **_k:True)
    _, info = catalog.recognize(source, ['musicbrainz'])
    assert info.raw_data['recognition']['status']=='ambiguous' and not info.media_id


def test_manual_credit_edits_are_not_overwritten_by_original_tags(fixture):
    config, source, root = fixture
    assert AudioMetadataHelper.write(source, MetaMusic(composers=['旧作曲'], conductors=['旧指挥'], orchestras=['旧乐团']))
    os.link(source, root/'song.wav')
    expected = {'composers':['新作曲'], 'conductors':['新指挥'], 'orchestras':['新乐团']}
    row = operations.preview(config, {'operation':'edit', 'paths':['song.wav'], 'metadata':expected})['items'][0]
    assert row['success'], row
    assert all(row['metadata'][key]==value for key,value in expected.items())
    operations.execute(config, {'item':row, 'operation':'edit', 'execution_id':'abcd-16'})
    result = AudioMetadataHelper.read_tags(root/'song.wav')
    assert all(getattr(result,key)==value for key,value in expected.items())
    assert AudioMetadataHelper.read_tags(source).composers == ['旧作曲']


def test_selected_disc_directories_share_album_alignment_without_expanding_scope(fixture, monkeypatch):
    config, source, root = fixture
    for disc in (1,2):
        folder = root/'专辑'/f'CD{disc}'; folder.mkdir(parents=True)
        os.link(source, folder/'01.wav')
    os.link(source, root/'专辑'/'CD2'/'未选.wav')
    paths = ['专辑/CD1/01.wav','专辑/CD2/01.wav']
    assert library.preview_groups(paths) == {'专辑':paths}
    from etk_music.models import MusicAlbumInfo, MusicInfo
    album = MusicAlbumInfo(title='专辑',media_source='musicbrainz',media_id='album',total_discs=2,tracks=[MusicInfo(title='原曲名',artists=['歌手'],total_discs=2,disc_number=disc,track_number=1,duration=1,media_source='musicbrainz',media_id=f'song-{disc}') for disc in (1,2)])
    monkeypatch.setattr(operations.catalog, 'detail', lambda *_:album)
    rows = operations.preview(config, {'paths':paths,'selections':{'专辑':{'media_source':'musicbrainz','media_id':'album','music_type':'album'}}})['items']
    assert len(rows)==2 and all(row['success'] for row in rows), rows
    assert len({row['music']['group_id'] for row in rows})==1
    assert {row['metadata']['disc_number'] for row in rows}=={1,2}
    assert source.samefile(root/'专辑'/'CD2'/'未选.wav')


def test_secondary_album_source_preserves_local_edition_fields(fixture, monkeypatch):
    from etk_music import album as albums
    from etk_music.models import MusicAlbumInfo, MusicInfo
    config, source, root = fixture
    files = [root/'01.wav', root/'02.wav']
    metas = [MetaMusic(title=f'曲目{i}',artists=['歌手'],album='专辑',disc_number=1,track_number=i,total_discs=2,release_year=2020,original_year=1990,year=2020) for i in (1,2)]
    album = MusicAlbumInfo(title='专辑',media_source='theaudiodb',media_id='a',tracks=[MusicInfo(title=f'曲目{i}',artists=['歌手'],disc_number=1,track_number=i,total_discs=1,release_year=1990,year=1990,media_source='theaudiodb',media_id=f'track{i}') for i in (1,2)])
    monkeypatch.setattr(albums, 'recognize_music_sources', lambda *_:(album, {'status':'matched'}))
    rows, _ = albums.recognize_group(files, metas, ['theaudiodb'])
    assert len(rows)==2
    assert all(info.total_discs==2 and info.release_year==2020 and info.original_year==1990 for info in rows.values())


@pytest.mark.parametrize('clear', [False, True])
def test_recognition_cache_cleanup_matches_mp_without_removing_cover_cache(fixture, tmp_path, clear):
    from etk_music.cache import Cache, MusicBrainzCache
    from etk_music.models import MusicInfo
    from worker import dispatch
    config, source, root = fixture
    cache_dir = tmp_path/'cache'
    configure(config, cache_dir)
    meta = MetaMusic(title='测试曲目',artists=['歌手'])
    cache = MusicBrainzCache()
    cache.update(meta, MusicInfo(title='测试曲目', media_source='musicbrainz', media_id='song'))
    Cache(region='musicbrainz:http:v2').set('response', {'value':{'title':'旧查询'}})
    Cache(region='covers').set('cover', {'data':'保留封面'})
    body = {'clear':True} if clear else {'key':cache.key(meta)}
    assert dispatch({'action':'cache','config':config,'body':body,'cache_dir':str(cache_dir)})['items']==[]
    assert Cache(region='musicbrainz:http:v2').get('response') is None
    assert Cache(region='covers').get('cover') == {'data':'保留封面'}


def test_douban_explore_uses_mp_tag_filters_and_sort(fixture, monkeypatch):
    captured = {}
    class Douban:
        def music_discover(self, source, **options):
            captured.update(options)
            return []
    monkeypatch.setattr(operations.catalog,'DoubanMusic',Douban)
    operations.catalog.explore({'source':'doubanmusic','tags':'爵士,美国','douban_sort':'S','page':2})
    assert captured['mode']=='tag' and captured['tags']=='爵士,美国' and captured['sort']=='S' and captured['page']==2


@pytest.mark.parametrize("kind", ["hardlink", "copy"])
def test_absent_lyrics_can_be_previewed_saved_and_replayed(fixture, kind):
    config, source, root = fixture
    folder = root / "Girls’ Generation" / "GIRLS’ GENERATION II ～Girls & Peace～ (2012)"
    folder.mkdir(parents=True)
    target = folder / "01 - FLOWER POWER.wav"
    if kind == "hardlink": os.link(source, target)
    else: shutil.copyfile(source, target)
    before = target.read_bytes()
    relative = str(target.relative_to(root))
    metadata = library.inspect(config, relative)
    assert not metadata['lyrics']
    metadata['lyrics'] = '[00:01.00]FLOWER POWER\n'
    row = operations.preview(config, {'operation':'edit','paths':[relative],'metadata':{'lyrics':metadata['lyrics'],'lyrics_revision':metadata['lyrics_revision'],'signature':metadata['signature']}})['items'][0]
    assert row['success'], row
    assert not target.with_suffix('.lrc').exists()
    body = {'item':row,'operation':'edit','execution_id':'cafe-81'}
    for _ in range(2):
        assert operations.execute(config, body)['success']
        assert library.inspect(config, relative)['lyrics'] == metadata['lyrics']
        assert source.read_bytes() == target.read_bytes() == before
    assert target.samefile(source) == (kind == "hardlink")


def test_artist_collection_groups_albums_discs_and_standalone_singles():
    paths = ['Artist/Album/CD1/01.flac','Artist/Album/CD2/01.flac','Artist/Singles/Sub/one.flac','Artist/Singles/two.flac','Artist/loose.flac']
    groups = library.preview_groups(paths, 'artist_collection', 'Artist')
    assert groups['Artist/Album'] == paths[:2]
    assert all(groups[path] == [path] for path in paths[2:])
    assert all(group == [path] for path, group in library.preview_groups(paths, 'recording').items())
    with pytest.raises(ValueError): library.preview_groups(paths, 'artist_collection', None)
    with pytest.raises(ValueError): library.preview_groups(['Other/song.flac'], 'artist_collection', 'Artist')


@pytest.mark.parametrize('mode', ['strict', 'ignore_duration', 'sequential'])
def test_rescrape_passes_matching_mode_to_existing_album_matcher(fixture, monkeypatch, mode):
    config, source, root = fixture
    os.link(source, root/'song.wav')
    from etk_music.models import MusicInfo
    def recognize(files, metas, sources, choice, directory, track_match):
        assert track_match == mode
        return {str(files[0].resolve()): MusicInfo(title='匹配曲名')}, {}
    monkeypatch.setattr(operations, 'recognize_group', recognize)
    row = operations.preview(config, {'paths':['song.wav'],'music_track_match':mode})['items'][0]
    assert row['success'] and row['metadata']['title'] == '匹配曲名'


def test_single_preview_can_be_corrected_to_a_selected_album(fixture, monkeypatch):
    config, source, root = fixture
    os.link(source, root/'song.wav')
    from etk_music.models import MusicInfo
    def recognize(files, metas, sources, choice, directory, track_match):
        assert choice['media_id'] == 'selected-album'
        assert directory == root
        return {str(files[0].resolve()): MusicInfo(title='纠正曲名')}, {}
    monkeypatch.setattr(operations, 'recognize_group', recognize)
    preview = operations.preview(config, {'paths':['song.wav'],'music_type':'recording',
        'selections':{'song.wav':{'music_type':'album','media_id':'selected-album'}}})
    assert preview['items'][0]['success'], preview
    assert preview['items'][0]['metadata']['title'] == '纠正曲名'
    assert preview['items'][0]['music']['group_directory'] == 'song.wav'


def test_editor_album_alignment_reaches_read_preview_and_seed_safe_save(fixture, tmp_path, monkeypatch):
    """时长冲突仅可显式忽略，预览重新核验；写入仍隔离做种硬链接。"""
    from worker import dispatch
    from etk_music.models import MusicAlbumInfo, MusicInfo
    config, source, root = fixture
    os.link(source, root/'song.wav')
    before = source.read_bytes()
    album = MusicAlbumInfo(title='专辑', media_source='musicbrainz', media_id='album', tracks=[
        MusicInfo(title='所选曲名', artists=['歌手'], track_number=1, duration=200, media_source='musicbrainz', media_id='11111111-1111-1111-1111-111111111111')])
    monkeypatch.setattr(operations.catalog, 'detail', lambda *_: album)
    selection = {'media_source':'musicbrainz','media_id':'album','music_type':'album'}
    body = {'path':'song.wav','paths':['song.wav'],'operation':'edit','selection':selection,'music_type':'album','music_cue_enable':False}
    def run(action):
        return dispatch({'action':action,'config':config,'body':body,'cache_dir':str(tmp_path/'cache')})
    with pytest.raises(ValueError, match='唯一对位'):
        run('edit_metadata')
    body['music_track_match'] = 'ignore_duration'
    identified = run('edit_metadata')
    assert identified['title'] == '所选曲名'
    body['metadata'] = {**library.inspect(config,'song.wav'), **{key:identified[key] for key in ('title','artists','media_source','media_id')}}
    body['metadata']['title'] = '用户核对后手改'
    preview = run('preview')['items'][0]
    assert preview['success'] and preview['music_cue_enable'] is False, preview
    assert preview['metadata']['title'] == '用户核对后手改'
    assert source.samefile(root/'song.wav') and source.read_bytes() == before
    body['music_track_match'] = 'strict'
    assert run('preview')['items'][0]['success'] is False
    result = dispatch({'action':'execute','config':config,'body':{'item':preview,'operation':'edit','execution_id':'cafe-90'},'cache_dir':str(tmp_path/'cache')})
    assert result['success']
    assert AudioMetadataHelper.read(root/result['target']).title == '用户核对后手改'
    assert not source.samefile(root/result['target']) and source.read_bytes() == before


def test_editor_cue_override_applies_to_read_and_preview_without_changing_setting(fixture, tmp_path, monkeypatch):
    from worker import dispatch
    from etk_music.audio import music_cue_enabled
    from etk_music.models import MusicInfo
    config, source, root = fixture
    os.link(source, root/'song.wav')
    (root/'song.cue').write_text('FILE "song.wav" WAVE\n TRACK 01 AUDIO\n TITLE "损坏 CUE"\n',encoding='utf8')
    monkeypatch.setattr(operations.catalog,'detail',lambda *_:MusicInfo(title='读取的曲名'))
    body = {'path':'song.wav','paths':['song.wav'],'operation':'edit','metadata':{'title':'手改曲名'},'selection':{'media_source':'musicbrainz','media_id':'song','music_type':'recording'}}
    def run(action):
        return dispatch({'action':action,'config':config,'body':body,'cache_dir':str(tmp_path/'cache')})
    with pytest.raises(ValueError,match='CUE'):
        run('edit_metadata')
    assert run('preview')['items'][0]['success'] is False
    body['music_cue_enable'] = False
    assert run('edit_metadata')['title'] == '读取的曲名'
    assert run('preview')['items'][0]['success']
    assert music_cue_enabled() is True


def test_sequential_album_uses_natural_paths_and_requires_equal_count(fixture, tmp_path, monkeypatch):
    from worker import dispatch
    from etk_music.models import MusicAlbumInfo, MusicInfo
    config, source, root = fixture
    for name in ['10.wav','2.wav']: os.link(source,root/name)
    album = MusicAlbumInfo(title='专辑', media_source='musicbrainz', media_id='album', tracks=[
        MusicInfo(title='第一曲',track_number=1,media_source='musicbrainz',media_id='one'),
        MusicInfo(title='第二曲',track_number=2,media_source='musicbrainz',media_id='two')])
    monkeypatch.setattr(operations.catalog,'detail',lambda *_:album)
    selection={'media_source':'musicbrainz','media_id':'album','music_type':'album'}
    rows=operations.preview(config,{'paths':['10.wav','2.wav'],'selection':selection,'music_track_match':'sequential'})['items']
    assert all(row['success'] for row in rows), rows
    assert {row['path']:row['metadata']['title'] for row in rows} == {'2.wav':'第一曲','10.wav':'第二曲'}
    with pytest.raises(ValueError,match='唯一对位'):
        dispatch({'action':'edit_metadata','config':config,'cache_dir':str(tmp_path/'cache'),'body':{'path':'2.wav','selection':selection,'music_track_match':'sequential'}})
