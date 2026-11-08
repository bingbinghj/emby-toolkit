use super::*;

fn backup() -> Value {
    json!([{"MediaSourceInfo":{"MediaStreams":[
        {"Type":"Audio","Codec":"ac3","Title":"英语","Language":"eng"},
        {"Type":"Subtitle","Codec":"ass","Title":"备份中文一","Language":"chi","IsExternal":false,"Index":2},
        {"Type":"Subtitle","Codec":"subrip","Title":"English","Language":"eng","IsExternal":false,"Index":3},
        {"Type":"Subtitle","Codec":"ass","Title":"备份中文二","Language":"chi","IsExternal":false,"Index":4}
    ]}}])
}

fn skipped() -> Vec<Value> {
    vec![
        json!({"type":"Subtitle","ordinal":1}),
        json!({"type":"Subtitle","ordinal":3}),
    ]
}

fn live(info: &Value) -> Value {
    json!({"MediaStreams":info[0]["MediaSourceInfo"]["MediaStreams"]})
}

#[test]
fn skipped_subtitle_protection_rejects_added_same_codec_tracks_before_copying() {
    for position in [1, 2, 4] {
        let mut info = backup();
        let original = info.clone();
        let mut item = live(&info);
        item["MediaStreams"].as_array_mut().unwrap().insert(
            position,
            json!({"Type":"Subtitle","Codec":"ass","Title":"新字幕","Language":"chi","IsExternal":false}),
        );
        assert!(preserve_skipped_titles(&mut info, &item, &[], &skipped()).is_err());
        assert_eq!(info, original);
    }
}

#[test]
fn skipped_subtitle_protection_rejects_missing_tracks_before_copying() {
    for position in [1, 2, 3] {
        let mut info = backup();
        let original = info.clone();
        let mut item = live(&info);
        item["MediaStreams"]
            .as_array_mut()
            .unwrap()
            .remove(position);
        assert!(preserve_skipped_titles(&mut info, &item, &[], &skipped()).is_err());
        assert_eq!(info, original);
    }
}

#[test]
fn skipped_subtitle_protection_checks_scope_including_non_skipped_tracks() {
    for (index, field, value) in [
        (2, "Codec", json!("ass")),
        (2, "IsExternal", json!(true)),
        (3, "Codec", json!("subrip")),
    ] {
        let mut info = backup();
        let original = info.clone();
        let mut item = live(&info);
        item["MediaStreams"][index][field] = value;
        assert!(preserve_skipped_titles(&mut info, &item, &[], &skipped()).is_err());
        assert_eq!(info, original);
    }
}

#[test]
fn matching_subtitle_scope_preserves_current_titles_and_languages_with_different_indexes() {
    let mut info = backup();
    let mut item = live(&info);
    for index in [1, 3] {
        item["MediaStreams"][index]["Index"] = json!(100 + index);
        item["MediaStreams"][index]["Title"] = json!(format!("用户现有标题{index}"));
        item["MediaStreams"][index]["Language"] = json!("zho");
        item["MediaStreams"][index]["Codec"] = json!("ASS");
    }
    preserve_skipped_titles(&mut info, &item, &[], &skipped()).unwrap();
    for index in [1, 3] {
        assert_eq!(
            info[0]["MediaSourceInfo"]["MediaStreams"][index]["Title"],
            item["MediaStreams"][index]["Title"]
        );
        assert_eq!(
            info[0]["MediaSourceInfo"]["MediaStreams"][index]["Language"],
            "zho"
        );
    }
    assert_eq!(
        info[0]["MediaSourceInfo"]["MediaStreams"][2]["Title"],
        "English"
    );
}
