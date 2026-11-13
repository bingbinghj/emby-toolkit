use super::*;
use serde_json::json;

fn source(id: &str, owner: Option<&str>, path: &str) -> Value {
    json!({"Id":id,"ItemId":owner,"Path":path,"MediaStreams":[{"Type":"Audio","Title":id}]})
}

fn item(sources: Vec<Value>) -> Value {
    json!({"Id":"102","MediaSources":sources})
}

#[test]
fn preserve_linked_version_uses_native_streams_and_keeps_container_fields() {
    let original = json!({"MediaSourceInfo":{"Size":100,"Container":"mkv","MediaStreams":[
        {"Type":"Audio","Index":1,"Title":"旧标题","Language":"jpn","IsDefault":true}
    ]},"Chapters":[]});
    let streams = json!([
        {"Type":"Video","Index":0,"Codec":"hevc"},
        {"Type":"Audio","Index":1,"Codec":"aac","Title":"用户国语","Language":"chi","IsDefault":false},
        {"Type":"Subtitle","Index":2,"Title":"用户字幕","Language":"yue","IsForced":true,"IsExternal":true,"Path":"/media/movie.zh.srt"}
    ]);
    let live = json!({"Id":"102","MediaSources":[
        {"ItemId":"101","Path":"/media/movie.mkv","MediaStreams":[{"Type":"Audio","Title":"其他版本"}]},
        {"ItemId":"102","Path":"/media/movie.mkv","MediaStreams":streams}
    ]});
    let mut protected = original.clone();
    preserve_live_streams(&mut protected, &live, &["/media/movie.mkv"]).unwrap();
    assert_eq!(protected["MediaSourceInfo"]["MediaStreams"], streams);
    assert_eq!(
        protected["MediaSourceInfo"]["Size"],
        original["MediaSourceInfo"]["Size"]
    );
    assert_eq!(
        protected["MediaSourceInfo"]["Container"],
        original["MediaSourceInfo"]["Container"]
    );
}

#[test]
fn preserve_linked_version_keeps_recoverable_backup_when_native_streams_are_empty() {
    let original =
        json!({"MediaSourceInfo":{"MediaStreams":[{"Type":"Audio","Title":"恢复备份"}]}});
    for live in [
        json!({"Id":"102","MediaSources":[],"MediaStreams":[]}),
        json!({"Id":"102","MediaSources":[{"ItemId":"102","Path":"/media/movie.mkv","MediaStreams":[]}]}),
    ] {
        let mut protected = original.clone();
        preserve_live_streams(&mut protected, &live, &["/media/movie.mkv"]).unwrap();
        assert_eq!(protected, original);
    }
}

#[test]
fn preserve_linked_version_rejects_ambiguous_native_sources_without_changing_backup() {
    let live = item(vec![
        source("one", Some("102"), "/media/movie.mkv"),
        source("two", Some("102"), "/media/movie.mkv"),
    ]);
    let original =
        json!({"MediaSourceInfo":{"MediaStreams":[{"Type":"Audio","Title":"恢复备份"}]}});
    let mut protected = original.clone();
    assert!(preserve_live_streams(&mut protected, &live, &["/media/movie.mkv"]).is_err());
    assert_eq!(protected, original);
}

#[test]
fn recovery_chapters_refuses_empty_live_chapters_when_formal_backup_is_nonempty() {
    let backup = json!({"MediaSourceInfo":{"Chapters":[]},"Chapters":[
        {"Name":"普通章节","StartPositionTicks":0,"MarkerType":"Chapter"},
        {"Name":"片头","StartPositionTicks":1889303482_u64,"MarkerType":"IntroStart"},
        {"Name":"片尾","StartPositionTicks":2911691541_u64,"MarkerType":"IntroEnd"}
    ]});
    let before = backup.clone();
    let live = json!({"Id":"102","Type":"Episode","MediaStreams":[
        {"Type":"Video","Index":0},{"Type":"Audio","Index":1}
    ],"Chapters":[]});
    let error = recovery_chapters(&backup, &live, &["/media/movie.mkv"]).unwrap_err();
    assert_eq!(error.0, axum::http::StatusCode::CONFLICT);
    assert!(error.1.contains("备份仍有章节"), "{}", error.1);
    assert_eq!(backup, before);
}

