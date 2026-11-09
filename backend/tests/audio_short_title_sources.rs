use emby_toolkit::language;
use serde_json::{Value, json};

fn audio(raw: Option<&str>, title: &str) -> Value {
    let mut stream = json!({
        "Type":"Audio","Title":title,"Codec":"ac3","Channels":2,
        "IsDefault":true,"Index":7
    });
    if let Some(raw) = raw {
        stream["Language"] = json!(raw);
    }
    json!([{"MediaSourceInfo":{"MediaStreams":[stream]}}])
}

#[test]
fn native_short_dubbing_titles_keep_sources_and_correct_the_language() {
    for (title, expected, code) in [
        ("台", "台配国语", "chi"),
        ("东和兴", "东和兴国语", "chi"),
        ("福建文艺", "福建文艺国语", "chi"),
        ("DVD粤", "DVD粤语", "yue"),
        ("TVB粤", "TVB粤语", "yue"),
        ("AC3.DVD粤", "DVD粤语", "yue"),
        ("tvb粤", "TVB粤语", "yue"),
    ] {
        for raw in [Some("chi"), Some("ace"), Some("und"), None] {
            let original = audio(raw, title);
            let (updated, report) = language::fill(&original, &json!([]), true);
            let mut expected_stream = original[0]["MediaSourceInfo"]["MediaStreams"][0].clone();
            expected_stream["Title"] = json!(expected);
            expected_stream["Language"] = json!(code);
            assert_eq!(
                updated[0]["MediaSourceInfo"]["MediaStreams"][0], expected_stream,
                "{raw:?} / {title}: {report}"
            );
            assert_eq!(report["pending"], json!([]), "{title}: {report}");
            let (again, repeated) = language::fill(&updated, &json!([]), true);
            assert_eq!(again, updated, "{title}: {repeated}");
            assert_eq!(repeated["changed"], 0, "{title}: {repeated}");
        }
    }
}

#[test]
fn previously_wrapped_short_dubbing_titles_can_be_filled_again() {
    for (raw, title, expected, code) in [
        ("chi", "国语（台）", "台配国语", "chi"),
        ("chi", "国语（东和兴）", "东和兴国语", "chi"),
        ("chi", "国语（福建文艺）", "福建文艺国语", "chi"),
        ("chi", "国语（DVD粤）", "DVD粤语", "yue"),
        ("ace", "亚齐语（DVD粤）", "DVD粤语", "yue"),
        ("chi", "国语（TVB粤）", "TVB粤语", "yue"),
    ] {
        let (updated, report) = language::fill(&audio(Some(raw), title), &json!([]), true);
        let stream = &updated[0]["MediaSourceInfo"]["MediaStreams"][0];
        assert_eq!(stream["Title"], expected, "{title}: {report}");
        assert_eq!(stream["Language"], code, "{title}: {report}");
        assert_eq!(report["pending"], json!([]), "{title}: {report}");
        let (again, repeated) = language::fill(&updated, &json!([]), true);
        assert_eq!(again, updated, "{title}: {repeated}");
    }
}

#[test]
fn similar_source_names_do_not_guess_a_language_or_add_a_dubbing_source() {
    for title in ["东和兴特别版", "福建文艺纪念版"] {
        let original = audio(Some("und"), title);
        let (updated, report) = language::fill(&original, &json!([]), true);
        assert_eq!(updated, original, "{title}: {report}");
        assert_eq!(report["changed"], 0, "{title}: {report}");
        assert_eq!(report["pending"][0]["title_ready"], false);
    }
    for (raw, title, expected, code) in [
        ("chi", "东和兴特别版", "国语（东和兴特别版）", "chi"),
        ("chi", "福建文艺纪念版", "国语（福建文艺纪念版）", "chi"),
        ("chi", "东和兴英语", "英语", "eng"),
        ("chi", "福建文艺粤语", "粤语", "yue"),
    ] {
        let (updated, report) = language::fill(&audio(Some(raw), title), &json!([]), true);
        let stream = &updated[0]["MediaSourceInfo"]["MediaStreams"][0];
        assert_eq!(stream["Title"], expected, "{title}: {report}");
        assert_eq!(stream["Language"], code, "{title}: {report}");
    }
}
