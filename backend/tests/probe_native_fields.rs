use emby_toolkit::probe;
use serde_json::{Value, json};

fn build(stream: Value) -> Option<Value> {
    probe::build(
        &json!({"format":{"duration":"2","size":"1000","format_name":"matroska"},
            "streams":[{"codec_type":"video","codec_name":"h264","index":0,"width":16,"height":16},stream]}),
        &json!({"fn":"fixture.mkv"}),
        &json!([]),
    )
}

#[test]
fn hearing_disposition_is_preserved_without_trusting_display_hints() {
    for kind in ["audio", "subtitle"] {
        for (disposition, hint, expected) in [
            (json!(1), false, true),
            (json!("1"), false, true),
            (json!(0), true, false),
            (Value::Null, true, false),
        ] {
            let info = build(json!({"codec_type":kind,"codec_name":if kind == "audio" {"aac"} else {"subrip"},"index":1,
                "channels":2,"sample_rate":"48000","tags":{"language":"eng","title":"Original track","IsHearingImpaired":hint},
                "disposition":{"hearing_impaired":disposition,"default":1,"forced":1}})).unwrap();
            let stream = &info[0]["MediaSourceInfo"]["MediaStreams"][1];
            assert_eq!(
                stream["IsHearingImpaired"], expected,
                "{kind} {disposition}"
            );
            assert_eq!(stream["IsDefault"], true);
            assert_eq!(stream["IsForced"], true);
            assert_eq!(stream["Title"], "Original track");
        }
    }
}

#[test]
fn audio_layout_keeps_the_explicit_channel_arrangement_before_count_fallback() {
    for (channels, layout, expected) in [
        (6, Some("6.0"), "6.0"),
        (6, Some("5.1(side)"), "5.1"),
        (8, Some("7.1(wide)"), "7.1"),
        (2, Some(" stereo (front) "), "stereo"),
        (6, None, "5.1"),
        (2, Some(""), "stereo"),
    ] {
        let info = build(json!({"codec_type":"audio","codec_name":"aac","index":1,
            "channels":channels,"channel_layout":layout,"sample_rate":"48000"}))
        .unwrap();
        let stream = &info[0]["MediaSourceInfo"]["MediaStreams"][1];
        assert_eq!(stream["Channels"], channels);
        assert_eq!(stream["ChannelLayout"], expected);
    }
}

#[test]
fn attachment_metadata_keeps_standard_ffprobe_and_native_probe_sizes() {
    for (field, size, filename, expected_codec) in [
        ("extradata_size", 759_720, "DejaVuSans.ttf", "ttf"),
        ("attachment_size", 12, "Font.OTF", "otf"),
    ] {
        let mut attachment = json!({"codec_type":"attachment","codec_name":"ttf","index":1,
            "tags":{"filename":filename,"mimetype":"application/x-truetype-font"}});
        attachment[field] = json!(size);
        let info = build(attachment).unwrap();
        let streams = info[0]["MediaSourceInfo"]["MediaStreams"]
            .as_array()
            .unwrap();
        assert_eq!(streams.len(), 2, "font metadata must not be discarded");
        let stream = &streams[1];
        assert_eq!(stream["Type"], "Attachment");
        assert_eq!(stream["AttachmentSize"], size);
        assert_eq!(stream["Path"], filename);
        assert_eq!(stream["MimeType"], "application/x-truetype-font");
        assert_eq!(stream["Codec"], expected_codec);
        assert_eq!(stream["IsExternal"], false);
    }
    let info = build(json!({"codec_type":"attachment","codec_name":"ttf","index":1})).unwrap();
    let stream = &info[0]["MediaSourceInfo"]["MediaStreams"][1];
    for field in ["AttachmentSize", "Path", "MimeType", "Title"] {
        assert!(
            stream[field].is_null(),
            "missing {field} must remain nullable"
        );
    }
}

#[test]
fn contradictory_or_unrepresentable_attachment_metadata_is_not_silently_dropped() {
    for attachment in [
        json!({"codec_type":"attachment","attachment_size":12,"extradata_size":13}),
        json!({"codec_type":"attachment","extradata_size":2_147_483_648_i64}),
        json!({"codec_type":"attachment","extradata_size":12.5}),
        json!({"codec_type":"attachment","tags":{"filename":123}}),
    ] {
        assert!(build(attachment.clone()).is_none(), "{attachment}");
    }
}