#[test]
fn recovery_chapters_keeps_backup_after_empty_native_media_info() {
    let backup = json!({"Chapters":[
        {"Name":"原章节","StartPositionTicks":123456789_u64,"MarkerType":"IntroStart"}
    ]});
    let live = json!({"Id":"102","MediaStreams":[],"Chapters":[]});
    assert_eq!(
        recovery_chapters(&backup, &live, &["/media/movie.mkv"]).unwrap(),
        backup["Chapters"]
    );
}

#[test]
fn recovery_chapters_allows_empty_live_chapters_with_an_empty_backup() {
    let backup = json!({"Chapters":[]});
    for live in [
        json!({"Id":"102","Type":"Episode","MediaStreams":[{"Type":"Video"}],"Chapters":[]}),
        json!({"Id":"102","Type":"Audio","MediaStreams":[{"Type":"Audio"}]}),
    ] {
        assert_eq!(
            recovery_chapters(&backup, &live, &["/media/movie.mkv"]).unwrap(),
            json!([])
        );
    }
}

#[test]
fn recovery_chapters_keeps_nonempty_native_chapters_authoritative() {
    let backup = json!({"Chapters":[{"Name":"旧章节","StartPositionTicks":0}]});
    let live = json!({"Id":"102","MediaStreams":[{"Type":"Video"}],"Chapters":[
        {"Name":"当前章节","StartPositionTicks":2911691541_u64,"MarkerType":"IntroEnd","ImageTag":"native"}
    ]});
    assert_eq!(
        recovery_chapters(&backup, &live, &["/media/movie.mkv"]).unwrap(),
        clean_chapters(&live["Chapters"]).unwrap()
    );
}

#[test]
fn matching_sources_with_the_same_explicit_owner_are_ambiguous() {
    let item = item(vec![
        source("source-a", Some("102"), "/media/movie.mkv"),
        source("source-b", Some("102"), "/media/movie.mkv"),
    ]);
    assert!(item_streams(&item, &["/media/movie.mkv"]).is_none());
}

#[test]
fn normalized_aliases_with_the_same_explicit_owner_are_ambiguous() {
    let item = item(vec![
        source("source-a", Some("102"), "/media/movie.mkv"),
        source("source-b", Some("102"), "/media/%6dovie.mkv"),
    ]);
    assert!(item_streams(&item, &["file:///media/movie.mkv"]).is_none());
}

#[test]
fn exact_paths_keep_priority_over_normalized_aliases_of_the_same_owner() {
    let item = item(vec![
        source("source-a", Some("102"), "file:///media/movie.mkv"),
        source("source-b", Some("102"), "/media/movie.mkv"),
    ]);
    assert_eq!(
        item_streams(&item, &["/media/movie.mkv"]).unwrap()[0]["Title"],
        "source-b"
    );
}

#[test]
fn unique_native_and_legacy_sources_keep_existing_selection_rules() {
    for (sources, target, selected) in [
        (
            vec![
                source("opaque", None, "/media/movie.mkv"),
                source("native", Some("102"), "/media/movie.mkv"),
            ],
            "/media/movie.mkv",
            "native",
        ),
        (
            vec![
                source("other", Some("101"), "/media/movie.mkv"),
                source("native", Some("102"), "/media/movie.mkv"),
            ],
            "/media/movie.mkv",
            "native",
        ),
        (
            vec![
                source("mediasource_101", None, "/media/movie.mkv"),
                source("mediasource_102", None, "/media/movie.mkv"),
            ],
            "/media/movie.mkv",
            "mediasource_102",
        ),
        (
            vec![source("native", Some("102"), "/media/%6dovie.mkv")],
            "file:///media/movie.mkv",
            "native",
        ),
    ] {
        assert_eq!(
            item_streams(&item(sources), &[target]).unwrap()[0]["Title"],
            selected
        );
    }
}

