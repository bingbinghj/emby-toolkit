use emby_toolkit::probe;
use serde_json::{Value, json};

fn video(transfer: &str, side: Value) -> Value {
    let raw = json!({"format":{"size":"1000","duration":"2","format_name":"matroska"},
        "streams":[{"codec_type":"video","codec_name":"hevc","index":0,
            "width":1920,"height":1080,"pix_fmt":"yuv420p10le",
            "color_primaries":"bt2020","color_transfer":transfer,"side_data_list":side}]});
    probe::build(&raw, &json!({"fn":"episode.mkv"}), &json!([])).unwrap()[0]
        ["MediaSourceInfo"]["MediaStreams"][0]
        .clone()
}

// Values come from the pinned Emby 4.10.2 MediaStream getters and enums.
// VideoRange and ExtendedVideoSubTypeDescription have no native setters.
#[test]
fn ordinary_video_ranges_match_the_native_writable_enums_and_readonly_getters() {
    for (transfer, side, kind, subtype, range, description) in [
        ("bt709", json!([]), "None", "None", "SDR", "None"),
        ("smpte2084", json!([]), "Hdr10", "Hdr10", "HDR 10", "HDR 10"),
        (
            "smpte2084",
            json!([{"side_data_type":"SMPTE2094-40"}]),
            "Hdr10Plus",
            "Hdr10Plus0",
            "HDR 10+",
            "HDR 10+",
        ),
        (
            "arib-std-b67",
            json!([]),
            "HyperLogGamma",
            "HyperLogGamma",
            "HLG",
            "HLG",
        ),
    ] {
        let stream = video(transfer, side);
        assert_eq!(stream["ExtendedVideoType"], kind);
        assert_eq!(stream["ExtendedVideoSubType"], subtype);
        assert_eq!(stream["VideoRange"], range);
        assert_eq!(stream["ExtendedVideoSubTypeDescription"], description);
        if kind == "None" {
            assert!(!stream["DisplayTitle"].as_str().unwrap().contains("SDR"));
        }
    }
}

#[test]
fn dolby_vision_profiles_use_native_enum_names_and_getter_descriptions() {
    for (profile, compatibility, subtype, description) in [
        (5, 0, "DoviProfile50", "Profile 5.0"),
        (7, 6, "DoviProfile76", "Profile 7.6 (Bluray)"),
        (8, 1, "DoviProfile81", "Profile 8.1 (HDR10 compatible)"),
        (8, 2, "DoviProfile82", "Profile 8.2 (SDR)"),
        (8, 4, "DoviProfile84", "Profile 8.4 (HLG compatible)"),
        (99, 1, "None", "None"),
    ] {
        let stream = video(
            "smpte2084",
            json!([{"side_data_type":"DOVI configuration record",
            "dv_profile":profile,"dv_bl_signal_compatibility_id":compatibility}]),
        );
        assert_eq!(stream["ExtendedVideoType"], "DolbyVision");
        assert_eq!(stream["ExtendedVideoSubType"], subtype);
        assert_eq!(stream["ExtendedVideoSubTypeDescription"], description);
        assert_eq!(stream["VideoRange"], "DolbyVision");
        assert!(
            stream["DisplayTitle"]
                .as_str()
                .unwrap()
                .contains("Dolby Vision")
        );
    }
}

#[test]
fn dolby_vision_integer_strings_preserve_the_same_native_profile() {
    let numeric = video(
        "smpte2084",
        json!([{"side_data_type":"DOVI configuration record",
        "dv_profile":8,"dv_bl_signal_compatibility_id":1}]),
    );
    let strings = video(
        "smpte2084",
        json!([{"side_data_type":"DOVI configuration record",
        "dv_profile":"8","dv_bl_signal_compatibility_id":"1"}]),
    );
    assert_eq!(strings, numeric);
    let unknown = video(
        "smpte2084",
        json!([{"side_data_type":"DOVI configuration record",
        "dv_profile":"invalid","dv_bl_signal_compatibility_id":"2"}]),
    );
    assert_eq!(unknown["ExtendedVideoType"], "DolbyVision");
    assert_eq!(
        unknown["ExtendedVideoSubType"], "None",
        "invalid numbers must not be coerced into a known profile"
    );
}
