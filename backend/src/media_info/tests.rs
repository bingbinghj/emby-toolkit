use super::*;
use serde_json::json;

fn source(id: &str, owner: Option<&str>, path: &str) -> Value {
    json!({"Id":id,"ItemId":owner,"Path":path,"MediaStreams":[{"Type":"Audio","Title":id}]})
}

fn item(sources: Vec<Value>) -> Value {
    json!({"Id":"102","MediaSources":sources})
}

#[test]
fn preserve_linked_version_uses_native_streams_and_keeps_container_fields() {
    let original = json!({"MediaSourceInfo":{"Size":100,"Container":"mkv","MediaStreams":[
        {"Type":"Audio","Index":1,"Title":"旧标题","Language":"jpn","IsDefault":true}
    ]},"Chapters":[]});
    let streams = json!([
        {"Type":"Video","Index":0,"Codec":"hevc"},
        {"Type":"Audio","Index":1,"Codec":"aac","Title":"用户国语","Language":"chi","IsDefault":false},
        {"Type":"Subtitle","Index":2,"Title":"用户字幕","Language":"yue","IsForced":true,"IsExternal":true,"Path":"/media/movie.zh.srt"}
    ]);
    let live = json!({"Id":"102","MediaSources":[
        {"ItemId":"101","Path":"/media/movie.mkv","MediaStreams":[{"Type":"Audio","Title":"其他版本"}]},
        {"ItemId":"102","Path":"/media/movie.mkv","MediaStreams":streams}
    ]});
    let mut protected = original.clone();
    preserve_live_streams(&mut protected, &live, &["/media/movie.mkv"]).unwrap();
    assert_eq!(protected["MediaSourceInfo"]["MediaStreams"], streams);
    assert_eq!(
        protected["MediaSourceInfo"]["Size"],
        original["MediaSourceInfo"]["Size"]
    );
    assert_eq!(
        protected["MediaSourceInfo"]["Container"],
        original["MediaSourceInfo"]["Container"]
    );
}

#[test]
fn preserve_linked_version_keeps_recoverable_backup_when_native_streams_are_empty() {
    let original =
        json!({"MediaSourceInfo":{"MediaStreams":[{"Type":"Audio","Title":"恢复备份"}]}});
    for live in [
        json!({"Id":"102","MediaSources":[],"MediaStreams":[]}),
        json!({"Id":"102","MediaSources":[{"ItemId":"102","Path":"/media/movie.mkv","MediaStreams":[]}]}),
    ] {
        let mut protected = original.clone();
        preserve_live_streams(&mut protected, &live, &["/media/movie.mkv"]).unwrap();
        assert_eq!(protected, original);
    }
}

#[test]
fn preserve_linked_version_rejects_ambiguous_native_sources_without_changing_backup() {
    let live = item(vec![
        source("one", Some("102"), "/media/movie.mkv"),
        source("two", Some("102"), "/media/movie.mkv"),
    ]);
    let original =
        json!({"MediaSourceInfo":{"MediaStreams":[{"Type":"Audio","Title":"恢复备份"}]}});
    let mut protected = original.clone();
    assert!(preserve_live_streams(&mut protected, &live, &["/media/movie.mkv"]).is_err());
    assert_eq!(protected, original);
}

#[test]
fn matching_sources_with_the_same_explicit_owner_are_ambiguous() {
    let item = item(vec![
        source("source-a", Some("102"), "/media/movie.mkv"),
        source("source-b", Some("102"), "/media/movie.mkv"),
    ]);
    assert!(item_streams(&item, &["/media/movie.mkv"]).is_none());
}

#[test]
fn normalized_aliases_with_the_same_explicit_owner_are_ambiguous() {
    let item = item(vec![
        source("source-a", Some("102"), "/media/movie.mkv"),
        source("source-b", Some("102"), "/media/%6dovie.mkv"),
    ]);
    assert!(item_streams(&item, &["file:///media/movie.mkv"]).is_none());
}

#[test]
fn exact_paths_keep_priority_over_normalized_aliases_of_the_same_owner() {
    let item = item(vec![
        source("source-a", Some("102"), "file:///media/movie.mkv"),
        source("source-b", Some("102"), "/media/movie.mkv"),
    ]);
    assert_eq!(
        item_streams(&item, &["/media/movie.mkv"]).unwrap()[0]["Title"],
        "source-b"
    );
}

#[test]
fn unique_native_and_legacy_sources_keep_existing_selection_rules() {
    for (sources, target, selected) in [
        (
            vec![
                source("opaque", None, "/media/movie.mkv"),
                source("native", Some("102"), "/media/movie.mkv"),
            ],
            "/media/movie.mkv",
            "native",
        ),
        (
            vec![
                source("other", Some("101"), "/media/movie.mkv"),
                source("native", Some("102"), "/media/movie.mkv"),
            ],
            "/media/movie.mkv",
            "native",
        ),
        (
            vec![
                source("mediasource_101", None, "/media/movie.mkv"),
                source("mediasource_102", None, "/media/movie.mkv"),
            ],
            "/media/movie.mkv",
            "mediasource_102",
        ),
        (
            vec![source("native", Some("102"), "/media/%6dovie.mkv")],
            "file:///media/movie.mkv",
            "native",
        ),
    ] {
        assert_eq!(
            item_streams(&item(sources), &[target]).unwrap()[0]["Title"],
            selected
        );
    }
}
