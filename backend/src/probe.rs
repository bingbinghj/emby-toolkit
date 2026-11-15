use crate::{media_edit::list, probe_display, state::string};
use serde_json::{Value, json};
fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .filter(|n| n.is_finite())
        .unwrap_or(0.0)
}
fn integer(value: &Value) -> i64 {
    number(value) as i64
}
fn rate(value: &Value) -> Value {
    let text = value.as_str().unwrap_or("");
    let value = if let Some((a, b)) = text.split_once('/') {
        let b = b.parse::<f64>().unwrap_or(0.0);
        if b == 0.0 {
            return Value::Null;
        }
        a.parse::<f64>().unwrap_or(0.0) / b
    } else {
        number(value)
    };
    if value == 0.0 {
        Value::Null
    } else {
        json!(value)
    }
}
fn text(data: &Value, field: &str, default: &str) -> String {
    data[field]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(default)
        .to_string()
}

// Emby 4.10.2 computes these readonly fields from the imported enum values.
pub(crate) fn native_video_range_fields(
    extended_type: &str,
    extended_subtype: &str,
) -> Option<(&'static str, &'static str)> {
    let range = match extended_type {
        "None" => "SDR",
        "Hdr10" => "HDR 10",
        "Hdr10Plus" => "HDR 10+",
        "HyperLogGamma" => "HLG",
        "DolbyVision" => "DolbyVision",
        _ => return None,
    };
    let description = match extended_subtype {
        "None" => "None",
        "Hdr10" => "HDR 10",
        "Hdr10Plus0" => "HDR 10+",
        "HyperLogGamma" => "HLG",
        "DoviProfile02" => "Profile 0.2 (SDR)",
        "DoviProfile10" => "Profile 1.0",
        "DoviProfile22" => "Profile 2.2 (SDR)",
        "DoviProfile30" => "Profile 3.0",
        "DoviProfile42" => "Profile 4.2 (SDR)",
        "DoviProfile50" => "Profile 5.0",
        "DoviProfile61" => "Profile 6.1 (HDR10 compatible)",
        "DoviProfile76" => "Profile 7.6 (Bluray)",
        "DoviProfile81" => "Profile 8.1 (HDR10 compatible)",
        "DoviProfile82" => "Profile 8.2 (SDR)",
        "DoviProfile83" => "Profile 8.3",
        "DoviProfile84" => "Profile 8.4 (HLG compatible)",
        "DoviProfile85" => "Profile 8.5",
        "DoviProfile92" => "Profile 9.2 (SDR)",
        _ => return None,
    };
    Some((range, description))
}

pub(crate) fn native_internal_subtitle_fields(codec: &str, index: i64) -> (bool, &'static str) {
    let codec = codec.to_lowercase();
    let text = !codec.is_empty()
        && !["dvb_teletext", "dvb_subtitle", "dvbsub", "dvd_subtitle", "dvdsub",
            "hdmv_pgs_subtitle", "pgssub", "xsub", "sub", "sup"].contains(&codec.as_str())
        && !codec.contains("pgs") && !codec.contains("dvbsub");
    (text, if index >= 100 { "VideoSideData" } else { "InternalStream" })
}