fn logical_track_info(streams: Vec<Value>) -> Value {
    json!({"MediaSourceInfo":{
        "Path":"/target/episode02.mkv","Container":"mkv",
        "Size":23456,"RunTimeTicks":67890,"MediaStreams":streams
    },"Chapters":[]})
}

fn logical_track_patch(
    kind: &str,
    ordinal: u64,
    codec: &str,
    channels: Value,
    language: &str,
    title: &str,
    changes: Value,
) -> Value {
    json!([{"identity":{"type":kind,"ordinal":ordinal,"index":17,
        "codec":codec,"channels":channels,"language":language,"title":title,
        "is_external":if kind=="Subtitle" {json!(false)} else {Value::Null}},
        "changes":changes}])
}

#[test]
fn logical_track_generic_chinese_title_edit_keeps_target_original_code() {
    for language in ["zho", "chi"] {
        let mut streams = (0..6)
            .map(|index| {
                json!({"Type":"Subtitle","Index":index,"Codec":"srt",
                "Language":"eng","Title":format!("Other {index}"),"IsExternal":false})
            })
            .collect::<Vec<_>>();
        streams.push(json!({"Type":"Subtitle","Index":26,"Codec":"ass",
            "Language":language,"Title":"广配","IsExternal":false}));
        let original = logical_track_info(streams);
        let patches = logical_track_patch(
            "Subtitle",
            7,
            "ass",
            Value::Null,
            "chs",
            "广配",
            json!({"Title":"广配字幕"}),
        );
        let (mut output, report) = apply_patches(&original, &patches);
        assert_eq!(report["matched"], 1, "{language}: {report}");
        crate::language::normalize_subtitles_for_save(&mut output, &original);
        let mut expected = original.clone();
        expected["MediaSourceInfo"]["MediaStreams"][6]["Title"] = json!("广配字幕");
        assert_eq!(
            output, expected,
            "target code and other tracks must remain intact"
        );
    }
}

#[test]
fn logical_track_unique_role_can_follow_same_type_reordering() {
    let original = logical_track_info(vec![
        json!({"Type":"Video","Index":0,"Codec":"hevc","Title":"Video"}),
        json!({"Type":"Audio","Index":1,"Codec":"dts","Channels":6,"Language":"eng","Title":"Surround"}),
        json!({"Type":"Audio","Index":5,"Codec":"aac","Channels":2,"Language":"eng","Title":"English"}),
    ]);
    let patches = logical_track_patch(
        "Audio",
        1,
        "aac",
        json!(2),
        "eng",
        "English",
        json!({"Title":"英语主音轨"}),
    );
    let (output, report) = apply_patches(&original, &patches);
    assert_eq!(report["matched"], 1, "{report}");
    let mut expected = original.clone();
    expected["MediaSourceInfo"]["MediaStreams"][2]["Title"] = json!("英语主音轨");
    assert_eq!(
        output, expected,
        "the unique AAC track must be edited, not the DTS ordinal"
    );
}

#[test]
fn logical_track_original_title_difference_does_not_block_unique_title_edit() {
    let original = logical_track_info(vec![json!({"Type":"Audio","Index":9,
        "Codec":"aac","Channels":2,"Language":"jpn","Title":"Japanese stereo"})]);
    let patches = logical_track_patch(
        "Audio",
        1,
        "aac",
        json!(2),
        "jpn",
        "日语",
        json!({"Title":"日语主音轨"}),
    );
    let (output, report) = apply_patches(&original, &patches);
    assert_eq!(report["matched"], 1, "{report}");
    let mut expected = original.clone();
    expected["MediaSourceInfo"]["MediaStreams"][0]["Title"] = json!("日语主音轨");
    assert_eq!(output, expected);
}

