use emby_toolkit::{language, media_info};
use serde_json::{Value, json};

fn nexus_tracks() -> Value {
    json!({"MediaSourceInfo":{"MediaStreams":[
        {"Type":"Video","Index":0,"Codec":"h264"},
        {"Type":"Audio","Index":1,"Codec":"ac3","Channels":2,"Language":"chi","Title":"台","IsDefault":false},
        {"Type":"Audio","Index":2,"Codec":"aac","Channels":2,"Language":"chi","Title":"东和兴","IsDefault":false},
        {"Type":"Audio","Index":3,"Codec":"aac","Channels":2,"Language":"chi","Title":"福建文艺","IsDefault":false},
        {"Type":"Audio","Index":4,"Codec":"ac3","Channels":2,"Language":"chi","Title":"TVB粤","IsDefault":false},
        {"Type":"Audio","Index":5,"Codec":"ac3","Channels":2,"Language":"jpn","Title":"日","IsDefault":false},
        {"Type":"Audio","Index":6,"Codec":"ac3","Channels":2,"Language":"ace","Title":"DVD粤","IsDefault":false},
        {"Type":"Subtitle","Index":7,"Codec":"ass","Language":"chi","Title":"台对","IsExternal":false},
        {"Type":"Subtitle","Index":8,"Codec":"subrip","Language":"ace","Title":"TVB粤对","IsExternal":false},
        {"Type":"Subtitle","Index":9,"Codec":"ass","Language":"chi","Title":"日对","IsExternal":false}
    ]},"Chapters":[{"Name":"片头","MarkerType":"IntroStart","StartPositionTicks":287611537}]})
}

fn audio_patch(ordinal: u64, index: u64, language: &str, title: &str, changes: Value) -> Value {
    json!({"identity":{"type":"Audio","ordinal":ordinal,"index":index,
        "codec":"ac3","channels":2,"language":language,"title":title},"changes":changes})
}

#[test]
fn sixth_audio_matches_its_type_ordinal_when_it_moves_before_first_subtitle() {
    let original = nexus_tracks();
    let patch = audio_patch(
        6,
        7,
        "ace",
        "DVD粤",
        json!({"Title":"DVD粤语","Language":"yue"}),
    );
    let (filled, report) = media_info::apply_patches(&original, &json!([patch]));
    let mut expected = original;
    expected["MediaSourceInfo"]["MediaStreams"][6]["Title"] = json!("DVD粤语");
    expected["MediaSourceInfo"]["MediaStreams"][6]["Language"] = json!("yue");
    assert_eq!(report["matched"], 1, "{report}");
    assert_eq!(report["skipped"], json!([]));
    assert_eq!(
        filled, expected,
        "subtitle and global stream indices must stay unchanged"
    );
}

#[test]
fn explicit_dvd_source_matches_but_known_japanese_language_conflict_is_kept() {
    let mut original = nexus_tracks();
    original["MediaSourceInfo"]["MediaStreams"][1]["Codec"] = json!("aac");
    let patches = json!([
        audio_patch(
            5,
            5,
            "chi",
            "日语",
            json!({"Title":"日语","Language":"jpn","IsDefault":true})
        ),
        audio_patch(
            6,
            7,
            "chi",
            "DVD粤",
            json!({"Title":"DVD粤语","Language":"yue"})
        )
    ]);
    let (filled, report) = media_info::apply_patches(&original, &patches);
    let mut expected = original;
    expected["MediaSourceInfo"]["MediaStreams"][6]["Title"] = json!("DVD粤语");
    expected["MediaSourceInfo"]["MediaStreams"][6]["Language"] = json!("yue");
    assert_eq!(report["matched"], 1, "{report}");
    assert_eq!(report["skipped"].as_array().unwrap().len(), 1);
    assert_eq!(filled, expected);
}

