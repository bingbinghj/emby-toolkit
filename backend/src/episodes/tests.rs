use super::*;

fn track(codec: &str, language: &str, title: &str) -> Value {
    json!({"codec":codec,"channels":"2","language":language,"title":title})
}

fn episode(audio: Vec<Value>, season: i64, version: i64) -> Value {
    json!({"season_number":season,"version_index":version,
        "_track_structure":{"audio":audio,"subtitles":[]}})
}

#[test]
fn missing_track_uses_closest_majority_reference_instead_of_last_episode() {
    let flac = track("flac", "chi", "国语");
    let taiwan = track("aac", "chi", "台配国语");
    let tvb = track("ac3", "yue", "TVB粤语");
    let cantonese = track("mp3", "yue", "粤语");
    let japanese = track("aac", "jpn", "日语");
    let mut items = vec![
        episode(
            vec![
                flac.clone(),
                taiwan.clone(),
                tvb.clone(),
                cantonese.clone(),
                japanese.clone(),
            ],
            1,
            1,
        ),
        episode(
            vec![taiwan.clone(), tvb, cantonese.clone(), japanese.clone()],
            1,
            1,
        ),
        episode(
            vec![
                flac,
                taiwan,
                track("aac", "yue", "TVB粤语"),
                cantonese,
                japanese,
            ],
            1,
            1,
        ),
    ];
    annotate(&mut items);
    assert_eq!(items[1]["track_status"], "inconsistent");
    assert_eq!(
        items[1]["track_difference"],
        "音轨数量 5→4（缺少 音轨1 FLAC 2ch Language=chi Title=国语）"
    );
}

#[test]
fn equally_close_references_with_different_missing_identities_remain_ambiguous() {
    let japanese = track("aac", "jpn", "日语");
    let mut items = vec![
        episode(vec![japanese.clone(), track("flac", "chi", "国语")], 1, 1),
        episode(vec![japanese.clone()], 1, 1),
        episode(vec![japanese, track("ac3", "eng", "英语")], 1, 1),
    ];
    annotate(&mut items);
    assert_eq!(items[1]["track_status"], "inconsistent");
    let hint = items[1]["track_difference"].as_str().unwrap();
    assert!(
        hint.contains("参考轨道结构不一致，无法可靠定位增减音轨"),
        "{hint}"
    );
    assert!(!hint.contains("缺少 音轨"), "{hint}");
}

#[test]
fn identical_extra_tracks_are_reported_without_guessing_physical_ordinal() {
    let english = track("ac3", "eng", "英语");
    let japanese = track("aac", "jpn", "日语");
    let mut items = vec![
        episode(vec![japanese.clone(), english.clone()], 1, 1),
        episode(vec![japanese.clone(), english.clone()], 1, 1),
        episode(vec![japanese, english.clone(), english], 1, 1),
    ];
    annotate(&mut items);
    let hint = items[2]["track_difference"].as_str().unwrap();
    assert!(hint.contains("同属性音轨多1条"), "{hint}");
    assert!(hint.contains("具体序号无法唯一定位"), "{hint}");
    assert!(!hint.contains("音轨3"), "{hint}");
}

#[test]
fn identical_missing_tracks_are_grouped_without_guessing_physical_ordinal() {
    let english = track("ac3", "eng", "英语");
    let mut items = vec![
        episode(
            vec![english.clone(), english.clone(), english.clone()],
            1,
            1,
        ),
        episode(
            vec![english.clone(), english.clone(), english.clone()],
            1,
            1,
        ),
        episode(vec![english], 1, 1),
    ];
    annotate(&mut items);
    let hint = items[2]["track_difference"].as_str().unwrap();
    assert!(hint.contains("同属性音轨少2条"), "{hint}");
    assert_eq!(hint.matches("具体序号无法唯一定位").count(), 1, "{hint}");
    assert!(!hint.contains("音轨2"), "{hint}");
    assert!(!hint.contains("音轨3"), "{hint}");
}

#[test]
fn unique_extra_track_keeps_specific_ordinal() {
    let english = track("ac3", "eng", "英语");
    let japanese = track("aac", "jpn", "日语");
    let mut items = vec![
        episode(vec![english.clone()], 1, 1),
        episode(vec![english.clone()], 1, 1),
        episode(vec![english, japanese], 1, 1),
    ];
    annotate(&mut items);
    assert_eq!(
        items[2]["track_difference"],
        "音轨数量 1→2（多出 音轨2 AAC 2ch Language=jpn Title=日语）"
    );
}

#[test]
fn reference_order_changes_do_not_make_an_extra_track_ambiguous() {
    let english = track("ac3", "eng", "英语");
    let japanese = track("aac", "jpn", "日语");
    let cantonese = track("flac", "yue", "粤语");
    let mut items = vec![
        episode(vec![english.clone(), japanese.clone()], 1, 1),
        episode(vec![japanese.clone(), english.clone()], 1, 1),
        episode(vec![english, japanese, cantonese], 1, 1),
    ];
    annotate(&mut items);
    assert_eq!(
        items[2]["track_difference"],
        "音轨数量 2→3（多出 音轨3 FLAC 2ch Language=yue Title=粤语）"
    );
}

#[test]
fn equal_counts_stay_consistent_even_when_properties_change() {
    let mut items = vec![
        episode(vec![track("flac", "chi", "国语")], 1, 1),
        episode(vec![track("aac", "jpn", "日语")], 1, 1),
    ];
    annotate(&mut items);
    for item in items {
        assert_eq!(item["track_status"], "consistent");
        assert_eq!(item["track_difference"], "");
    }
}

#[test]
fn unavailable_items_and_season_version_groups_keep_existing_behavior() {
    let one = vec![track("aac", "jpn", "日语")];
    let two = vec![track("ac3", "eng", "英语"), track("aac", "jpn", "日语")];
    let mut items = vec![
        episode(one, 1, 1),
        episode(two.clone(), 1, 2),
        episode(two, 2, 1),
        json!({"season_number":1,"version_index":1,"_track_structure":null}),
    ];
    annotate(&mut items);
    for item in &items[..3] {
        assert_eq!(item["track_status"], "consistent");
    }
    assert_eq!(items[3]["track_status"], "unavailable");
    assert_eq!(items[3]["track_difference"], "");
    assert!(items.iter().all(
        |item| item.get("track_structure").is_none() && item.get("_track_structure").is_none()
    ));
}