pub fn build(probe: &Value, node: &Value, mappings: &Value) -> Option<Value> {
    if probe.as_object().is_none_or(|v| v.is_empty()) {
        return None;
    }
    let filename = ["fn", "n", "file_name", "original_name"]
        .iter()
        .map(|k| string(node, k))
        .find(|s| !s.is_empty())
        .unwrap_or("unknown".into());
    let extension = std::path::Path::new(&filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let format = &probe["format"];
    let size = [&node["fs"], &node["size"], &format["size"]]
        .iter()
        .map(|v| integer(v))
        .find(|n| *n != 0)
        .unwrap_or(0);
    let duration = number(&format["duration"]);
    let mut bitrate = integer(&format["bit_rate"]);
    if bitrate == 0 && size != 0 && duration > 0.0 {
        bitrate = (size as f64 * 8.0 / duration) as i64;
    }
    let container = if extension.is_empty() || extension == "strm" {
        let raw = string(format, "format_name").to_lowercase();
        if raw.contains("matroska") {
            "mkv".into()
        } else if raw.contains("mov") || raw.contains("mp4") {
            "mp4".into()
        } else {
            raw.split(',').next().unwrap_or("").to_string()
        }
    } else {
        extension
    };
    let chapters=list(&probe["chapters"]).iter().enumerate().map(|(i,c)|json!({"Name":c["tags"]["title"].as_str().filter(|s|!s.is_empty()).map(str::to_string).unwrap_or(format!("章节 {}",i+1)),"MarkerType":"Chapter","ChapterIndex":i,"StartPositionTicks":(number(&c["start_time"])*10000000.0)as i64})).collect::<Vec<_>>();
    let mut streams = vec![];
    for raw in list(&probe["streams"]) {
        let kind = string(&raw, "codec_type").to_lowercase();
        let codec = string(&raw, "codec_name").to_lowercase();
        let tags = &raw["tags"];
        let forced = integer(&raw["disposition"]["forced"]) != 0;
        let default = integer(&raw["disposition"]["default"]) != 0;
        let index = raw
            .get("index")
            .map(integer)
            .unwrap_or(streams.len() as i64);
        let mut stream = json!({"Codec":codec,"Index":index,"IsForced":forced,"Protocol":"File","TimeBase":text(&raw,"time_base","1/1000"),"IsDefault":default,"IsExternal":false,"IsInterlaced":false,"AttachmentSize":0,"ExtendedVideoType":"None","IsHearingImpaired":false,"ExtendedVideoSubType":"None","IsTextSubtitleStream":false,"SupportsExternalStream":false,"ExtendedVideoSubTypeDescription":"None"});
        match kind.as_str() {
            "video" => {
                let width = integer(&raw["width"]);
                let height = integer(&raw["height"]);
                let pixel = string(&raw, "pix_fmt").to_lowercase();
                let mut depth = integer(&raw["bits_per_raw_sample"]);
                if depth == 0 {
                    depth = integer(&raw["bits_per_sample"]);
                }
                if depth == 0 && !pixel.is_empty() {
                    depth = if pixel.contains("12") {
                        12
                    } else if pixel.contains("10") {
                        10
                    } else {
                        8
                    };
                }
                let side = list(&raw["side_data_list"]);
                let dv = side.iter().find(|s| {
                    let t = string(s, "side_data_type").to_uppercase();
                    t.contains("DOVI") || t.contains("DOLBY VISION")
                });
                let profile = dv.map(|s| s.get("dv_profile").unwrap_or(&s["profile"]));
                let compatibility = dv.map(|s| {
                    s.get("dv_bl_signal_compatibility_id")
                        .unwrap_or(&s["bl_signal_compatibility_id"])
                });
                let transfer = string(&raw, "color_transfer");
                let primaries = string(&raw, "color_primaries");
                let (extended, subtype) = if dv.is_some() {
                    let profile_integer = |value: &Value| value.as_i64().or_else(|| value.as_str()?.parse().ok());
                    let subtype = match (
                        profile.and_then(profile_integer),
                        compatibility.and_then(profile_integer),
                    ) {
                        (Some(0), Some(2)) => "DoviProfile02",
                        (Some(1), Some(0)) => "DoviProfile10",
                        (Some(2), Some(2)) => "DoviProfile22",
                        (Some(3), Some(0)) => "DoviProfile30",
                        (Some(4), Some(2)) => "DoviProfile42",
                        (Some(5), _) => "DoviProfile50",
                        (Some(6), Some(1)) => "DoviProfile61",
                        (Some(7), _) => "DoviProfile76",
                        (Some(8), Some(1)) => "DoviProfile81",
                        (Some(8), Some(2)) => "DoviProfile82",
                        (Some(8), Some(3)) => "DoviProfile83",
                        (Some(8), Some(4)) => "DoviProfile84",
                        (Some(8), Some(5)) => "DoviProfile85",
                        (Some(9), Some(2)) => "DoviProfile92",
                        _ => "None",
                    };
                    ("DolbyVision", subtype)
                } else if raw["side_data_list"].to_string().contains("HDR10+")
                    || raw["side_data_list"].to_string().contains("SMPTE2094-40")
                {
                    ("Hdr10Plus", "Hdr10Plus0")
                } else if transfer == "smpte2084" {
                    ("Hdr10", "Hdr10")
                } else if transfer == "arib-std-b67" {
                    ("HyperLogGamma", "HyperLogGamma")
                } else {
                    ("None", "None")
                };
                let (video_range, description) = native_video_range_fields(extended, subtype)?;
                let resolution = if width >= 3800 {
                    "4K".into()
                } else if width >= 1900 {
                    "1080p".into()
                } else if width >= 1200 {
                    "720p".into()
                } else if height != 0 {
                    format!("{height}p")
                } else {
                    String::new()
                };
                let effect = match extended {
                    "DolbyVision" => "Dolby Vision",
                    "Hdr10" => "HDR10",
                    "Hdr10Plus" => "HDR10+",
                    "HyperLogGamma" => "HLG",
                    _ => "",
                };
                let fps = rate(&raw["avg_frame_rate"]);
                let fps = if fps.is_null() {
                    rate(&raw["r_frame_rate"])
                } else {
                    fps
                };
                let fields = json!({"Type":"Video","Level":integer(&raw["level"]),"Title":text(tags,"title",""),"Width":width,"Height":height,"BitRate":if integer(&raw["bit_rate"])==0{bitrate}else{integer(&raw["bit_rate"])},"Profile":text(&raw,"profile",""),"BitDepth":depth,"RefFrames":raw.get("refs").map(integer).unwrap_or(1),"ColorSpace":raw["colorspace"].as_str().or_else(||raw["color_space"].as_str()).unwrap_or(""),"VideoRange":video_range,"AspectRatio":if width!=0&&height!=0{format!("{width}:{height}")}else{String::new()},"PixelFormat":pixel,"DisplayTitle":([resolution,effect.into(),codec.to_uppercase()].iter().filter(|s|!s.is_empty()).cloned().collect::<Vec<_>>().join(" ")),"IsAnamorphic":false,"ColorTransfer":transfer,"RealFrameRate":fps,"AverageFrameRate":fps,"ColorPrimaries":primaries,"ExtendedVideoType":extended,"ExtendedVideoSubType":subtype,"ExtendedVideoSubTypeDescription":description});
                stream
                    .as_object_mut()
                    .unwrap()
                    .extend(fields.as_object().unwrap().clone());
            }
            "audio" | "subtitle" => {
                let capital = if kind == "audio" { "Audio" } else { "Subtitle" };
                let (lang, display, _) = probe_display::friendly(
                    tags["language"].as_str().unwrap_or(""),
                    tags["title"].as_str().unwrap_or(""),
                    capital,
                    tags["DisplayTitle"].as_str().unwrap_or(""),
                    tags["IsHearingImpaired"] == true,
                    mappings,
                );
                let title = text(tags, "title", "");
                stream["Type"] = json!(capital);
                stream["Title"] = json!(title);
                stream["Language"] = if kind == "subtitle" {
                    json!(tags["language"].as_str().unwrap_or(""))
                } else {
                    json!(lang)
                };
                stream["DisplayLanguage"] = json!(display);
                let mut display_parts = if display.is_empty() || display == "未知" {
                    vec![]
                } else {
                    vec![display]
                };
                if kind == "audio" {
                    let channels = integer(&raw["channels"]);
                    let layout = match channels {
                        8 => "7.1".into(),
                        7 => "6.1".into(),
                        6 => "5.1".into(),
                        2 => "stereo".into(),
                        1 => "mono".into(),
                        _ => {
                            if string(&raw, "channel_layout").is_empty() {
                                if channels != 0 {
                                    channels.to_string()
                                } else {
                                    String::new()
                                }
                            } else {
                                string(&raw, "channel_layout").replace("(side)", "")
                            }
                        }
                    };
                    let profile = string(&raw, "profile");
                    let mix = format!("{profile} {title}").to_lowercase();
                    let codec_label = match codec.as_str() {
                        "truehd" => {
                            if mix.contains("atmos") {
                                "TRUEHD Atmos"
                            } else {
                                "TRUEHD"
                            }
                        }
                        "eac3" => {
                            if mix.contains("atmos") {
                                "DDP Atmos"
                            } else {
                                "DDP"
                            }
                        }
                        "dts" => {
                            if mix.contains("ma") || mix.contains("master") || mix.contains("xll") {
                                "DTS-HD MA"
                            } else if mix.contains("hra") || mix.contains("high resolution") {
                                "DTS-HD HRA"
                            } else {
                                "DTS"
                            }
                        }
                        _ => "",
                    };
                    let codec_label = if codec_label.is_empty() {
                        codec.to_uppercase()
                    } else {
                        codec_label.into()
                    };
                    for part in [&codec_label, &layout] {
                        if !part.is_empty() {
                            display_parts.push(part.clone());
                        }
                    }
                    if default {
                        display_parts.push("(默认)".into());
                    }
                    stream.as_object_mut().unwrap().extend(json!({"BitRate":integer(&raw["bit_rate"]),"BitDepth":if integer(&raw["bits_per_raw_sample"])==0{integer(&raw["bits_per_sample"])}else{integer(&raw["bits_per_raw_sample"])},"Channels":channels,"SampleRate":integer(&raw["sample_rate"]),"ChannelLayout":layout,"Profile":if profile.is_empty(){codec_label}else{profile}}).as_object().unwrap().clone());
                } else {
                    let subtitle = match codec.as_str() {
                        "hdmv_pgs_subtitle" | "pgssub" => "PGSSUB",
                        "subrip" | "srt" => "SUBRIP",
                        "ass" => "ASS",
                        "ssa" => "SSA",
                        "webvtt" => "VTT",
                        "mov_text" => "MOV_TEXT",
                        "dvd_subtitle" => "DVDSUB",
                        _ => "",
                    };
                    let subtitle = if subtitle.is_empty() {
                        codec.to_uppercase()
                    } else {
                        subtitle.into()
                    };
                    stream["Codec"] = json!(subtitle);
                    display_parts.push(if default {
                        format!("(默认 {subtitle})")
                    } else {
                        format!("({subtitle})")
                    });
                    // Match Emby's readonly getters, including less common
                    // text formats and subtitles carried in video side data.
                    let (text, location) = native_internal_subtitle_fields(&codec, index);
                    stream["IsTextSubtitleStream"] = json!(text);
                    stream["SubtitleLocationType"] = json!(location);
                }
                stream["DisplayTitle"] = json!(display_parts.join(" "));
            }
            _ => continue,
        }
        streams.push(stream);
    }
    if streams.is_empty() {
        return None;
    }
    Some(
        json!([{"Chapters":chapters,"MediaSourceInfo":{"Size":size,"Type":"Default","Bitrate":bitrate,"Formats":[],"Chapters":[],"IsRemote":true,"Protocol":"File","Container":container,"MediaStreams":streams,"RunTimeTicks":if duration>0.0{(duration*10000000.0)as i64}else{0},"RequiresClosing":false,"RequiresLooping":false,"RequiresOpening":false,"SupportsProbing":true,"IsInfiniteStream":false,"HasMixedProtocols":false,"SupportsDirectPlay":true,"RequiredHttpHeaders":{},"SupportsTranscoding":true,"SupportsDirectStream":true,"ReadAtNativeFramerate":false,"AddApiKeyToDirectStreamUrl":false}}]),
    )
}