#[test]
fn filled_dvd_title_matches_its_short_source_without_selecting_tvb_audio() {
    for source_title in ["DVD粤语", "dvd粤语"] {
        for target_title in ["DVD粤", "DVD粤语", "dvd粤语"] {
            let mut original = nexus_tracks();
            original["MediaSourceInfo"]["MediaStreams"][6]["Title"] = json!(target_title);
            let patch = audio_patch(6, 7, "yue", source_title, json!({"IsDefault":true}));
            let (filled, report) = media_info::apply_patches(&original, &json!([patch]));
            let mut expected = original;
            expected["MediaSourceInfo"]["MediaStreams"][6]["IsDefault"] = json!(true);
            assert_eq!(
                report["matched"], 1,
                "{source_title}/{target_title}: {report}"
            );
            assert_eq!(report["skipped"], json!([]));
            assert_eq!(
                filled, expected,
                "TVB track and unedited DVD title/language must stay unchanged"
            );
        }
    }
}

#[test]
fn title_evidence_still_refuses_duplicate_dvd_audio_candidates() {
    let stream =
        json!({"Type":"Audio","Codec":"ac3","Channels":2,"Language":"ace","Title":"DVD粤"});
    let original = json!({"MediaStreams":[stream.clone(),stream]});
    let patch = audio_patch(
        1,
        7,
        "chi",
        "DVD粤",
        json!({"Title":"DVD粤语","Language":"yue"}),
    );
    let (filled, report) = media_info::apply_patches(&original, &json!([patch]));
    assert_eq!(report["matched"], 0, "{report}");
    assert_eq!(filled, original);
    assert_eq!(report["skipped"].as_array().unwrap().len(), 1);
}

#[test]
fn audio_title_evidence_keeps_known_language_commentary_and_subtitle_conflicts_protected() {
    for (stream, identity) in [
        (
            json!({"Type":"Audio","Codec":"ac3","Channels":2,"Language":"jpn","Title":"主音轨"}),
            json!({"type":"Audio","ordinal":1,"codec":"ac3","channels":2,"language":"eng","title":"主音轨"}),
        ),
        (
            json!({"Type":"Audio","Codec":"ac3","Channels":2,"Language":"jpn","Title":"Unknown"}),
            json!({"type":"Audio","ordinal":1,"codec":"ac3","channels":2,"language":"eng","title":"Unknown"}),
        ),
        (
            json!({"Type":"Audio","Codec":"ac3","Channels":2,"Language":"jpn","Title":"日语 Commentary"}),
            json!({"type":"Audio","ordinal":1,"codec":"ac3","channels":2,"language":"chi","title":"日语"}),
        ),
        (
            json!({"Type":"Subtitle","Codec":"ass","Language":"chi","Title":"日对","IsExternal":false}),
            json!({"type":"Subtitle","ordinal":1,"codec":"ass","language":"jpn","title":"日对","is_external":false}),
        ),
    ] {
        let original = json!({"MediaStreams":[stream]});
        let (filled, report) = media_info::apply_patches(
            &original,
            &json!([{"identity":identity,"changes":{"Title":"本次标题"}}]),
        );
        assert_eq!(report["matched"], 0, "{report}");
        assert_eq!(filled, original);
    }
}

#[test]
fn known_japanese_audio_short_title_fills_without_a_duplicate_parenthetical() {
    for title in ["日", "日语（日）"] {
        let original = json!({"MediaStreams":[{"Type":"Audio","Index":5,"Codec":"ac3",
            "Channels":2,"Language":"jpn","Title":title,"IsDefault":false}]});
        let (filled, report) = language::fill(&original, &json!([]), true);
        let mut expected = original;
        expected["MediaStreams"][0]["Title"] = json!("日语");
        assert_eq!(filled, expected, "{title}: {report}");
        assert_eq!(report["pending"], json!([]));
        let (again, repeated) = language::fill(&filled, &json!([]), true);
        assert_eq!(again, filled);
        assert_eq!(repeated["changed"], 0);
    }
}

#[test]
fn short_japanese_title_rule_keeps_other_languages_and_real_remarks() {
    for (language, title, expected) in [
        ("chi", "日", "国语（日）"),
        ("jpn", "日语（日版）", "日语（日版）"),
        ("jpn", "日语（导演备注）", "日语（导演备注）"),
    ] {
        let original = json!({"MediaStreams":[{"Type":"Audio","Language":language,"Title":title}]});
        let (filled, report) = language::fill(&original, &json!([]), true);
        assert_eq!(
            filled["MediaStreams"][0]["Title"], expected,
            "{title}: {report}"
        );
        assert_eq!(filled["MediaStreams"][0]["Language"], language);
    }
}

