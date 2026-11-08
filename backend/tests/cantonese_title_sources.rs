use emby_toolkit::language;
use serde_json::{Value, json};

fn audio(language: &str, title: &str) -> Value {
    json!([{"MediaSourceInfo":{"MediaStreams":[{
        "Type":"Audio","Language":language,"Title":title,
        "Codec":"ac3","Channels":2,"IsDefault":false,"IsForced":true
    }]}}])
}

#[test]
fn cantonese_dubbing_sources_survive_title_fill_and_language_correction() {
    for (raw, title, expected, code) in [
        ("yue", "DVD粤语", "DVD粤语", "yue"),
        ("yue", "TVB粤语", "TVB粤语", "yue"),
        ("chi", "AC3.DVD粤语", "DVD粤语", "yue"),
        ("und", "AC3.TVB粤语", "TVB粤语", "yue"),
        ("eng", "DTS-HD MA 2.0 tvb Cantonese", "TVB粤语", "yue"),
        ("yue", "DVD.粤语 SDH", "DVD粤语", "yue"),
        ("yue", "爱子DVD粤语", "DVD粤语", "yue"),
    ] {
        let original = audio(raw, title);
        let (updated, report) = language::fill(&original, &json!([]), true);
        let mut expected_stream = original[0]["MediaSourceInfo"]["MediaStreams"][0].clone();
        expected_stream["Title"] = json!(expected);
        expected_stream["Language"] = json!(code);
        assert_eq!(
            updated[0]["MediaSourceInfo"]["MediaStreams"][0], expected_stream,
            "{title}: {report}"
        );
        assert_eq!(report["pending"], json!([]), "{title}: {report}");
        let (again, repeated) = language::fill(&updated, &json!([]), true);
        assert_eq!(again, updated, "{title}: {repeated}");
        assert_eq!(repeated["changed"], 0, "{title}: {repeated}");
    }
}

#[test]
fn cantonese_sources_do_not_restore_excluded_sources_or_guess_a_language() {
    for (raw, title, expected) in [
        ("yue", "爱子粤语", "粤语"),
        ("yue", "港配粤语", "粤语"),
        ("yue", "MYDVD粤语", "粤语"),
        ("yue", "TVBextra粤语", "粤语"),
        ("chi", "DVD国语", "国语"),
        ("jpn", "TVB日语", "日语"),
        ("chi", "东和台配国语", "东和台配国语"),
        ("yue", "DVD粤语 COMMENTARY2", "DVD 导评2"),
    ] {
        let (updated, report) = language::fill(&audio(raw, title), &json!([]), true);
        let stream = &updated[0]["MediaSourceInfo"]["MediaStreams"][0];
        assert_eq!(stream["Title"], expected, "{title}: {report}");
        assert_eq!(stream["Language"], raw, "{title}: {report}");
    }
    let original = audio("und", "DVD / TVB");
    let (updated, report) = language::fill(&original, &json!([]), true);
    assert_eq!(updated, original);
    assert_eq!(report["pending"].as_array().unwrap().len(), 1);
}