#[test]
fn logical_track_explicit_language_must_keep_priority_when_target_already_has_that_value() {
    let original = logical_track_info(vec![json!({"Type":"Subtitle","Index":9,
        "Codec":"ass","Language":"chs","Title":"广配","IsExternal":false})]);
    let patches = logical_track_patch(
        "Subtitle",
        1,
        "ass",
        Value::Null,
        "chs",
        "广配",
        json!({"Title":"繁体广配字幕","Language":"chs"}),
    );
    let (mut output, report) = apply_patches(&original, &patches);
    assert_eq!(report["matched"], 1, "{report}");
    let positions = report["explicit_language_positions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_u64().unwrap() as usize)
        .collect::<Vec<_>>();
    assert_eq!(positions, vec![0]);
    crate::language::normalize_subtitles_for_save_with_explicit(&mut output, &original, &positions);
    assert_eq!(
        output["MediaSourceInfo"]["MediaStreams"][0]["Language"], "chs",
        "an explicit Language patch must outrank a conflicting title hint"
    );
}

#[test]
fn logical_track_known_different_languages_and_scripts_are_not_compatible() {
    for (source, target) in [
        ("eng", "jpn"),
        ("chs", "cht"),
        ("cht", "chs"),
        ("yue", "chi"),
    ] {
        let original = logical_track_info(vec![json!({"Type":"Subtitle","Index":1,
            "Codec":"ass","Language":target,"Title":"字幕","IsExternal":false})]);
        let patch = logical_track_patch(
            "Subtitle",
            1,
            "ass",
            Value::Null,
            source,
            "字幕",
            json!({"Title":"本次标题","Language":"chs"}),
        );
        let (output, report) = apply_patches(&original, &patch);
        assert_eq!(report["matched"], 0, "{source}/{target}: {report}");
        assert_eq!(output, original);
        assert!(
            report["skipped"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("Language")
        );
    }
}

#[test]
fn logical_track_generic_chinese_multiple_scripts_are_ambiguous() {
    let original = logical_track_info(vec![
        json!({"Type":"Subtitle","Index":0,"Codec":"ass","Language":"eng","Title":"English","IsExternal":false}),
        json!({"Type":"Subtitle","Index":1,"Codec":"ass","Language":"chi","Title":"广配","IsExternal":false}),
        json!({"Type":"Subtitle","Index":2,"Codec":"ass","Language":"chs","Title":"广配","IsExternal":false}),
        json!({"Type":"Subtitle","Index":3,"Codec":"ass","Language":"cht","Title":"广配","IsExternal":false}),
    ]);
    let patch = logical_track_patch(
        "Subtitle",
        1,
        "ass",
        Value::Null,
        "zho",
        "广配",
        json!({"Title":"广配字幕"}),
    );
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 0, "{report}");
    assert_eq!(output, original);
    assert!(
        report["skipped"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("无法唯一匹配")
    );
}

#[test]
fn logical_track_generic_chinese_title_script_hint_is_respected() {
    let original = logical_track_info(vec![json!({"Type":"Subtitle","Index":1,
        "Codec":"ass","Language":"zho","Title":"繁體广配","IsExternal":false})]);
    let patch = logical_track_patch(
        "Subtitle",
        1,
        "ass",
        Value::Null,
        "chs",
        "广配",
        json!({"Title":"广配字幕"}),
    );
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 0, "{report}");
    assert_eq!(output, original);
}

#[test]
fn logical_track_competing_patches_both_leave_target_intact() {
    for ordinal in [1, 2] {
        let original = logical_track_info(vec![json!({"Type":"Audio","Index":1,
            "Codec":"aac","Channels":2,"Language":"eng","Title":"English"})]);
        let first = logical_track_patch(
            "Audio",
            1,
            "aac",
            json!(2),
            "eng",
            "English",
            json!({"Title":"英语"}),
        );
        let second = logical_track_patch(
            "Audio",
            ordinal,
            "aac",
            json!(2),
            "eng",
            "English",
            json!({"Language":"jpn"}),
        );
        let (output, report) = apply_patches(&original, &json!([first[0], second[0]]));
        assert_eq!(report["matched"], 0, "{report}");
        assert_eq!(report["skipped"].as_array().unwrap().len(), 2);
        assert_eq!(output, original);
        assert!(
            report["skipped"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("竞争")
        );
    }
}

#[test]
fn logical_track_delete_requires_unique_complete_original_identity() {
    let stream = json!({"Type":"Audio","Index":1,"Codec":"aac","Channels":2,"Language":"eng","Title":"English"});
    let original = logical_track_info(vec![stream.clone(), stream.clone()]);
    let mut patch = logical_track_patch("Audio", 1, "aac", json!(2), "eng", "English", json!({}));
    patch[0]["delete"] = json!(true);
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 0, "{report}");
    assert_eq!(output, original);

    let mut changed_title = logical_track_info(vec![stream]);
    changed_title["MediaSourceInfo"]["MediaStreams"][0]["Title"] = json!("English commentary");
    let (output, report) = apply_patches(&changed_title, &patch);
    assert_eq!(report["matched"], 0, "{report}");
    assert_eq!(output, changed_title);

    let original = logical_track_info(vec![
        json!({"Type":"Audio","Index":1,"Codec":"dts","Channels":6,"Language":"eng","Title":"Surround"}),
        json!({"Type":"Audio","Index":2,"Codec":"aac","Channels":2,"Language":"eng","Title":"English"}),
    ]);
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 1, "{report}");
    let mut expected = original;
    expected["MediaSourceInfo"]["MediaStreams"]
        .as_array_mut()
        .unwrap()
        .remove(1);
    assert_eq!(output, expected);
}