#[test]
fn complete_language_titles_do_not_override_known_independent_audio_language() {
    for (source, source_title, target, target_title) in [
        ("jpn", "English", "eng", "English"),
        ("jpn", "英语", "eng", "English"),
        ("eng", "Japanese", "jpn", "Japanese"),
        ("jpn", "DVD粤语", "yue", "DVD粤"),
        ("eng", "DVD粤语", "yue", "DVD粤"),
        ("cmn", "DVD粤语", "yue", "DVD粤"),
        ("zh-cmn", "DVD粤语", "yue", "DVD粤"),
        ("mis", "English", "eng", "English"),
        ("unk", "English", "eng", "English"),
        ("nor", "English", "nob", "English"),
        ("no", "English", "nob", "English"),
        ("tgl", "English", "fil", "English"),
        ("guo", "English", "chi", "English"),
        ("tag", "English", "fil", "English"),
        ("dnj", "English", "dan", "English"),
        ("zh-cn", "English", "cht", "English"),
        ("zh-hans", "English", "cht", "English"),
        ("zh-tw", "English", "chs", "English"),
        ("zh-hant", "English", "chs", "English"),
        ("en-US", "English", "en-GB", "English"),
        ("en-US", "Japanese", "en", "Japanese"),
        ("zh-Hans-CN", "English", "cht", "English"),
        ("zh-Hans-CN", "English", "zh-Hant-TW", "English"),
    ] {
        let original = json!({"MediaStreams":[{"Type":"Audio","Index":1,
            "Codec":"ac3","Channels":2,"Language":target,"Title":target_title,"IsDefault":false}]});
        let patch = audio_patch(1, 17, source, source_title, json!({"IsDefault":true}));
        let (filled, report) = media_info::apply_patches(&original, &json!([patch]));
        assert_eq!(
            report["matched"], 0,
            "{source}/{source_title} -> {target}/{target_title}: {report}"
        );
        assert_eq!(filled, original);
    }
}

#[test]
fn complete_audio_language_title_aliases_keep_the_original_language_authoritative() {
    for (source, source_title, target, target_title) in [
        ("eng", "English", "eng", "英语"),
        ("en", "English", "eng", "英语"),
        ("und", "English", "eng", "English"),
        ("", "English", "eng", "English"),
        ("jpn", "English", "jpn", "English"),
        ("fre", "English", "fra", "English"),
        ("en-US", "Japanese", "en-US", "Japanese"),
        ("zh-cn", "English", "chs", "English"),
        ("zh-tw", "English", "cht", "English"),
        ("no", "English", "nor", "English"),
        ("tl", "English", "tgl", "English"),
    ] {
        let original = json!({"MediaStreams":[{"Type":"Audio","Index":1,
            "Codec":"ac3","Channels":2,"Language":target,"Title":target_title,"IsDefault":false}]});
        let patch = audio_patch(1, 17, source, source_title, json!({"IsDefault":true}));
        let (filled, report) = media_info::apply_patches(&original, &json!([patch]));
        let mut expected = original;
        expected["MediaStreams"][0]["IsDefault"] = json!(true);
        assert_eq!(
            report["matched"], 1,
            "{source}/{source_title} -> {target}/{target_title}: {report}"
        );
        assert_eq!(filled, expected);
    }
}

#[test]
fn unmarked_audio_language_still_needs_the_original_title_and_codec_to_match() {
    for source_language in ["", "und"] {
        let original = json!({"MediaStreams":[{"Type":"Audio","Index":1,
            "Codec":"aac","Channels":2,"Language":"eng","Title":"English","IsDefault":false}]});
        let patch = audio_patch(1, 17, source_language, "English", json!({"IsDefault":true}));
        let (filled, report) = media_info::apply_patches(&original, &json!([patch]));
        assert_eq!(report["matched"], 0, "{source_language}: {report}");
        assert_eq!(filled, original);
    }
}
