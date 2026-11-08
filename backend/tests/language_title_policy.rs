use emby_toolkit::language;
use serde_json::{Value, json};

fn info(streams: Value) -> Value {
    json!([{"MediaSourceInfo":{"MediaStreams":streams}}])
}

#[test]
fn nine_chinese_subtitles_are_normal_skips_without_pending_warnings() {
    let original = info(json!(
        (1..=9)
            .map(|n| json!({
                "Type":"Subtitle","Language":"chi","Title":format!("原始中文字幕{n}")
            }))
            .collect::<Vec<_>>()
    ));
    let (updated, report) = language::fill(&original, &json!([]), true);
    assert_eq!(updated, original);
    assert_eq!(report["changed"], 0);
    assert_eq!(report["pending"], json!([]));
    assert_eq!(report["skipped_tracks"].as_array().unwrap().len(), 9);
}

#[test]
fn skipped_subtitles_do_not_hide_unknown_tracks_or_shift_type_ordinals() {
    let original = info(json!([
        {"Type":"Subtitle","Language":"chi","Title":"简英双语"},
        {"Type":"Audio","Language":"jpn","Title":""},
        {"Type":"Subtitle","Language":"eng","Title":""},
        {"Type":"Subtitle","Language":"und","Title":""}
    ]));
    let (updated, report) = language::fill(&original, &json!([]), true);
    let streams = &updated[0]["MediaSourceInfo"]["MediaStreams"];
    assert_eq!(
        streams[0],
        original[0]["MediaSourceInfo"]["MediaStreams"][0]
    );
    assert_eq!(streams[1]["Title"], "日语");
    assert_eq!(streams[2]["Title"], "英文");
    assert_eq!(report["changed"], 2);
    assert_eq!(report["pending"].as_array().unwrap().len(), 1);
    assert_eq!(report["pending"][0]["ordinal"], 3);
    assert_eq!(report["skipped_tracks"][0]["ordinal"], 1);
}