#[test]
fn logical_track_explicit_language_positions_follow_deletion_and_other_title_inference() {
    let original = logical_track_info(vec![
        json!({"Type":"Audio","Index":1,"Codec":"aac","Channels":2,"Language":"eng","Title":"English"}),
        json!({"Type":"Subtitle","Index":9,"Codec":"ass","Language":"chs","Title":"广配","IsExternal":false}),
        json!({"Type":"Subtitle","Index":11,"Codec":"ass","Language":"zho","Title":"日配","IsExternal":false}),
    ]);
    let mut deletion =
        logical_track_patch("Audio", 1, "aac", json!(2), "eng", "English", json!({}));
    deletion[0]["delete"] = json!(true);
    let explicit = logical_track_patch(
        "Subtitle",
        1,
        "ass",
        Value::Null,
        "chs",
        "广配",
        json!({"Language":"chs","Title":"繁体广配"}),
    );
    let inferred = logical_track_patch(
        "Subtitle",
        2,
        "ass",
        Value::Null,
        "zho",
        "日配",
        json!({"Title":"简体日配"}),
    );
    let (mut output, report) =
        apply_patches(&original, &json!([deletion[0], explicit[0], inferred[0]]));
    assert_eq!(report["matched"], 3, "{report}");
    assert_eq!(report["explicit_language_positions"], json!([0]));
    crate::language::normalize_subtitles_for_save_with_explicit(&mut output, &original, &[0]);
    assert_eq!(
        output["MediaSourceInfo"]["MediaStreams"][0]["Language"],
        "chs"
    );
    assert_eq!(
        output["MediaSourceInfo"]["MediaStreams"][1]["Language"],
        "chs"
    );
}

#[test]
fn logical_track_codec_variation_needs_unique_strong_title_and_language_role() {
    let original = logical_track_info(vec![
        json!({"Type":"Audio","Index":1,"Codec":"ac3","Channels":2,"Language":"jpn","Title":"解说"}),
        json!({"Type":"Audio","Index":2,"Codec":"aac","Channels":2,"Language":"jpn","Title":"日配","SampleRate":44100,"BitRate":192000}),
    ]);
    let mut patch = logical_track_patch(
        "Audio",
        1,
        "ac3",
        json!(2),
        "jpn",
        "日配",
        json!({"Title":"日语主音轨","Codec":"ac3","Channels":99,"Size":1}),
    );
    patch[0]["identity"]["sample_rate"] = json!(48000);
    patch[0]["identity"]["bit_rate"] = json!(384000);
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 1, "{report}");
    let mut expected = original;
    expected["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("日语主音轨");
    assert_eq!(
        output, expected,
        "only requested editable fields may cross encodings"
    );

    let original = logical_track_info(vec![json!({"Type":"Audio","Index":1,
        "Codec":"aac","Channels":2,"Language":"jpn","Title":"日本語"})]);
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 0, "{report}");
    assert_eq!(output, original);
}

