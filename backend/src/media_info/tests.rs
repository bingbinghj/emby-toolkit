use super::*;
use serde_json::json;

fn source(id: &str, owner: Option<&str>, path: &str) -> Value {
    json!({"Id":id,"ItemId":owner,"Path":path,"MediaStreams":[{"Type":"Audio","Title":id}]})
}

fn item(sources: Vec<Value>) -> Value {
    json!({"Id":"102","MediaSources":sources})
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
