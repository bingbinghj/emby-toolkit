"""真实音频：标签同步命名、做种保护、冲突及中断后的幂等恢复。"""
import os
from pathlib import Path
import pytest
from etk_music import library, naming, operations
from etk_music.audio import AudioMetadataHelper
from etk_music.meta import MetaMusic
from test_library import fixture


def preview(config, relative="旧歌手/旧专辑/01.wav", **metadata):
    row = operations.preview(config, {"operation":"edit", "paths":[relative], "metadata":metadata or {"title":"Oh!", "album":"Oh! (2010)", "album_artist":"少女时代", "year":2010, "release_year":2010}})["items"][0]
    assert row["success"], row
    return row


def setup_song(fixture):
    config, seed, root = fixture
    config["music_rename_on_save"] = True
    source = root/"旧歌手/旧专辑/01.wav"
    source.parent.mkdir(parents=True)
    os.link(seed, source)
    lyrics_seed = seed.with_suffix(".lrc")
    lyrics_seed.write_text("[00:00.00]原来的歌词\n", encoding="utf-8")
    os.link(lyrics_seed, source.with_suffix(".lrc"))
    library.scan_one(config, str(source.relative_to(root)), "initial")
    return config, seed, root, source, lyrics_seed


@pytest.mark.parametrize("enabled", [None, False])
def test_rename_defaults_off_and_preserves_path_when_tags_change(fixture, enabled):
    config, seed, root, source, lyrics_seed = setup_song(fixture)
    config.pop("music_rename_on_save")
    if enabled is not None:
        config["music_rename_on_save"] = enabled
    original = seed.read_bytes()
    row = preview(config)
    assert row["target"] == row["path"]
    operations.execute(config, {"operation":"edit", "item":row, "execution_id":"eeaa-1"})
    assert AudioMetadataHelper.read_tags(source).title == "Oh!"
    assert source.with_suffix(".lrc").read_bytes() == lyrics_seed.read_bytes()
    assert seed.read_bytes() == original


def test_enabled_preview_renames_previously_saved_tags_without_another_tag_change(fixture):
    config, seed, root, source, lyrics_seed = setup_song(fixture)
    original = seed.read_bytes()
    config["music_rename_on_save"] = False
    row = preview(config)
    operations.execute(config, {"operation":"edit", "item":row, "execution_id":"eeaa-2"})
    assert source.exists()
    config["music_rename_on_save"] = True
    saved_audio = source.read_bytes()
    row = preview(config, str(source.relative_to(root)), title="Oh!")
    assert not row["tag_updates"]
    assert row["target"] == "少女时代/Oh! (2010)/01 - Oh!.wav"
    operations.execute(config, {"operation":"edit", "item":row, "execution_id":"eeaa-3"})
    destination = root/row["target"]
    assert destination.read_bytes() == saved_audio
    assert destination.with_suffix(".lrc").read_bytes() == lyrics_seed.read_bytes()
    assert not source.exists() and seed.read_bytes() == original
    again = preview(config, row["target"], title="Oh!")
    assert again["target"] == row["target"] and "rename_lyrics" not in again


def test_saved_chinese_artist_tags_fix_existing_flac_directory(fixture):
    import shutil
    config, seed, root = fixture
    config["music_rename_on_save"] = True
    original = seed.with_suffix(".flac")
    shutil.copyfile(Path(__file__).parent/"fixtures/audio/silence.flac", original)
    metadata = {"title":"Girls’ Generation", "artists":["少女时代"], "album_artist":"少女时代", "album":"Girls’ Generation", "release_year":2007, "track_number":1}
    assert AudioMetadataHelper.write(original, MetaMusic(**metadata))
    path = root/"Girls’ Generation/Girls’ Generation (2007)/01 - Girls’ Generation.flac"
    path.parent.mkdir(parents=True)
    os.link(original, path)
    path.with_suffix(".lrc").write_text("[00:01]歌词\n")
    audio = original.read_bytes()
    row = preview(config, str(path.relative_to(root)), **metadata)
    assert not row["tag_updates"]
    assert row["target"] == "少女时代/Girls’ Generation (2007)/01 - Girls’ Generation.flac"
    operations.execute(config, {"operation":"edit", "item":row, "execution_id":"eeaa-5"})
    assert (root/row["target"]).read_bytes() == audio == original.read_bytes()
    assert (root/row["target"]).with_suffix(".lrc").read_text() == "[00:01]歌词\n"


