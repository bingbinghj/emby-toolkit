use emby_toolkit::language::normalize_subtitles_for_save;
use serde_json::{Value, json};

#[test]
fn explicit_script_titles_normalize_saved_codes_and_display_without_losing_track_data() {
    for (title, language, display) in [
        ("简体", "chs", "Chinese Simplified"),
        ("台配简中特效（黑边）", "chs", "Chinese Simplified"),
        ("中简", "chs", "Chinese Simplified"),
        ("简英", "chs", "Chinese Simplified"),
        ("简英双语特效", "chs", "Chinese Simplified"),
        ("繁體", "cht", "Chinese Traditional"),
        ("原盘繁中（画面）", "cht", "Chinese Traditional"),
        ("中繁", "cht", "Chinese Traditional"),
        ("繁日", "cht", "Chinese Traditional"),
        ("繁德雙語", "cht", "Chinese Traditional"),
    ] {
        let original = json!({"Type":"Subtitle","Title":title,"Language":"chi",
            "DisplayLanguage":"Chinese","DisplayTitle":"Chinese (默认 强制 ASS)",
            "Codec":"ass","Index":5,"IsDefault":true,"IsForced":true});
        let mut info = json!([{"MediaSourceInfo":{"MediaStreams":[original.clone()]}}]);
        let mut before = info.clone();
        before[0]["MediaSourceInfo"]["MediaStreams"][0]["Title"] = json!("原始标题");
        normalize_subtitles_for_save(&mut info, &before);
        let mut expected = original;
        expected["Language"] = json!(language);
        expected["DisplayLanguage"] = json!(display);
        expected["DisplayTitle"] = json!(format!("{display} (默认 强制 ASS)"));
        assert_eq!(
            info[0]["MediaSourceInfo"]["MediaStreams"][0], expected,
            "{title}"
        );
    }
}

#[test]
fn generic_or_ambiguous_titles_and_audio_remain_unchanged() {
    for title in [
        "粤语",
        "导评",
        "中英双语",
        "简繁双语",
        "繁简英",
        "简体/繁体",
        "Chinese Simplified",
        "chs",
        "简英特效",
    ] {
        let mut info = json!({"MediaStreams":[{"Type":"Subtitle","Title":title,"Language":"chi"},
            {"Type":"Audio","Title":"简体","Language":"chi"}]});
        let mut original = info.clone();
        original["MediaStreams"][0]["Title"] = json!("原始标题");
        let requested = info.clone();
        normalize_subtitles_for_save(&mut info, &original);
        assert_eq!(info, requested, "{title}");
    }
}

#[test]
fn saved_normalization_does_not_invent_display_fields_and_is_idempotent() {
    let mut info = json!({"MediaStreams":[
        {"Type":"Subtitle","Title":"简体","Language":"chi"},
        {"Type":"Subtitle","Title":"繁体","Language":"chi","DisplayTitle":"自定义 ASS"}
    ]});
    let mut before = info.clone();
    for (index, stream) in before["MediaStreams"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        stream["Title"] = json!("原始标题");
        stream["Index"] = json!(index);
    }
    for (index, stream) in info["MediaStreams"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        stream["Index"] = json!(index);
    }
    normalize_subtitles_for_save(&mut info, &before);
    assert_eq!(
        info["MediaStreams"][0],
        json!({"Type":"Subtitle","Title":"简体","Language":"chs","Index":0})
    );
    assert_eq!(info["MediaStreams"][1]["DisplayTitle"], "自定义 ASS");
    assert_eq!(info["MediaStreams"][1]["Language"], "cht");
    let once: Value = info.clone();
    normalize_subtitles_for_save(&mut info, &before);
    assert_eq!(info, once);
}

#[test]
fn untouched_script_titles_and_explicit_language_corrections_take_priority() {
    for (title, language) in [("日语简体", "chi"), ("繁体", "chs")] {
        let original =
            json!({"MediaStreams":[{"Type":"Subtitle","Index":5,"Title":title,"Language":"chi"}]});
        let mut requested = original.clone();
        requested["MediaStreams"][0]["Language"] = json!(language);
        let expected = requested.clone();
        normalize_subtitles_for_save(&mut requested, &original);
        assert_eq!(requested, expected);
    }
}

#[test]
fn duplicate_track_indices_cannot_infer_a_language_from_a_title() {
    let original =
        json!({"MediaStreams":[{"Type":"Subtitle","Index":5,"Title":"原始标题","Language":"chi"}]});
    let mut requested = json!({"MediaStreams":[
        {"Type":"Subtitle","Index":5,"Title":"简体","Language":"chi"},
        {"Type":"Subtitle","Index":5,"Title":"繁体","Language":"chi"}
    ]});
    let before = requested.clone();
    normalize_subtitles_for_save(&mut requested, &original);
    assert_eq!(requested, before);
}