#[test]
fn logical_track_unknown_language_needs_matching_nonempty_title_and_codec() {
    for (source, target) in [("", "jpn"), ("eng", "")] {
        let original = logical_track_info(vec![json!({"Type":"Audio","Index":1,
            "Codec":"aac","Channels":2,"Language":target,"Title":"Original"})]);
        let patch = logical_track_patch(
            "Audio",
            1,
            "aac",
            json!(2),
            source,
            "Other",
            json!({"Title":"本次标题"}),
        );
        let (output, report) = apply_patches(&original, &patch);
        assert_eq!(report["matched"], 0, "{report}");
        assert_eq!(output, original);

        let patch = logical_track_patch(
            "Audio",
            1,
            "aac",
            json!(2),
            source,
            "Original",
            json!({"Title":"本次标题"}),
        );
        let (output, report) = apply_patches(&original, &patch);
        assert_eq!(report["matched"], 1, "{report}");
        assert_eq!(
            output["MediaSourceInfo"]["MediaStreams"][0]["Language"],
            target
        );
    }
    let original = logical_track_info(vec![json!({"Type":"Audio","Index":1,
        "Codec":"aac","Channels":2,"Language":"jpn","Title":"Original"})]);
    let mut patch = logical_track_patch(
        "Audio",
        1,
        "aac",
        json!(2),
        "",
        "",
        json!({"Title":"本次标题"}),
    );
    patch[0]["identity"]
        .as_object_mut()
        .unwrap()
        .remove("language");
    patch[0]["identity"]
        .as_object_mut()
        .unwrap()
        .remove("title");
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(
        report["matched"], 0,
        "missing identity keys cannot bypass role evidence: {report}"
    );
    assert_eq!(output, original);
}

#[test]
fn logical_track_numeric_and_iso_aliases_do_not_change_target_values() {
    let original = logical_track_info(vec![json!({"Type":"Audio","Index":1,
        "Codec":"aac","Channels":2.0,"Language":"ja","Title":"Japanese"})]);
    let patch = logical_track_patch(
        "Audio",
        1,
        "aac",
        json!(2),
        "jpn",
        "Japanese",
        json!({"Title":"日语"}),
    );
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 1, "{report}");
    let mut expected = original;
    expected["MediaSourceInfo"]["MediaStreams"][0]["Title"] = json!("日语");
    assert_eq!(output, expected);
}

#[test]
fn logical_track_two_unknown_languages_do_not_hide_different_roles() {
    let original = logical_track_info(vec![json!({"Type":"Audio","Index":1,
        "Codec":"aac","Channels":2,"Language":null,"Title":"Japanese"})]);
    let mut patch = logical_track_patch(
        "Audio",
        1,
        "aac",
        json!(2),
        "",
        "English",
        json!({"Title":"本次标题"}),
    );
    patch[0]["identity"]["language"] = Value::Null;
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 0, "{report}");
    assert_eq!(output, original);
}

#[test]
fn logical_track_single_video_can_change_title_across_encodings_but_multiple_videos_are_not_guessed()
 {
    let video = json!({"Type":"Video","Index":0,"Codec":"hevc","Title":null});
    let original = logical_track_info(vec![video]);
    let mut patch = logical_track_patch(
        "Video",
        1,
        "h264",
        Value::Null,
        "",
        "",
        json!({"Title":"硬字幕"}),
    );
    patch[0]["identity"]["title"] = Value::Null;
    patch[0]["identity"]["language"] = Value::Null;
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 1, "{report}");
    let mut expected = original;
    expected["MediaSourceInfo"]["MediaStreams"][0]["Title"] = json!("硬字幕");
    assert_eq!(output, expected);

    let first = json!({"Type":"Video","Index":0,"Codec":"h264","Title":null});
    let second = json!({"Type":"Video","Index":1,"Codec":"h264","Title":null});
    let original = logical_track_info(vec![first, second]);
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 0, "{report}");
    assert_eq!(output, original);
}