def test_relative_seed_symlink_is_isolated_before_rename_without_tag_changes(fixture):
    config, seed, root = fixture
    config["music_rename_on_save"] = True
    path = root/"song.wav"
    path.symlink_to(os.path.relpath(seed, root))
    original = seed.read_bytes()
    row = preview(config, path.name, title="原曲名")
    assert not row["tag_updates"] and row["target"] != row["path"]
    operations.execute(config, {"operation":"edit", "item":row, "execution_id":"eeaa-6"})
    target = root/row["target"]
    assert target.is_file() and not target.is_symlink()
    assert seed.read_bytes() == original
    assert AudioMetadataHelper.read_tags(target).title == "原曲名"


def test_disabling_rename_after_preview_does_not_write(fixture):
    config, seed, root, source, _ = setup_song(fixture)
    row = preview(config)
    config["music_rename_on_save"] = False
    with pytest.raises(ValueError, match="设置已变化"):
        operations.execute(config, {"operation":"edit", "item":row, "execution_id":"eeaa-7"})
    assert source.samefile(seed) and not (root/row["target"]).exists()


def test_mp_template_year_artist_track_disc_and_invalid_characters():
    assert naming.render({"title":"A/B?", "artists":["A", "B"], "album":"专辑 (2010)", "year":2010, "track_number":9, "total_discs":2}, ".flac") == "A ／ B/专辑 (2010)/Disc 1/09 - A／B？.flac"
    assert naming.render({"title":"单曲"}, ".MP3") == "Unknown Artist/Unknown Album/单曲.MP3"
    assert naming.render({"title":"曲目", "album_artist":"专辑艺人", "artists":["艺人"], "album":"专辑 (2010)"}, ".m4a") == "专辑艺人/专辑 (2010)/曲目.m4a"


def test_tag_and_lyric_rename_protect_seed_scope_and_index(fixture):
    config, seed, root, source, lyrics_seed = setup_song(fixture)
    original, old_lyrics = seed.read_bytes(), lyrics_seed.read_bytes()
    untouched = source.with_name("02.wav")
    os.link(seed, untouched)
    row = preview(config)
    assert row["target"] == "少女时代/Oh! (2010)/01 - Oh!.wav"
    assert source.samefile(seed) and not (root/row["target"]).exists()
    body = {"operation":"edit", "item":row, "execution_id":"aabb-1"}
    result = operations.execute(config, body)
    destination = root/result["target"]
    assert destination.exists() and not source.exists()
    assert destination.with_suffix(".lrc").read_bytes() == old_lyrics
    assert not source.with_suffix(".lrc").exists()
    assert seed.read_bytes() == original and lyrics_seed.read_bytes() == old_lyrics
    assert untouched.samefile(seed) and untouched.parent.exists()
    assert not destination.samefile(seed)
    assert [item["path"] for item in library.listing(config)["items"]] == [row["target"]]
    inode = destination.stat().st_ino
    assert operations.execute(config, body) == result
    assert destination.stat().st_ino == inode
    second = preview(config, row["target"], title="再次修改")
    operations.execute(config, {"operation":"edit", "item":second, "execution_id":"aabb-2"})
    assert (root/second["target"]).with_suffix(".lrc").exists()
    assert seed.read_bytes() == original


def test_rename_and_new_lyrics_are_one_confirmed_operation(fixture):
    config, seed, root, source, lyrics_seed = setup_song(fixture)
    row = preview(config, title="新标题", lyrics="[00:00.50]新歌词")
    result = operations.execute(config, {"operation":"edit", "item":row, "execution_id":"aabb-3"})
    assert (root/result["target"]).with_suffix(".lrc").read_text() == "[00:00.50]新歌词\n"
    assert lyrics_seed.read_text() == "[00:00.00]原来的歌词\n"
    assert not source.parent.exists()
    assert not source.parent.parent.exists()