#[test]
fn internal_subtitle_capability_matches_the_native_stream_normalizer() {
    for (codec, expected) in [
        ("subrip", true),
        ("ass", true),
        ("ttml", true),
        ("hdmv_pgs_subtitle", false),
    ] {
        let info = build(json!({"codec_type":"subtitle","codec_name":codec,"index":1})).unwrap();
        let stream = &info[0]["MediaSourceInfo"]["MediaStreams"][1];
        assert_eq!(stream["IsTextSubtitleStream"], expected);
        assert_eq!(stream["SupportsExternalStream"], expected);
        assert_eq!(stream["IsExternal"], false);
    }
}

#[test]
fn video_aspect_uses_display_and_sample_ratios_for_anamorphic_pixels() {
    for (width, height, sample, display, aspect, anamorphic) in [
        (720, 480, Some("32:27"), Some("16:9"), "16:9", true),
        (720, 480, Some("32:27"), None, "16:9", true),
        (1440, 1080, Some("4:3"), Some("16:9"), "16:9", true),
        (3840, 2160, Some("1:1"), Some("16:9"), "16:9", false),
        (1920, 1080, Some("0:1"), None, "1920:1080", false),
        (1920, 1080, None, None, "1920:1080", false),
    ] {
        let info = build(json!({"codec_type":"video","codec_name":"h264","index":1,
            "width":width,"height":height,"sample_aspect_ratio":sample,"display_aspect_ratio":display})).unwrap();
        let stream = &info[0]["MediaSourceInfo"]["MediaStreams"][1];
        assert_eq!(stream["AspectRatio"], aspect);
        assert_eq!(stream["IsAnamorphic"], anamorphic);
        assert_eq!(stream["Width"], width);
        assert_eq!(stream["Height"], height);
    }
}

#[test]
fn video_interlacing_requires_an_explicit_interlaced_field_order() {
    for (order, interlaced) in [
        ("tt", true),
        ("bb", true),
        ("tb", true),
        ("bt", true),
        ("progressive", false),
        ("unknown", false),
        ("", false),
    ] {
        let info = build(json!({"codec_type":"video","codec_name":"h264","index":1,
            "width":720,"height":480,"field_order":order}))
        .unwrap();
        assert_eq!(
            info[0]["MediaSourceInfo"]["MediaStreams"][1]["IsInterlaced"], interlaced,
            "{order}"
        );
    }
}

#[test]
fn reference_frames_are_nullable_when_the_probe_does_not_report_a_valid_count() {
    for (raw, expected) in [
        (json!(4), json!(4)),
        (json!("2"), json!(2)),
        (Value::Null, Value::Null),
        (json!(0), Value::Null),
        (json!(-1), Value::Null),
        (json!(2.5), Value::Null),
        (json!(2_147_483_648_i64), Value::Null),
    ] {
        let info = build(json!({"codec_type":"video","codec_name":"h264","index":1,
            "width":16,"height":16,"refs":raw}))
        .unwrap();
        assert_eq!(
            info[0]["MediaSourceInfo"]["MediaStreams"][1]["RefFrames"], expected,
            "{raw}"
        );
    }
}

#[test]
fn graphical_subtitle_dimensions_are_preserved_without_guessing_text_subtitle_dimensions() {
    let info = build(
        json!({"codec_type":"subtitle","codec_name":"hdmv_pgs_subtitle","index":1,
        "width":1920,"height":1080}),
    )
    .unwrap();
    let stream = &info[0]["MediaSourceInfo"]["MediaStreams"][1];
    assert_eq!(stream["Width"], 1920);
    assert_eq!(stream["Height"], 1080);
    assert_eq!(stream["IsTextSubtitleStream"], false);
    let info = build(json!({"codec_type":"subtitle","codec_name":"subrip","index":1})).unwrap();
    let stream = &info[0]["MediaSourceInfo"]["MediaStreams"][1];
    assert!(stream["Width"].is_null());
    assert!(stream["Height"].is_null());
    assert_eq!(stream["IsTextSubtitleStream"], true);
}