#[test]
fn logical_track_main_audio_does_not_match_an_explicit_commentary_track() {
    let original = logical_track_info(vec![json!({"Type":"Audio","Index":1,
        "Codec":"aac","Channels":2,"Language":"eng","Title":"Director Commentary"})]);
    let patch = logical_track_patch(
        "Audio",
        1,
        "aac",
        json!(2),
        "eng",
        "English Main",
        json!({"Title":"English Main Updated"}),
    );
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 0, "{report}");
    assert_eq!(output, original);
}

#[test]
fn logical_track_commentary_audio_does_not_match_an_ordinary_track() {
    let original = logical_track_info(vec![json!({"Type":"Audio","Index":1,
        "Codec":"aac","Channels":2,"Language":"eng","Title":"English Main"})]);
    let patch = logical_track_patch(
        "Audio",
        1,
        "aac",
        json!(2),
        "eng",
        "Director Commentary",
        json!({"Title":"Director Commentary Updated"}),
    );
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 0, "{report}");
    assert_eq!(output, original);
}

#[test]
fn logical_track_commentary_role_is_excluded_before_unique_main_audio_selection() {
    let original = logical_track_info(vec![
        json!({"Type":"Audio","Index":1,"Codec":"aac","Channels":2,"Language":"eng","Title":"Director Commentary"}),
        json!({"Type":"Audio","Index":2,"Codec":"aac","Channels":2,"Language":"eng","Title":"English Stereo"}),
    ]);
    let patch = logical_track_patch(
        "Audio",
        1,
        "aac",
        json!(2),
        "eng",
        "English Main",
        json!({"Title":"English Main Updated"}),
    );
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 1, "{report}");
    let mut expected = original;
    expected["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("English Main Updated");
    assert_eq!(output, expected);
}

#[test]
fn logical_track_original_forced_flag_rejects_full_and_forced_subtitle_mismatch() {
    for target_title in ["English Full", "English Forced"] {
        let original = logical_track_info(vec![json!({"Type":"Subtitle","Index":9,
            "Codec":"ass","Language":"eng","Title":target_title,
            "IsExternal":false,"IsForced":true})]);
        let mut patch = logical_track_patch(
            "Subtitle",
            1,
            "ass",
            Value::Null,
            "eng",
            "English Full",
            json!({"Title":"英语完整字幕"}),
        );
        patch[0]["identity"]["is_forced"] = json!(false);
        let (output, report) = apply_patches(&original, &patch);
        assert_eq!(report["matched"], 0, "{target_title}: {report}");
        assert_eq!(output, original);
    }
}

#[test]
fn logical_track_editing_forced_flag_still_matches_original_subtitle_role() {
    let original = logical_track_info(vec![
        json!({"Type":"Subtitle","Index":9,"Codec":"ass","Language":"eng",
            "Title":"English Full","IsExternal":false,"IsForced":true}),
        json!({"Type":"Subtitle","Index":11,"Codec":"ass","Language":"eng",
            "Title":"English Stereo","IsExternal":false,"IsForced":false}),
    ]);
    let mut patch = logical_track_patch(
        "Subtitle",
        1,
        "ass",
        Value::Null,
        "eng",
        "English Full",
        json!({"Title":"本次强制字幕","IsForced":true}),
    );
    patch[0]["identity"]["is_forced"] = json!(false);
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 1, "{report}");
    let mut expected = original;
    expected["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("本次强制字幕");
    expected["MediaSourceInfo"]["MediaStreams"][1]["IsForced"] = json!(true);
    assert_eq!(output, expected);
}

#[test]
fn logical_track_matching_forced_flags_allow_original_title_variation() {
    for forced in [false, true] {
        let original = logical_track_info(vec![json!({"Type":"Subtitle","Index":9,
            "Codec":"ass","Language":"eng","Title":"English Stereo",
            "IsExternal":false,"IsForced":forced})]);
        let mut patch = logical_track_patch(
            "Subtitle",
            1,
            "ass",
            Value::Null,
            "eng",
            "English Original",
            json!({"Title":"本次字幕标题"}),
        );
        patch[0]["identity"]["is_forced"] = json!(forced);
        let (output, report) = apply_patches(&original, &patch);
        assert_eq!(report["matched"], 1, "{forced}: {report}");
        let mut expected = original;
        expected["MediaSourceInfo"]["MediaStreams"][0]["Title"] = json!("本次字幕标题");
        assert_eq!(output, expected);
    }
}