@pytest.mark.parametrize("extension", [".wav", ".lrc", ".txt", ".lyricsfile.yaml"])
def test_preview_and_save_refuse_existing_destinations(fixture, extension):
    config, seed, root, source, _ = setup_song(fixture)
    row = preview(config)
    destination = (root/row["target"]).with_suffix(extension)
    destination.parent.mkdir(parents=True)
    destination.write_bytes(b"keep other file")
    failed = operations.preview(config, {"operation":"edit", "paths":[row["path"]], "metadata":{"title":"Oh!", "album":"Oh! (2010)", "album_artist":"少女时代", "release_year":2010}})["items"][0]
    assert not failed["success"] and "目标已存在" in failed["message"]
    with pytest.raises(ValueError, match="目标已存在"):
        operations.execute(config, {"operation":"edit", "item":row, "execution_id":"aabb-4"})
    assert destination.read_bytes() == b"keep other file" and source.samefile(seed)


@pytest.mark.parametrize("stage", ["prepared", "audio_done", "lyrics_done", "moving", "linked", "completed"])
def test_restart_finishes_rename_without_losing_audio_or_lyrics(fixture, monkeypatch, stage):
    config, seed, root, source, lyrics_seed = setup_song(fixture)
    original = seed.read_bytes()
    row = preview(config)
    write = operations.write_json
    def interrupt(path, data):
        if data.get("state") == stage:
            raise SystemExit("模拟提交回执前退出")
        write(path, data)
    monkeypatch.setattr(operations, "write_json", interrupt)
    body = {"operation":"edit", "item":row, "execution_id":"aabb-5"}
    with pytest.raises(SystemExit): operations.execute(config, body)
    monkeypatch.setattr(operations, "write_json", write)
    result = operations.execute(config, body)
    destination = root/result["target"]
    assert AudioMetadataHelper.read_tags(destination).title == "Oh!"
    assert destination.with_suffix(".lrc").read_bytes() == lyrics_seed.read_bytes()
    assert not source.exists() and not source.with_suffix(".lrc").exists()
    assert seed.read_bytes() == original


def test_mid_move_collision_rolls_back_only_owned_new_links(fixture, monkeypatch):
    config, seed, root, source, _ = setup_song(fixture)
    row = preview(config)
    destination = root/row["target"]
    link = os.link
    def collide(old, new, **kwargs):
        if str(new).endswith(".lrc"):
            Path(new).write_text("外部新文件")
        return link(old, new, **kwargs)
    monkeypatch.setattr(os, "link", collide)
    with pytest.raises(FileExistsError):
        operations.execute(config, {"operation":"edit", "item":row, "execution_id":"aabb-6"})
    operations.cleanup(config, {"task_id":"aabb"})
    assert source.is_file() and source.with_suffix(".lrc").is_file()
    assert not destination.exists()
    assert destination.with_suffix(".lrc").read_text() == "外部新文件"
    assert AudioMetadataHelper.read_tags(seed).title == "原曲名"


def test_preview_detects_duplicates_and_existing_lyrics_changes(fixture):
    config, seed, root, source, _ = setup_song(fixture)
    other = source.with_name("02.wav")
    os.link(seed, other)
    rows = operations.preview(config, {"operation":"edit", "paths":[str(source.relative_to(root)), str(other.relative_to(root))], "metadata":{"title":"重名"}})["items"]
    assert all(not row["success"] and "名称相同" in row["message"] for row in rows)
    row = preview(config)
    source.with_suffix(".txt").write_text("另一份歌词")
    with pytest.raises(ValueError, match="歌词在预览后已变化"):
        operations.execute(config, {"operation":"edit", "item":row, "execution_id":"aabb-7"})
    assert source.samefile(seed)


