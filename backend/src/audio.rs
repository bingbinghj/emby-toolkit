use crate::{language, media_edit::list, media_info, state::string};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn text(v: &Value, k: &str) -> String {
    string(v, k).trim().to_string()
}
fn number(v: &Value, k: &str) -> i64 {
    v[k].as_f64()
        .or_else(|| v[k].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0.0) as i64
}
fn signature(v: &Value) -> Vec<String> {
    vec![
        text(v, "Language").to_lowercase(),
        text(v, "Title").to_lowercase(),
    ]
}
fn output_signature(v: &Value) -> Vec<String> {
    let mut s = signature(v);
    s.push(text(v, "DisplayLanguage").to_lowercase());
    s
}
fn has_metadata(v: &Value) -> bool {
    signature(v).iter().any(|s| !s.is_empty())
}
fn valid(v: &Value) -> bool {
    v["Type"] == "Audio"
        && (has_metadata(v) || number(v, "Channels") != 0 || number(v, "SampleRate") != 0)
}
fn truehd(parent: &Value, core: &Value, incomplete: bool) -> bool {
    text(parent, "Codec").eq_ignore_ascii_case("truehd")
        && text(core, "Codec").eq_ignore_ascii_case("ac3")
        && (number(core, "Channels") == 6 || (incomplete && number(core, "Channels") == 0))
        && (number(core, "SampleRate") == 48000 || (incomplete && number(core, "SampleRate") == 0))
        && ([448000, 640000].contains(&number(core, "BitRate"))
            || (incomplete && number(core, "BitRate") == 0))
}
fn dts(v: &Value) -> String {
    let raw = format!("{} {}", text(v, "Profile"), text(v, "DisplayTitle")).to_lowercase();
    let raw = regex::Regex::new(r"[a-z0-9]+")
        .unwrap()
        .find_iter(&raw)
        .map(|m| m.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    if !raw.contains("dts") {
        return String::new();
    }
    if raw.contains("xll") || raw.contains("master audio") || raw.contains("dts hd ma") {
        "ma"
    } else if raw.contains("high resolution") || raw.contains("dts hd hra") {
        "hra"
    } else if raw.contains("dts hd") {
        "hd"
    } else {
        ""
    }
    .into()
}
pub fn compatible(parent: &Value, core: &Value, incomplete: bool) -> bool {
    truehd(parent, core, incomplete)
        || (text(parent, "Codec").eq_ignore_ascii_case("dts")
            && text(core, "Codec").eq_ignore_ascii_case("dts")
            && !dts(parent).is_empty()
            && dts(core).is_empty()
            && [0, 48000].contains(&number(core, "SampleRate"))
            && [0, 768000, 1509000, 1536000].contains(&number(core, "BitRate")))
}
fn report(reason: &str, ambiguous: bool, pairs: Option<usize>) -> Value {
    let mut r = json!({"safe":false,"ambiguous":ambiguous,"reason":reason});
    if let Some(pairs) = pairs {
        r["pair_count"] = json!(pairs);
    }
    r
}
pub fn metadata(info: &Value) -> Value {
    let Some(streams) = media_info::container(info).and_then(|c| c["MediaStreams"].as_array())
    else {
        return report("媒体信息缺少 MediaStreams", false, None);
    };
    let audio = streams
        .iter()
        .enumerate()
        .filter(|(_, s)| valid(s))
        .collect::<Vec<_>>();
    let pairs = audio
        .windows(2)
        .enumerate()
        .filter(|(_, w)| truehd(w[0].1, w[1].1, false))
        .map(|(i, _)| (i, i + 1))
        .collect::<Vec<_>>();
    if pairs.is_empty() {
        return report("未发现 TrueHD 后紧邻的 AC3 兼容音轨", false, None);
    }
    if !pairs.iter().any(|(p, c)| {
        !text(audio[*p].1, "Language").is_empty()
            && !text(audio[*c].1, "Language").is_empty()
            && !text(audio[*p].1, "Language").eq_ignore_ascii_case(&text(audio[*c].1, "Language"))
    }) {
        return report(
            "TrueHD 与 AC3 未出现语言冲突，不视为错位",
            false,
            Some(pairs.len()),
        );
    }
    let tail = audio
        .iter()
        .rev()
        .take_while(|(_, s)| !has_metadata(s))
        .count();
    let empties = audio
        .iter()
        .enumerate()
        .filter(|(_, (_, s))| !has_metadata(s))
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    if tail == 0 || empties != (audio.len() - tail..audio.len()).collect::<Vec<_>>() {
        return report(
            "空语言/标题槽位未形成尾部闭合，不视为错位",
            false,
            Some(pairs.len()),
        );
    }
    if tail > pairs.len() {
        return report("尾部空槽多于 AC3 兼容音轨数量", false, Some(pairs.len()));
    }
    let duplicate = pairs.len() - tail;
    let removable = pairs
        .iter()
        .filter(|(p, c)| {
            has_metadata(audio[*c].1)
                && (signature(audio[*p].1) == signature(audio[*c].1)
                    || (audio.get(*c + 1).is_some_and(|(_, s)| {
                        text(s, "Codec").eq_ignore_ascii_case("truehd")
                            && signature(s) == signature(audio[*c].1)
                    })))
        })
        .map(|(_, c)| *c)
        .collect::<Vec<_>>();
    if duplicate > removable.len() || removable.len() > 10 {
        return report(
            "重复元数据数量无法与 AC3 兼容音轨精确闭合",
            removable.len() > 10,
            Some(pairs.len()),
        );
    }
    let mut choices = BTreeMap::new();
    for mask in 0u32..1 << removable.len() {
        if mask.count_ones() as usize != duplicate {
            continue;
        }
        let removed = removable
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, p)| *p)
            .collect::<Vec<_>>();
        let sources = audio
            .iter()
            .enumerate()
            .filter(|(i, (_, s))| !removed.contains(i) && has_metadata(s))
            .map(|(_, (_, s))| (*s).clone())
            .collect::<Vec<_>>();
        let sig = sources.iter().map(output_signature).collect::<Vec<_>>();
        choices.entry(sig).or_insert((removed, sources));
    }
    if choices.len() != 1 {
        return report("存在多种不同的元数据纠正结果", true, Some(pairs.len()));
    }
    let (removed, sources) = choices.into_values().next().unwrap();
    let cores = pairs.iter().map(|(_, c)| *c).collect::<BTreeSet<_>>();
    let targets = audio
        .iter()
        .enumerate()
        .filter(|(i, _)| !cores.contains(i))
        .map(|(_, (p, _))| *p)
        .collect::<Vec<_>>();
    if sources.len() != targets.len() {
        return report(
            "纠正后的元数据数量与真实音轨数量不一致",
            false,
            Some(pairs.len()),
        );
    }
    json!({"safe":true,"ambiguous":false,"reason":"可唯一闭合","pair_count":pairs.len(),"tail_empty_count":tail,"duplicate_count":duplicate,"removed_duplicate_audio_positions":removed,"core_stream_positions":cores.iter().map(|p|audio[*p].0).collect::<Vec<_>>(),"target_stream_positions":targets,"metadata_signature":sources.iter().map(signature).collect::<Vec<_>>(),"metadata_sources":sources})
}
fn canonical(raw: &str) -> String {
    let token = raw.trim().to_lowercase().replace('_', "-");
    language::lookup(&token, &json!([]))
        .map(|v| string(&v, "value"))
        .unwrap_or(token)
}
fn metadata_matches(current: &Value, reference: &Value) -> bool {
    let expected = canonical(&text(reference, "Language"));
    if expected.is_empty() {
        return true;
    }
    let actual = canonical(&text(current, "Language"));
    if ["chi", "zho", "zh", "cmn"].contains(&expected.as_str())
        && ["chi", "zho", "zh", "cmn", "yue", "chs", "cht"].contains(&actual.as_str())
    {
        return true;
    }
    !actual.is_empty() && actual == expected
}
fn matches(current: &Value, reference: &Value, clpi: bool) -> bool {
    if !clpi && number(current, "Index") != number(reference, "Index") {
        return false;
    }
    let codec = text(current, "Codec").to_lowercase();
    if codec != text(reference, "Codec").to_lowercase() {
        return false;
    }
    if codec == "dts" {
        let a = dts(current);
        let b = dts(reference);
        if a.is_empty() != b.is_empty()
            || (!a.is_empty() && !b.is_empty() && a != "hd" && b != "hd" && a != b)
        {
            return false;
        }
    }
    ["Channels", "SampleRate", "BitRate"]
        .iter()
        .filter(|k| !clpi || **k != "Channels")
        .all(|k| {
            let a = number(current, k);
            let b = number(reference, k);
            a == 0 || b == 0 || a == b
        })
}
#[derive(Clone)]
struct Choice {
    retained: Vec<usize>,
    removed: Vec<usize>,
    score: usize,
}
fn align(
    audio: &[(usize, &Value)],
    reference: &[&Value],
    cores: &BTreeMap<usize, usize>,
    clpi: bool,
    unanchored: bool,
) -> Vec<Choice> {
    #[allow(clippy::too_many_arguments)] // Memoized Python parity algorithm keeps its inputs explicit.
    fn visit(
        a: usize,
        b: usize,
        previous: bool,
        audio: &[(usize, &Value)],
        reference: &[&Value],
        cores: &BTreeMap<usize, usize>,
        clpi: bool,
        unanchored: bool,
        cache: &mut BTreeMap<(usize, usize, bool), Vec<Choice>>,
    ) -> Vec<Choice> {
        if let Some(value) = cache.get(&(a, b, previous)) {
            return value.clone();
        }
        if audio.len() - a < reference.len() - b {
            return vec![];
        }
        if a == audio.len() {
            return if b == reference.len() {
                vec![Choice {
                    retained: vec![],
                    removed: vec![],
                    score: 0,
                }]
            } else {
                vec![]
            };
        }
        let mut choices = vec![];
        if unanchored || cores.contains_key(&a) {
            let cost = usize::from(!(previous && cores.get(&a) == a.checked_sub(1).as_ref()));
            for mut c in visit(
                a + 1,
                b,
                false,
                audio,
                reference,
                cores,
                clpi,
                unanchored,
                cache,
            ) {
                c.removed.insert(0, a);
                c.score += cost;
                choices.push(c);
            }
        }
        if b < reference.len() && matches(audio[a].1, reference[b], clpi) {
            for mut c in visit(
                a + 1,
                b + 1,
                true,
                audio,
                reference,
                cores,
                clpi,
                unanchored,
                cache,
            ) {
                c.retained.insert(0, a);
                choices.push(c);
            }
        }
        let min = choices.iter().map(|c| c.score).min().unwrap_or(0);
        choices.retain(|c| c.score == min);
        choices.truncate(2);
        cache.insert((a, b, previous), choices.clone());
        choices
    }
    visit(
        0,
        0,
        false,
        audio,
        reference,
        cores,
        clpi,
        unanchored,
        &mut BTreeMap::new(),
    )
}
fn metadata_alignment(source: &[Value], reference: &[&Value]) -> Option<Vec<Value>> {
    type Choices = BTreeMap<Vec<Vec<String>>, Vec<Value>>;
    fn visit(
        a: usize,
        b: usize,
        source: &[Value],
        reference: &[&Value],
        cache: &mut BTreeMap<(usize, usize), Choices>,
    ) -> BTreeMap<Vec<Vec<String>>, Vec<Value>> {
        if let Some(value) = cache.get(&(a, b)) {
            return value.clone();
        }
        if source.len() - a < reference.len() - b {
            return BTreeMap::new();
        }
        if b == reference.len() {
            return BTreeMap::from([(vec![], vec![])]);
        }
        let mut choices = visit(a + 1, b, source, reference, cache);
        if choices.len() < 2 && metadata_matches(&source[a], reference[b]) {
            for (mut signature, mut selected) in visit(a + 1, b + 1, source, reference, cache) {
                signature.insert(0, output_signature(&source[a]));
                selected.insert(0, source[a].clone());
                choices.entry(signature).or_insert(selected);
                if choices.len() > 1 {
                    break;
                }
            }
        }
        cache.insert((a, b), choices.clone());
        choices
    }
    let choices = visit(0, 0, source, reference, &mut BTreeMap::new());
    if choices.len() == 1 {
        choices.into_values().next()
    } else {
        None
    }
}
pub fn reference(
    info: &Value,
    reference: &Value,
    evidence: &str,
    proven: &[String],
    strict: bool,
) -> Option<Value> {
    let streams = media_info::container(info)?["MediaStreams"].as_array()?;
    let ref_streams = media_info::container(reference)?["MediaStreams"].as_array()?;
    let original = streams
        .iter()
        .enumerate()
        .filter(|(_, s)| valid(s))
        .collect::<Vec<_>>();
    let mut audio = original.clone();
    let mut ref_audio = ref_streams.iter().filter(|s| valid(s)).collect::<Vec<_>>();
    if ref_audio.is_empty() {
        return None;
    }
    let clpi = evidence.to_uppercase().contains("CLPI");
    let stn = evidence.to_uppercase().contains("MPLS/STN");
    let fail = |reason: &str| {
        let mut r = report(reason, true, None);
        r["reference_checked"] = json!(true);
        r
    };
    if !clpi {
        if audio.iter().any(|(_, s)| {
            s["Index"]
                .as_i64()
                .or_else(|| s["Index"].as_str().and_then(|s| s.parse().ok()))
                .is_none()
        }) || ref_audio.iter().any(|s| {
            s["Index"]
                .as_i64()
                .or_else(|| s["Index"].as_str().and_then(|s| s.parse().ok()))
                .is_none()
        }) {
            return Some(fail("物理流 Index 缺失或不是数字，无法进行精确探测闭合"));
        }
        audio.sort_by_key(|(_, s)| number(s, "Index"));
        ref_audio.sort_by_key(|s| number(s, "Index"));
    }
    if audio.len() == ref_audio.len() {
        if audio
            .iter()
            .zip(&ref_audio)
            .all(|((_, s), r)| matches(s, r, clpi))
        {
            let mut r = report("可信原始探测与当前音轨结构一致，无需纠正", false, None);
            r["reference_checked"] = json!(true);
            return Some(r);
        }
        return None;
    }
    if audio.len() < ref_audio.len() {
        return None;
    }
    let mut parent_by_stream = BTreeMap::new();
    for order in [&audio, &original] {
        for pair in order.windows(2) {
            let core = pair[1];
            let index = core.1["Index"]
                .as_str()
                .map(str::to_string)
                .unwrap_or(core.1["Index"].to_string());
            if compatible(pair[0].1, core.1, clpi)
                && (!(strict || !proven.is_empty()) || proven.contains(&index))
            {
                parent_by_stream.entry(core.0).or_insert(pair[0].0);
            }
        }
        if clpi {
            break;
        }
    }
    let cores = audio
        .iter()
        .enumerate()
        .filter_map(|(p, (s, _))| {
            let parent = parent_by_stream.get(s)?;
            Some((
                p,
                audio
                    .iter()
                    .position(|(candidate, _)| candidate == parent)?,
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let count = audio.len() - ref_audio.len();
    if !clpi && !stn && count > cores.len() {
        return Some(fail(
            "当前音轨多于可信原始探测，但无法全部对应到高清音轨兼容核心",
        ));
    }
    let choices = align(&audio, &ref_audio, &cores, clpi, clpi || stn);
    if choices.is_empty() {
        return Some(fail(if stn {
            "当前音轨无法与主播放列表 STN 参考唯一闭合"
        } else {
            "当前音轨无法与可信原始探测唯一闭合"
        }));
    }
    if choices.len() != 1 {
        return Some(fail(if stn {
            "主播放列表 STN 参考存在多种不同的删除结果"
        } else {
            "可信原始探测存在多种不同的 Core 删除结果"
        }));
    }
    let choice = &choices[0];
    let mut sources = choice
        .retained
        .iter()
        .map(|p| audio[*p].1.clone())
        .collect::<Vec<_>>();
    if clpi
        && !sources
            .iter()
            .zip(&ref_audio)
            .all(|(s, r)| metadata_matches(s, r))
    {
        let compact = audio
            .iter()
            .filter(|(_, s)| has_metadata(s))
            .map(|(_, s)| (*s).clone())
            .collect::<Vec<_>>();
        if let Some(aligned) = metadata_alignment(&compact, &ref_audio) {
            sources = aligned;
        } else if stn {
            sources = sources
                .iter()
                .zip(&ref_audio)
                .map(|(s, r)| {
                    if metadata_matches(s, r) {
                        s.clone()
                    } else {
                        (*r).clone()
                    }
                })
                .collect();
        } else {
            return Some(fail("可信原始探测已确定 Core，但语言元数据无法唯一对应"));
        }
    }
    let removed_cores = choice
        .removed
        .iter()
        .filter(|p| {
            p.checked_sub(1)
                .is_some_and(|prev| cores.get(p) == Some(&prev) && !choice.removed.contains(&prev))
        })
        .count();
    Some(
        json!({"safe":true,"ambiguous":false,"reference_checked":true,"reason":"可信原始探测可唯一闭合","pair_count":count,"tail_empty_count":0,"duplicate_count":0,"removed_duplicate_audio_positions":choice.removed,"core_stream_positions":choice.removed.iter().map(|p|audio[*p].0).collect::<Vec<_>>(),"target_stream_positions":choice.retained.iter().map(|p|audio[*p].0).collect::<Vec<_>>(),"metadata_signature":sources.iter().map(signature).collect::<Vec<_>>(),"metadata_sources":sources,"reference_evidence":evidence,"removed_non_core_count":choice.removed.len()-removed_cores,"exact_stream_identity":!clpi}),
    )
}
pub fn analyze(info: &Value, raw: Option<&Value>, evidence: &str, prefer: bool) -> Value {
    let report = metadata(info);
    let Some(raw) = raw else {
        return report;
    };
    let reference = reference(info, raw, evidence, &[], false);
    if prefer {
        return reference.unwrap_or_else(||json!({"safe":false,"ambiguous":true,"reference_checked":true,"reason":"CD2 精确探测无法与当前音轨结构唯一对应"}));
    }
    if report["safe"] == true {
        report
    } else {
        reference.unwrap_or(report)
    }
}
pub fn apply(info: &Value, mut report: Value) -> (Value, Value) {
    let mut repaired = info.clone();
    if report["safe"] != true {
        report["changed"] = json!(false);
        return (repaired, report);
    }
    let targets = list(&report["target_stream_positions"]);
    let sources = list(&report["metadata_sources"]);
    let cores = list(&report["core_stream_positions"]);
    let streams = media_info::streams_mut(&mut repaired).unwrap();
    for (target, source) in targets.iter().zip(sources) {
        let target = &mut streams[target.as_u64().unwrap() as usize];
        let display = text(target, "DisplayTitle");
        if !display.is_empty() {
            let old = text(target, "DisplayLanguage");
            let suffix =
                if !old.is_empty() && display.to_lowercase().starts_with(&old.to_lowercase()) {
                    display[old.len()..].trim()
                } else {
                    &display
                };
            target["DisplayTitle"] = json!(
                [text(&source, "DisplayLanguage"), suffix.into()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        for field in ["Language", "Title", "DisplayLanguage"] {
            if let Some(value) = source.get(field) {
                target[field] = value.clone();
            } else {
                target.as_object_mut().unwrap().remove(field);
            }
        }
    }
    let mut index = 0;
    streams.retain(|_| {
        let keep = !cores.contains(&json!(index));
        index += 1;
        keep
    });
    report["changed"] = json!(true);
    (repaired, report)
}
