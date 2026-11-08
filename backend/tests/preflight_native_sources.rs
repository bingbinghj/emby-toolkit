use emby_toolkit::media_preflight;
use serde_json::json;

#[test]
fn another_versions_video_does_not_pass_current_item_preflight() {
    let mut item = json!({"Id":"102","Path":"/media/102.strm","MediaStreams":[],"MediaSources":[
        {"ItemId":"101","Path":"/cd2/101.mkv","MediaStreams":[{"Type":"Video","Width":1920,"Height":1080}]},
        {"ItemId":"102","Path":"/cd2/102.mkv","MediaStreams":[]}
    ]});
    assert!(!media_preflight::valid_video(&item));
    item["MediaSources"][1]["MediaStreams"] = json!([{"Type":"Video","Width":3840,"Height":2160}]);
    assert!(media_preflight::valid_video(&item));
}

#[test]
fn video_preflight_supports_verified_legacy_ids_without_borrowing_conflicting_owners() {
    let mut item = json!({"Id":"102","Path":"/media/102.strm","MediaSources":[
        {"Id":"mediasource_102","Path":"/cd2/102.mkv","MediaStreams":[{"Type":"Video","Width":1920,"Height":1080}]}
    ]});
    assert!(media_preflight::valid_video(&item));
    item["MediaSources"][0]["ItemId"] = json!("101");
    item["MediaSources"][0]["Path"] = item["Path"].clone();
    assert!(!media_preflight::valid_video(&item));
}

#[test]
fn ambiguous_sources_do_not_pass_preflight_but_native_single_streams_still_work() {
    let mut item = json!({"Id":"102","Path":"/media/102.strm","MediaStreams":[{"Type":"Video","Width":1920,"Height":1080}],"MediaSources":[
        {"Id":"opaque-a","Path":"/media/102.strm","MediaStreams":[{"Type":"Video","Width":1920,"Height":1080}]},
        {"Id":"opaque-b","Path":"/media/102.strm","MediaStreams":[{"Type":"Video","Width":3840,"Height":2160}]}
    ]});
    assert!(!media_preflight::valid_video(&item));
    item["MediaSources"] = json!([]);
    assert!(media_preflight::valid_video(&item));
    item["MediaStreams"][0]["Width"] = json!(0);
    assert!(!media_preflight::valid_video(&item));
}
