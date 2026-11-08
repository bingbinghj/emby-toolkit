use emby_toolkit::{language, probe};
use serde_json::{Value, json};

fn extract(kind: &str, language: &str, title: &str) -> Value {
    probe::build(
        &json!({
            "format":{"duration":"100","format_name":"matroska"},
            "streams":[{
                "codec_type":kind,"codec_name":if kind == "audio" { "ac3" } else { "subrip" },
                "channels":2,"sample_rate":"48000",
                "tags":{"language":language,"title":title}
            }]
        }),
        &json!({"fn":"example.mkv"}),
        &json!([]),
    )
    .unwrap()
}

#[test]
fn probe_keeps_original_sources_and_commentary_numbers_for_shared_title_rules() {
    for (raw, title, expected) in [
        ("chi", "东和台配国语", "东和台配国语"),
        ("eng", "COMMENTARY2", "导评2"),
        ("eng", "English COMMENTARY#3", "导评3"),
    ] {
        let info = extract("audio", raw, title);
        let stream = &info[0]["MediaSourceInfo"]["MediaStreams"][0];
        assert_eq!(stream["Title"], title);
        assert!(stream["DisplayTitle"].as_str().unwrap().contains("AC3"));
        let (filled, report) = language::fill(&info, &json!([]), true);
        assert_eq!(
            filled[0]["MediaSourceInfo"]["MediaStreams"][0]["Title"],
            expected
        );
        assert_eq!(report["pending"], json!([]));
    }
}

#[test]
fn probe_does_not_rewrite_chinese_subtitle_titles_before_the_normal_skip() {
    for (raw, title) in [
        ("chi", "原盘繁体字幕（听译）"),
        ("ace", "对应台配国语"),
        ("ace", "对应DVD粤语"),
    ] {
        let info = extract("subtitle", raw, title);
        assert_eq!(
            info[0]["MediaSourceInfo"]["MediaStreams"][0]["Title"],
            title
        );
        let (filled, report) = language::fill(&info, &json!([]), true);
        assert_eq!(filled, info);
        assert_eq!(report["skipped_tracks"].as_array().unwrap().len(), 1);
        assert_eq!(report["pending"], json!([]));
    }
}

#[test]
fn empty_native_title_is_left_for_the_shared_language_fill() {
    let info = extract("audio", "jpn", "");
    assert_eq!(info[0]["MediaSourceInfo"]["MediaStreams"][0]["Title"], "");
    let (filled, _) = language::fill(&info, &json!([]), true);
    assert_eq!(
        filled[0]["MediaSourceInfo"]["MediaStreams"][0]["Title"],
        "日语"
    );
}
