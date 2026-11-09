use emby_toolkit::language;
use serde_json::{Value, json};

fn audio(raw: Option<&str>, title: &str) -> Value {
    let mut stream =
        json!({"Type":"Audio","Title":title,"Codec":"aac","Channels":2,"IsDefault":true});
    if let Some(raw) = raw {
        stream["Language"] = json!(raw);
    }
    json!({"MediaStreams":[stream]})
}

#[test]
fn he_aac_codec_markers_never_supply_a_language() {
    for title in [
        "HE-AAC",
        "AAC-HE",
        "HE AAC",
        "HE_AAC",
        "HE-AACv2",
        "HE-AAC v2",
        "AAC-HEv2",
        "AAC_HE_v2",
        "HE-AAC 2.0",
        "HE-AACv2 2.0",
    ] {
        for (raw, expected) in [("jpn", "日语"), ("eng", "英语")] {
            let original = audio(Some(raw), title);
            let (filled, report) = language::fill(&original, &json!([]), true);
            let mut expected_stream = original["MediaStreams"][0].clone();
            expected_stream["Title"] = json!(expected);
            assert_eq!(
                filled["MediaStreams"][0], expected_stream,
                "{title}: {report}"
            );
            assert_eq!(report["changed_languages"], 0, "{title}: {report}");
            assert_eq!(report["pending"], json!([]), "{title}: {report}");
        }
        for raw in [Some("und"), None] {
            let original = audio(raw, title);
            let (filled, report) = language::fill(&original, &json!([]), true);
            assert_eq!(filled, original, "{title}: {report}");
            assert_eq!(report["changed"], 0, "{title}: {report}");
            assert_eq!(report["pending"].as_array().unwrap().len(), 1);
        }
    }
}

#[test]
fn codec_prefixes_do_not_create_false_language_conflicts() {
    for (raw, title, expected, code) in [
        ("jpn", "HE-AAC stereo", "日语（stereo）", "jpn"),
        ("jpn", "HE-AAC 日语", "日语", "jpn"),
        ("eng", "HE-AAC 日语", "日语", "jpn"),
        ("und", "AAC-HE English", "英语", "eng"),
        ("und", "HE-AACv2 日语", "日语", "jpn"),
        ("jpn", "HE", "希伯来语", "heb"),
    ] {
        let original = audio(Some(raw), title);
        let (filled, report) = language::fill(&original, &json!([]), true);
        assert_eq!(
            filled["MediaStreams"][0]["Title"], expected,
            "{title}: {report}"
        );
        assert_eq!(
            filled["MediaStreams"][0]["Language"], code,
            "{title}: {report}"
        );
        assert_eq!(report["pending"], json!([]), "{title}: {report}");
        let (again, repeated) = language::fill(&filled, &json!([]), true);
        assert_eq!(again, filled, "{title}: {repeated}");
        assert_eq!(repeated["changed"], 0, "{title}: {repeated}");
    }
}

#[test]
fn he_aac_markers_in_the_middle_or_end_do_not_become_languages() {
    for (raw, title, expected, code) in [
        ("jpn", "日语 HE-AAC", "日语 HE-AAC", "jpn"),
        ("jpn", "Japanese HE-AAC", "日语 HE-AAC", "jpn"),
        ("jpn", "English AAC-HE", "英语 AAC-HE", "eng"),
        ("eng", "Japanese HE_AACv2 SDH", "日语 HE_AACv2", "jpn"),
        ("jpn", "日语HE-AAC", "日语HE-AAC", "jpn"),
        ("jpn", "英语AAC-HE", "英语AAC-HE", "eng"),
        ("jpn", "MYHE-AAC", "日语（MYHE-AAC）", "jpn"),
    ] {
        let original = audio(Some(raw), title);
        let (filled, report) = language::fill(&original, &json!([]), true);
        assert_eq!(
            filled["MediaStreams"][0]["Title"], expected,
            "{title}: {report}"
        );
        assert_eq!(
            filled["MediaStreams"][0]["Language"], code,
            "{title}: {report}"
        );
        assert_eq!(report["pending"], json!([]), "{title}: {report}");
        let (again, repeated) = language::fill(&filled, &json!([]), true);
        assert_eq!(again, filled, "{title}: {repeated}");
    }
    for title in ["original HE-AAC", "Japanese HE"] {
        let original = audio(Some("und"), title);
        let (filled, report) = language::fill(&original, &json!([]), true);
        assert_eq!(filled, original, "{title}: {report}");
        assert_eq!(report["pending"].as_array().unwrap().len(), 1);
    }
}