#[test]
fn chinese_skip_includes_title_markers_and_never_skips_chinese_audio() {
    let original = info(json!([
        {"Type":"Subtitle","Language":"eng","Title":"繁英双语"},
        {"Type":"Audio","Language":"chi","Title":""}
    ]));
    let (updated, report) = language::fill(&original, &json!([]), true);
    assert_eq!(
        updated[0]["MediaSourceInfo"]["MediaStreams"][0]["Title"],
        "繁英双语"
    );
    assert_eq!(
        updated[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"],
        "国语"
    );
    assert_eq!(report["pending"], json!([]));
    assert_eq!(report["changed"], 1);
}

#[test]
fn explicit_chinese_fill_keeps_the_existing_manual_behavior() {
    let original = info(json!([{ "Type":"Subtitle","Language":"chi","Title":"" }]));
    let (updated, report) = language::fill(&original, &json!([]), false);
    assert_eq!(
        updated[0]["MediaSourceInfo"]["MediaStreams"][0]["Title"],
        "中文"
    );
    assert_eq!(report["pending"], json!([]));
    assert_eq!(report["changed"], 1);
}

#[test]
fn native_dubbing_titles_restore_language_and_keep_dubbing_sources() {
    let original = info(json!([
        {"Type":"Audio","Language":"chi","Title":"LPCM.爱子粤语","Codec":"pcm_s16le","Channels":2,"IsDefault":true},
        {"Type":"Audio","Language":"chi","Title":"LPCM.台配国语","Codec":"pcm_s16le","Channels":2},
        {"Type":"Audio","Language":"chi","Title":"LPCM.东和台配","Codec":"pcm_s16le","Channels":2},
        {"Type":"Audio","Language":"jpn","Title":"LPCM.原声日语","Codec":"pcm_s16le","Channels":2}
    ]));
    let (updated, report) = language::fill(&original, &json!([]), true);
    let streams = &updated[0]["MediaSourceInfo"]["MediaStreams"];
    for (index, (code, title)) in [
        ("yue", "粤语"),
        ("chi", "台配国语"),
        ("chi", "东和台配国语"),
        ("jpn", "日语"),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(streams[index]["Language"], code);
        assert_eq!(streams[index]["Title"], title);
        assert_eq!(streams[index]["Codec"], "pcm_s16le");
        assert_eq!(streams[index]["Channels"], 2);
    }
    assert_eq!(streams[0]["IsDefault"], true);
    assert_eq!(report["changed"], 4);
    assert_eq!(report["changed_titles"], 4);
    assert_eq!(report["changed_languages"], 1);
    assert_eq!(
        report["language_changes"],
        json!([{"type":"Audio","ordinal":1,"from":"chi","to":"yue"}])
    );
    let (again, repeated) = language::fill(&updated, &json!([]), true);
    assert_eq!(again, updated);
    assert_eq!(repeated["changed"], 0);
}

#[test]
fn title_inference_supports_iso_names_locale_tags_and_keeps_source_remarks() {
    for (kind, raw, title, code, expected) in [
        ("Audio", "und", "DTS-HD MA 5.1 Japanese", "jpn", "日语"),
        ("Audio", "jpn", "AAC.English SDH", "eng", "英语"),
        ("Audio", "und", "FLAC.French Commentary", "fra", "导评"),
        ("Audio", "chi", "LPCM.原生日语", "jpn", "日语"),
        ("Audio", "jpn", "LPCM.台配粤语", "yue", "粤语"),
        ("Audio", "und", "ja-JP Original", "jpn", "日语"),
        ("Subtitle", "und", "pt-BR SDH", "por", "葡萄牙文"),
        (
            "Subtitle",
            "por",
            "Portuguese (Brazil) SDH - Fansub",
            "por",
            "葡萄牙文 (Brazil) - Fansub",
        ),
        (
            "Subtitle",
            "und",
            "Spanish (Latin America)",
            "spa",
            "西班牙文 (Latin America)",
        ),
        ("Audio", "und", "English eng", "eng", "英语"),
    ] {
        let original = info(json!([{"Type":kind,"Language":raw,"Title":title}]));
        let (updated, report) = language::fill(&original, &json!([]), true);
        assert_eq!(
            updated[0]["MediaSourceInfo"]["MediaStreams"][0]["Language"], code,
            "{title}: {report}"
        );
        assert_eq!(
            updated[0]["MediaSourceInfo"]["MediaStreams"][0]["Title"], expected,
            "{title}: {report}"
        );
        assert_eq!(report["pending"], json!([]), "{title}: {report}");
        let (again, repeat) = language::fill(&updated, &json!([]), true);
        assert_eq!(again, updated, "{title}");
        assert_eq!(repeat["changed"], 0, "{title}");
    }
}

#[test]
fn custom_aliases_are_used_for_inference_and_override_iso_display_labels() {
    let mappings = json!([
        {"label":"定制克林贡语","value":"tlh","aliases":"Klingon,战士母语"},
        {"label":"自定义英语","value":"en","aliases":["English","eng"]}
    ]);
    let original = info(json!([
        {"Type":"Audio","Language":"und","Title":"LPCM.战士母语"},
        {"Type":"Audio","Language":"eng","Title":"English Commentary"}
    ]));
    let (updated, report) = language::fill(&original, &mappings, true);
    let streams = &updated[0]["MediaSourceInfo"]["MediaStreams"];
    assert_eq!(streams[0]["Language"], "tlh");
    assert_eq!(streams[0]["Title"], "定制克林贡语");
    assert_eq!(streams[1]["Language"], "eng");
    assert_eq!(streams[1]["Title"], "导评");
    assert_eq!(report["changed_languages"], 1);
    assert_eq!(report["changed"], 2);
}

#[test]
fn ambiguous_languages_and_feature_tokens_do_not_guess_a_language() {
    for title in ["Japanese / ENG", "English [jpn]", "stereo", "SDH"] {
        let original = info(json!([{"Type":"Audio","Language":"und","Title":title}]));
        let (updated, report) = language::fill(&original, &json!([]), true);
        assert_eq!(updated, original, "{title}: {report}");
        assert_eq!(report["changed"], 0, "{title}");
        assert_eq!(report["pending"].as_array().unwrap().len(), 1, "{title}");
    }
}

#[test]
fn chinese_skip_protects_all_subtitle_fields_before_reverse_language_inference() {
    let original = info(json!([
        {"Type":"Subtitle","Language":"eng","Title":"繁英双语","DisplayLanguage":"English","IsForced":true},
        {"Type":"Subtitle","Language":"und","Title":"Chinese SDH","DisplayTitle":"Original"},
        {"Type":"Subtitle","Language":"chi","Title":"English","Codec":"ass"}
    ]));
    let (updated, report) = language::fill(&original, &json!([]), true);
    assert_eq!(updated, original);
    assert_eq!(report["skipped_tracks"].as_array().unwrap().len(), 3);
    assert_eq!(report["language_changes"], json!([]));
    assert_eq!(report["pending"], json!([]));
}

#[test]
fn inferred_chinese_subtitles_are_skipped_without_changing_any_fields() {
    for title in ["CHI", "zho", "zh-CN", "Mandarin", "Cantonese"] {
        let original = info(json!([{
            "Type":"Subtitle","Language":"und","Title":title,
            "Codec":"subrip","IsForced":true,"Index":19
        }]));
        let (updated, report) = language::fill(&original, &json!([]), true);
        assert_eq!(updated, original, "{title}: {report}");
        assert_eq!(
            report["skipped_tracks"].as_array().unwrap().len(),
            1,
            "{title}: {report}"
        );
        assert_eq!(report["changed"], 0, "{title}: {report}");
        assert_eq!(report["pending"], json!([]), "{title}: {report}");
    }
}

#[test]
fn a_language_only_correction_counts_once_and_never_changes_technical_fields() {
    let original = info(
        json!([{"Type":"Audio","Language":"eng","Title":"粤语","Codec":"dts","Channels":6,"IsDefault":false}]),
    );
    let (updated, report) = language::fill(&original, &json!([]), true);
    assert_eq!(report["changed"], 1);
    assert_eq!(report["changed_titles"], 0);
    assert_eq!(report["changed_languages"], 1);
    assert_eq!(report["audio"], 1);
    assert_eq!(
        updated[0]["MediaSourceInfo"]["MediaStreams"][0],
        json!({"Type":"Audio","Language":"yue","Title":"粤语","Codec":"dts","Channels":6,"IsDefault":false})
    );
}

#[test]
fn technical_prefixes_and_prose_words_do_not_become_languages() {
    for (raw, title, code, expected) in [
        ("und", "FLAC.24-bit.96kHz.English", "eng", "英语"),
        ("jpn", "Hi-Res LPCM Japanese", "jpn", "日语"),
        ("und", "ENG SDH", "eng", "英语"),
        ("und", "eng SDH", "eng", "英语"),
        (
            "jpn",
            "English Commentary in Stereo",
            "eng",
            "导评 in Stereo",
        ),
    ] {
        let original = info(json!([{"Type":"Audio","Language":raw,"Title":title}]));
        let (updated, report) = language::fill(&original, &json!([]), true);
        assert_eq!(
            updated[0]["MediaSourceInfo"]["MediaStreams"][0]["Language"], code,
            "{title}: {report}"
        );
        assert_eq!(
            updated[0]["MediaSourceInfo"]["MediaStreams"][0]["Title"], expected,
            "{title}: {report}"
        );
        assert_eq!(report["pending"], json!([]), "{title}: {report}");
    }
    let original = info(json!([{"Type":"Audio","Language":"und","Title":"in stereo"}]));
    let (updated, report) = language::fill(&original, &json!([]), true);
    assert_eq!(updated, original);
    assert_eq!(report["pending"].as_array().unwrap().len(), 1);
}

#[test]
fn commentaries_use_chinese_titles_with_optional_numbers_and_are_idempotent() {
    for (raw, source, expected_language, expected_title) in [
        ("eng", "COMMENTARY1", "eng", "导评1"),
        ("eng", "COMMENTARY", "eng", "导评"),
        ("eng", "导评", "eng", "导评"),
        ("eng", "commentary # 2", "eng", "导评2"),
        ("eng", "CoMmEnTaRy 3", "eng", "导评3"),
        ("jpn", "Japanese COMMENTARY#4 SDH", "jpn", "导评4"),
        ("chi", "English Commentary #5", "eng", "导评5"),
        ("eng", "导评6", "eng", "导评6"),
        ("eng", "導評 #7", "eng", "导评7"),
        (
            "fra",
            "French Commentary (Brazil) by Alice",
            "fra",
            "导评 (Brazil) by Alice",
        ),
        (
            "eng",
            "English COMMENTARY #8 CC AD Forced - Fansub",
            "eng",
            "导评8 CC AD Forced - Fansub",
        ),
    ] {
        let original = info(
            json!([{"Type":"Audio","Language":raw,"Title":source,"Channels":2,"IsDefault":false}]),
        );
        let (updated, report) = language::fill(&original, &json!([]), true);
        let stream = &updated[0]["MediaSourceInfo"]["MediaStreams"][0];
        assert_eq!(stream["Title"], expected_title, "{source}: {report}");
        assert_eq!(stream["Language"], expected_language, "{source}: {report}");
        assert_eq!(stream["Channels"], 2);
        assert_eq!(stream["IsDefault"], false);
        assert_eq!(report["pending"], json!([]), "{source}: {report}");
        let (again, repeat) = language::fill(&updated, &json!([]), true);
        assert_eq!(again, updated, "{source}");
        assert_eq!(repeat["changed"], 0, "{source}");
    }
}

#[test]
fn unnumbered_commentary_is_localized_and_sdh_is_removed() {
    for (kind, raw, source, code, title) in [
        ("Audio", "eng", "Commentary", "eng", "导评"),
        ("Audio", "jpn", "SDH", "jpn", "日语"),
        ("Audio", "und", "Japanese 解说 SDH", "jpn", "日语"),
        ("Audio", "chi", "台配国语 解说", "chi", "台配国语"),
        ("Subtitle", "eng", "English SDH", "eng", "英文"),
    ] {
        let original = info(json!([{"Type":kind,"Language":raw,"Title":source}]));
        let (updated, report) = language::fill(&original, &json!([]), true);
        let stream = &updated[0]["MediaSourceInfo"]["MediaStreams"][0];
        assert_eq!(stream["Title"], title, "{source}: {report}");
        assert_eq!(stream["Language"], code, "{source}: {report}");
        assert_eq!(report["pending"], json!([]), "{source}: {report}");
    }
}

#[test]
fn unknown_or_conflicting_language_still_produces_a_ready_commentary_title() {
    for (raw, source, title) in [
        ("und", "COMMENTARY1", "导评1"),
        ("und", "COMMENTARY", "导评"),
        ("eng", "Japanese / English Commentary", "导评"),
        ("eng", "Japanese / English COMMENTARY#2", "导评2"),
    ] {
        let original = info(json!([{"Type":"Audio","Language":raw,"Title":source,"Codec":"aac"}]));
        let (updated, report) = language::fill(&original, &json!([]), true);
        let stream = &updated[0]["MediaSourceInfo"]["MediaStreams"][0];
        assert_eq!(stream["Title"], title, "{source}: {report}");
        assert_eq!(stream["Language"], raw);
        assert_eq!(stream["Codec"], "aac");
        assert_eq!(report["changed"], 1);
        assert_eq!(report["changed_titles"], 1);
        assert_eq!(report["changed_languages"], 0);
        assert_eq!(report["language_changes"], json!([]));
        assert_eq!(report["pending"].as_array().unwrap().len(), 1);
        assert_eq!(report["pending"][0]["title_ready"], true);
    }
    let original = info(json!([{"Type":"Audio","Language":"und","Title":"导评1"}]));
    let (updated, report) = language::fill(&original, &json!([]), true);
    assert_eq!(updated, original);
    assert_eq!(report["changed"], 0);
    assert_eq!(report["pending"][0]["title_ready"], true);
}

#[test]
fn taiwan_dubbing_sources_generalize_without_adding_other_dubbing_types() {
    for (raw, title, expected) in [
        ("chi", "LPCM.华视台配", "华视台配国语"),
        ("cmn", "LPCM.木棉花台配", "木棉花台配国语"),
        ("zho", "LPCM.东和台配", "东和台配国语"),
        ("chi", "LPCM.国配国语", "国语"),
        ("yue", "LPCM.港配粤语", "粤语"),
        ("chi", "LPCM.上译国语", "国语"),
        ("chi", "LPCM.某某国语", "国语"),
        ("jpn", "LPCM.台配", "日语"),
    ] {
        let original = info(json!([{"Type":"Audio","Language":raw,"Title":title}]));
        let (updated, report) = language::fill(&original, &json!([]), true);
        let stream = &updated[0]["MediaSourceInfo"]["MediaStreams"][0];
        assert_eq!(stream["Title"], expected, "{title}: {report}");
        assert_eq!(stream["Language"], raw, "{title}: {report}");
        assert_eq!(report["changed_languages"], 0);
        let (again, repeat) = language::fill(&updated, &json!([]), true);
        assert_eq!(again, updated, "{title}: {repeat}");
        assert_eq!(repeat["changed"], 0);
    }
}

#[test]
fn unknown_taiwan_dubbing_never_invents_a_language_or_language_title() {
    let original = info(json!([{"Type":"Audio","Language":"und","Title":"LPCM.华视台配"}]));
    let (updated, report) = language::fill(&original, &json!([]), true);
    let stream = &updated[0]["MediaSourceInfo"]["MediaStreams"][0];
    assert_eq!(stream["Title"], "华视台配");
    assert_eq!(stream["Language"], "und");
    assert_eq!(report["changed_languages"], 0);
    assert_eq!(report["language_changes"], json!([]));
    assert_eq!(report["pending"][0]["title_ready"], true);
    let (again, repeated) = language::fill(&updated, &json!([]), true);
    assert_eq!(again, updated);
    assert_eq!(repeated["changed"], 0);
    assert_eq!(repeated["pending"].as_array().unwrap().len(), 1);
    let explicit = info(json!([{"Type":"Audio","Language":"und","Title":"华视台配国语"}]));
    let (updated, report) = language::fill(&explicit, &json!([]), true);
    assert_eq!(
        updated[0]["MediaSourceInfo"]["MediaStreams"][0]["Language"],
        "chi"
    );
    assert_eq!(
        updated[0]["MediaSourceInfo"]["MediaStreams"][0]["Title"],
        "华视台配国语"
    );
    assert_eq!(report["pending"], json!([]));
}
