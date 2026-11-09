use emby_toolkit::language;
use serde_json::{Value, json};

#[test]
fn nexus_cantonese_subtitles_keep_every_field_despite_incorrect_language_tags() {
    for title in [
        "TVB粤对",
        "DVD粤对",
        "TVB粵對",
        "DVD粵對",
        "亚齐文（TVB粤对）",
        "亚齐文（DVD粤对）",
    ] {
        let original = json!([{"MediaSourceInfo":{"MediaStreams":[{
            "Type":"Subtitle","Language":"ace","Title":title,
            "Codec":"subrip","DisplayTitle":"ace (SUBRIP)",
            "Index":8,"IsExternal":false,"IsDefault":true,"IsForced":false
        }]}}]);
        let (updated, report) = language::fill(&original, &json!([]), true);
        assert_eq!(updated, original, "{title}: {report}");
        assert_eq!(report["changed"], 0);
        assert_eq!(report["pending"], json!([]));
        assert_eq!(report["skipped_tracks"][0]["ordinal"], 1);
    }
}

#[test]
fn chinese_skips_do_not_stop_other_nexus_subtitles_or_audio_titles() {
    let original = json!([{"MediaSourceInfo":{"MediaStreams":[
        {"Type":"Subtitle","Language":"chi","Title":"台对"},
        {"Type":"Audio","Language":"chi","Title":"DVD粤"},
        {"Type":"Subtitle","Language":"ace","Title":"TVB粤对"},
        {"Type":"Subtitle","Language":"jpn","Title":"日对"},
        {"Type":"Subtitle","Language":"ace","Title":"DVD粤对"},
        {"Type":"Subtitle","Language":"eng","Title":"英文字幕"}
    ]}}]);
    let (updated, report) = language::fill(&original, &json!([]), true);
    let streams = &updated[0]["MediaSourceInfo"]["MediaStreams"];
    for index in [0, 2, 4] {
        assert_eq!(
            streams[index],
            original[0]["MediaSourceInfo"]["MediaStreams"][index]
        );
    }
    assert_eq!(streams[1]["Title"], "DVD粤语");
    assert_eq!(streams[1]["Language"], "yue");
    assert_eq!(streams[3]["Title"], "日文（日对）");
    assert_eq!(streams[5]["Title"], "英文");
    let ordinals: Vec<&Value> = report["skipped_tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| &entry["ordinal"])
        .collect();
    assert_eq!(ordinals, vec![&json!(1), &json!(2), &json!(4)]);
    assert_eq!(report["pending"], json!([]));
    let (again, repeated) = language::fill(&updated, &json!([]), true);
    assert_eq!(again, updated);
    assert_eq!(repeated["changed"], 0);
}