#[test]
fn logical_track_legacy_subtitle_patch_needs_original_title_evidence_for_forced_target() {
    for (source_title, matched) in [("English Full", 0), ("English Forced", 1)] {
        let original = logical_track_info(vec![json!({"Type":"Subtitle","Index":9,
            "Codec":"ass","Language":"eng","Title":"English Forced",
            "IsExternal":false,"IsForced":true})]);
        let patch = logical_track_patch(
            "Subtitle",
            1,
            "ass",
            Value::Null,
            "eng",
            source_title,
            json!({"Title":"本次字幕标题"}),
        );
        let (output, report) = apply_patches(&original, &patch);
        assert_eq!(report["matched"], matched, "{source_title}: {report}");
        let mut expected = original;
        if matched == 1 {
            expected["MediaSourceInfo"]["MediaStreams"][0]["Title"] = json!("本次字幕标题");
        }
        assert_eq!(output, expected);
    }
}

#[test]
fn logical_track_forced_source_needs_original_title_evidence_when_target_flag_is_unknown() {
    let original = logical_track_info(vec![json!({"Type":"Subtitle","Index":9,
        "Codec":"ass","Language":"eng","Title":"English Full","IsExternal":false})]);
    let mut patch = logical_track_patch(
        "Subtitle",
        1,
        "ass",
        Value::Null,
        "eng",
        "English Forced",
        json!({"Title":"本次强制字幕"}),
    );
    patch[0]["identity"]["is_forced"] = json!(true);
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 0, "{report}");
    assert_eq!(output, original);
}

#[test]
fn logical_track_original_hearing_impaired_flag_rejects_full_and_sdh_subtitle_mismatch() {
    for (target_title, source_flag) in [
        ("English Full", Some(false)),
        ("English SDH", Some(false)),
        ("English SDH", None),
    ] {
        let original = logical_track_info(vec![json!({"Type":"Subtitle","Index":9,
            "Codec":"ass","Language":"eng","Title":target_title,
            "IsExternal":false,"IsHearingImpaired":true})]);
        let mut patch = logical_track_patch(
            "Subtitle",
            1,
            "ass",
            Value::Null,
            "eng",
            "English Full",
            json!({"Title":"英语完整字幕"}),
        );
        if let Some(flag) = source_flag {
            patch[0]["identity"]["is_hearing_impaired"] = json!(flag);
        }
        let (output, report) = apply_patches(&original, &patch);
        assert_eq!(
            report["matched"], 0,
            "{target_title}/{source_flag:?}: {report}"
        );
        assert_eq!(output, original);
    }
}

#[test]
fn logical_track_sdh_role_still_uses_original_identity_when_editing_title_and_forced_flag() {
    let original = logical_track_info(vec![
        json!({"Type":"Subtitle","Index":9,"Codec":"ass","Language":"eng",
            "Title":"English SDH","IsExternal":false,"IsForced":false,"IsHearingImpaired":false}),
        json!({"Type":"Subtitle","Index":11,"Codec":"ass","Language":"eng",
            "Title":"English SDH Alternate","IsExternal":false,"IsForced":false,"IsHearingImpaired":true}),
    ]);
    let mut patch = logical_track_patch(
        "Subtitle",
        1,
        "ass",
        Value::Null,
        "eng",
        "English SDH",
        json!({"Title":"英语完整字幕","IsForced":true}),
    );
    patch[0]["identity"]["is_forced"] = json!(false);
    patch[0]["identity"]["is_hearing_impaired"] = json!(true);
    let (output, report) = apply_patches(&original, &patch);
    assert_eq!(report["matched"], 1, "{report}");
    let mut expected = original;
    expected["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("英语完整字幕");
    expected["MediaSourceInfo"]["MediaStreams"][1]["IsForced"] = json!(true);
    assert_eq!(output, expected);
}
