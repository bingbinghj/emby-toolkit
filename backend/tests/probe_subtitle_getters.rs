use emby_toolkit::probe;
use serde_json::{Value, json};

fn subtitle(codec: &str, index: i32) -> Value {
    let info = probe::build(
        &json!({
            "format":{"duration":"100","format_name":"matroska","size":"1000"},
            "streams":[
                {"codec_type":"video","codec_name":"h264","width":1920,"height":1080},
                {"codec_type":"subtitle","codec_name":codec,"index":index,
                    "tags":{"language":"eng","title":"Original subtitle"}}
            ]
        }),
        &json!({"fn":"example.mkv"}),
        &json!([]),
    )
    .unwrap();
    info[0]["MediaSourceInfo"]["MediaStreams"][1].clone()
}

#[test]
fn readonly_text_classification_matches_the_native_graphic_codec_contract() {
    // MediaStream.IsTextFormat in Emby 4.10.2 excludes graphic formats; it
    // also accepts text codecs beyond the common ASS/SRT formats.
    for (codec, expected) in [
        ("subrip", true),
        ("ass", true),
        ("ttml", true),
        ("sami", true),
        ("microdvd", true),
        ("mov_text", true),
        ("", false),
        ("dvb_teletext", false),
        ("dvb_subtitle", false),
        ("dvbsub", false),
        ("dvd_subtitle", false),
        ("dvdsub", false),
        ("hdmv_pgs_subtitle", false),
        ("pgssub", false),
        ("xsub", false),
        ("sub", false),
        ("sup", false),
        ("image_pgs_stream", false),
        ("image_dvbsub_stream", false),
    ] {
        for variant in [codec.to_string(), codec.to_uppercase()] {
            let stream = subtitle(&variant, 1);
            assert_eq!(stream["IsTextSubtitleStream"], expected, "{variant}");
            assert_eq!(stream["IsExternal"], false);
            assert_eq!(stream["Title"], "Original subtitle");
        }
    }
}

#[test]
fn readonly_subtitle_location_matches_the_native_internal_index_boundary() {
    // The native getter describes index 100+ as video side data, including
    // graphic subtitles. The probe still produces internal streams only.
    for codec in ["ass", "hdmv_pgs_subtitle"] {
        for (index, expected) in [
            (-1, "InternalStream"),
            (99, "InternalStream"),
            (100, "VideoSideData"),
            (123, "VideoSideData"),
        ] {
            let stream = subtitle(codec, index);
            assert_eq!(stream["Index"], index);
            assert_eq!(stream["IsExternal"], false);
            assert_eq!(stream["SubtitleLocationType"], expected, "{codec} #{index}");
        }
    }
}