def test_destination_symlink_cannot_reach_seed(fixture):
    config, seed, root, source, _ = setup_song(fixture)
    (root/"少女时代").symlink_to(seed.parent, target_is_directory=True)
    row = operations.preview(config, {"operation":"edit", "paths":[str(source.relative_to(root))], "metadata":{"album_artist":"少女时代"}})["items"][0]
    assert not row["success"] and "符号链接" in row["message"]
    assert source.samefile(seed)


def test_lyrics_only_changes_keep_existing_path(fixture):
    config, _, _, source, _ = setup_song(fixture)
    config["music_rename_on_save"] = False
    row = preview(config, lyrics="[00:00.50]只改歌词")
    assert row["target"] == row["path"] and "rename_lyrics" not in row


def test_unused_template_fields_keep_an_already_synchronized_path(fixture):
    config, seed, root, source, _ = setup_song(fixture)
    row = preview(config)
    operations.execute(config, {"operation":"edit", "item":row, "execution_id":"eeaa-4"})
    assert preview(config, row["target"], total_tracks=20)["target"] == row["target"]
    assert preview(config, row["target"], artists=["新的客串艺人"])["target"] == row["target"]


@pytest.mark.parametrize("shared", [False, True])
def test_album_cover_remains_available_after_folder_change(fixture, shared):
    config, seed, root, source, _ = setup_song(fixture)
    image = seed.parent/"cover.jpg"
    image.write_bytes(b"original image")
    old_cover = source.parent/"cover.jpg"
    os.link(image, old_cover)
    if shared:
        os.link(seed, source.with_name("02.wav"))
    row = preview(config)
    operations.execute(config, {"operation":"edit", "item":row, "execution_id":"aabb-8"})
    assert (root/row["target"]).parent.joinpath("cover.jpg").read_bytes() == image.read_bytes() == b"original image"
    assert old_cover.exists() == shared


@pytest.mark.parametrize("step", ["link", "unlink"])
def test_restart_halfway_through_file_moves(fixture, monkeypatch, step):
    config, seed, root, source, lyrics_seed = setup_song(fixture)
    row = preview(config)
    destination = root/row["target"]
    original = getattr(os, step)
    def interrupt(*args, **kwargs):
        result = original(*args, **kwargs)
        if (step == "link" and Path(args[1]) == destination) or (step == "unlink" and Path(args[0]) == source):
            raise SystemExit("模拟文件移动中断")
        return result
    monkeypatch.setattr(os, step, interrupt)
    body = {"operation":"edit", "item":row, "execution_id":"aabb-9"}
    with pytest.raises(SystemExit): operations.execute(config, body)
    monkeypatch.setattr(os, step, original)
    assert operations.execute(config, body)["success"]
    assert not source.exists()
    assert destination.with_suffix(".lrc").read_bytes() == lyrics_seed.read_bytes()
    assert AudioMetadataHelper.read_tags(seed).title == "原曲名"


@pytest.mark.parametrize("extension", ["flac", "mp3", "m4a", "wma"])
def test_other_audio_containers_keep_extension_stream_and_seed(fixture, extension):
    import shutil
    import subprocess
    config, seed, root = fixture
    config["music_rename_on_save"] = True
    original = seed.with_suffix("."+extension)
    shutil.copyfile(Path(__file__).parent/"fixtures"/"audio"/("silence."+extension), original)
    assert AudioMetadataHelper.write(original, MetaMusic(title="原曲名", artists=["歌手"], album="专辑", track_number=1))
    path = root/("01."+extension)
    os.link(original, path)
    old = original.read_bytes()
    row = preview(config, path.name, title="新曲名")
    result = operations.execute(config, {"operation":"edit", "item":row, "execution_id":"ccdd-1"})
    destination = root/result["target"]
    assert destination.suffix == "."+extension
    assert original.read_bytes() == old and not destination.samefile(original)
    assert AudioMetadataHelper.read_tags(destination).title == "新曲名"
    if shutil.which("ffmpeg"):
        def audio_hash(path):
            return subprocess.check_output(["ffmpeg","-v","error","-i",str(path),"-map","0:a:0","-c:a","copy","-f","hash","-"])
        assert audio_hash(original) == audio_hash(destination)
