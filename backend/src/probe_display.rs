use crate::{assets, media_edit::list, state::string};
use serde_json::Value;
use std::sync::LazyLock;

static RULES: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("../assets/stream_rules.json")).unwrap());
static LANGUAGES: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("../assets/asset_rules.json")).unwrap());
fn re(pattern: &str) -> std::sync::Arc<fancy_regex::Regex> {
    static CACHE: LazyLock<
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<fancy_regex::Regex>>>,
    > = LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut cache = CACHE.lock().unwrap();
    cache
        .entry(pattern.into())
        .or_insert_with(|| std::sync::Arc::new(fancy_regex::Regex::new(pattern).unwrap()))
        .clone()
}
fn matched(pattern: &str, text: &str) -> bool {
    re(pattern).is_match(text).unwrap_or(false)
}
fn replace(pattern: &str, text: &str, value: &str) -> String {
    re(pattern).replace_all(text, value).into_owned()
}
fn simplified(text: &str) -> String {
    let mut text = text.to_string();
    for (old, new) in [
        ("粵", "粤"),
        ("語", "语"),
        ("國", "国"),
        ("雙", "双"),
        ("臺", "台"),
        ("灣", "湾"),
        ("聽", "听"),
        ("導", "导"),
        ("評", "评"),
        ("視", "视"),
        ("體", "体"),
        ("簡", "简"),
        ("譯", "译"),
        ("長", "长"),
        ("蘋", "苹"),
        ("華", "华"),
    ] {
        text = text.replace(old, new);
    }
    text
}
fn marker_text(text: &str) -> String {
    replace(
        r"\s+",
        &replace(
            r"[.\-_+/|\\\[\]()【】（）]+",
            &simplified(text).to_lowercase(),
            " ",
        ),
        " ",
    )
    .trim()
    .into()
}
fn markers(text: &str, words: &[String]) -> bool {
    let text = marker_text(text);
    words.iter().any(|word| {
        let word = marker_text(word);
        !word.is_empty()
            && if crate::translation::chinese(&word) {
                text.contains(&word)
            } else {
                matched(
                    &format!(r"(?<![a-z0-9]){}(?![a-z0-9])", regex::escape(&word)),
                    &text,
                )
            }
    })
}
fn words(key: &str) -> Vec<String> {
    list(&LANGUAGES["keywords"][key])
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}
fn features(kind: &str, text: &str) -> Vec<String> {
    let text = simplified(text);
    list(&RULES["DEFAULT_STREAM_FEATURE_MAPPING"])
        .iter()
        .filter(|item| list(&item["types"]).iter().any(|t| t == kind))
        .filter(|item| {
            list(&item["patterns"])
                .iter()
                .filter_map(Value::as_str)
                .any(|p| matched(&format!("(?i){p}"), &text))
        })
        .map(|item| string(item, "label"))
        .collect()
}
fn clean(text: &str) -> String {
    let mut text = text
        .chars()
        .filter(|c| {
            matches!(*c as u32,0x4e00..=0x9fff|0x3400..=0x4dbf|0xf900..=0xfaff|0x20000..=0x2ebef)
                || "（）".contains(*c)
        })
        .collect::<String>();
    for word in list(&RULES["STREAM_TITLE_GARBAGE_FILTER"]) {
        if let Some(word) = word.as_str() {
            text = text.replace(word, "");
        }
    }
    text.trim().into()
}
fn script(text: &str) -> &str {
    if text.contains('繁') {
        "cht"
    } else if text.contains('简') || text.contains('簡') {
        "chs"
    } else {
        ""
    }
}
fn code_script(code: &str) -> &str {
    let code = code.to_lowercase().replace('_', "-");
    if code.split('-').any(|p| ["hans", "chs", "zhs"].contains(&p)) {
        "chs"
    } else if code.split('-').any(|p| ["hant", "cht", "zht"].contains(&p)) {
        "cht"
    } else if code.starts_with("zh-")
        || code.starts_with("cmn-")
        || code.starts_with("zho-")
        || code.starts_with("chi-")
    {
        if code.ends_with("-cn") || code.ends_with("-sg") {
            "chs"
        } else if code.ends_with("-tw") || code.ends_with("-hk") || code.ends_with("-mo") {
            "cht"
        } else {
            ""
        }
    } else {
        ""
    }
}
fn normalized_subtitle(text: &str) -> String {
    let text = text.trim().replace("繁體", "繁体").replace("簡體", "简体");
    let text = replace(r"^(中.+?双语)[（(](简体|繁体)[）)]$", &text, "$1$2");
    if [
        "简",
        "簡",
        "简体",
        "簡體",
        "简中",
        "中简",
        "中簡",
        "中文简体",
        "简体中文",
    ]
    .contains(&text.as_str())
    {
        return "中文简体".into();
    }
    if ["繁", "繁体", "繁體", "繁中", "中繁", "中文繁体", "繁体中文"].contains(&text.as_str())
    {
        return "中文繁体".into();
    }
    text
}
fn compact(text: &str) -> String {
    let text = text
        .trim()
        .replace("簡", "简")
        .replace("繁體", "繁体")
        .replace("簡體", "简体")
        .replace("雙語", "双语");
    let text = replace(r"^(中.+?双语)[（(](简体|繁体)[）)]$", &text, "$1$2");
    if matched(r"^中.+?双语(简体|繁体)$", &text) {
        return text;
    }
    if let Ok(Some(c)) = re(r"^(简|繁)(?!体|中)(.+?双语)(.*)$").captures(&text) {
        return format!(
            "中{}{}{}",
            c.get(2).unwrap().as_str(),
            c.get(3).unwrap().as_str().trim(),
            if c.get(1).unwrap().as_str() == "简" {
                "简体"
            } else {
                "繁体"
            }
        );
    }
    if let Ok(Some(c)) = re(r"^(简|繁)(?!体|中)([英日韩西德法葡俄泰越意])$").captures(&text)
    {
        return format!(
            "中{}双语{}",
            c.get(2).unwrap().as_str(),
            if c.get(1).unwrap().as_str() == "简" {
                "简体"
            } else {
                "繁体"
            }
        );
    }
    String::new()
}
fn label(code: &str, mappings: &Value) -> String {
    match code {
        "chi" | "zho" | "zh" | "cmn" => return "国语".into(),
        "yue" => return "粤语".into(),
        "chs" => return "简体".into(),
        "cht" => return "繁体".into(),
        _ => (),
    }
    let mappings = if list(mappings).is_empty() {
        &LANGUAGES["defaults"]
    } else {
        mappings
    };
    list(mappings)
        .iter()
        .find(|item| {
            string(item, "value").eq_ignore_ascii_case(code)
                || list(&item["aliases"])
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|s| s.eq_ignore_ascii_case(code))
        })
        .map(|item| string(item, "label"))
        .unwrap_or(code.to_uppercase())
}
pub fn friendly(
    raw_lang: &str,
    raw_title: &str,
    kind: &str,
    display_title: &str,
    hearing: bool,
    mappings: &Value,
) -> (String, String, String) {
    let raw_lang = raw_lang.trim();
    let raw_title = raw_title.trim();
    let mut feature = features(kind, &format!("{raw_title} {display_title}"));
    let mut detect = raw_title.to_string();
    for item in list(&RULES["DEFAULT_STREAM_FEATURE_MAPPING"]) {
        if feature.contains(&string(&item, "label")) {
            for pattern in list(&item["patterns"]).iter().filter_map(Value::as_str) {
                detect = replace(&format!("(?i){pattern}"), &detect, " ");
            }
        }
    }
    let mut code = assets::normalize(raw_lang, mappings);
    let title_has_lang = markers(
        &detect,
        &[
            "简", "繁", "chs", "cht", "eng", "english", "en", "英文", "英语", "jpn", "japanese",
            "ja", "jp", "日文", "日语", "kor", "korean", "ko", "kr", "韩文", "韩语", "台配",
            "台湾",
        ]
        .map(str::to_string),
    ) || LANGUAGES["keywords"]
        .as_object()
        .unwrap()
        .values()
        .any(|v| {
            markers(
                &detect,
                &list(v)
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>(),
            )
        });
    if title_has_lang {
        code = assets::normalize(&detect, mappings);
    }
    let combined = if title_has_lang {
        detect.clone()
    } else {
        format!("{detect} {raw_lang}")
    };
    let mut display = String::new();
    if kind == "Subtitle" {
        let combined_title = format!("{detect} {display_title}");
        let mut writing = code_script(raw_lang);
        if writing.is_empty() {
            writing = script(&combined_title);
        }
        if writing.is_empty() {
            if markers(
                &combined_title,
                &[
                    "cht",
                    "tc",
                    "big5",
                    "zh tw",
                    "zh hk",
                    "zh hant",
                    "traditional",
                ]
                .map(str::to_string),
            ) {
                writing = "cht";
            } else if markers(
                &combined_title,
                &["chs", "sc", "gb", "zh cn", "zh hans", "simplified"].map(str::to_string),
            ) {
                writing = "chs";
            }
        }
        let dual = compact(raw_title);
        if !dual.is_empty() {
            code = if writing.is_empty() {
                script(&dual)
            } else {
                writing
            }
            .to_string();
            display = dual;
        } else if markers(&detect, &words("sub_yue")) {
            code = "yue".into();
            display = "粤语".into();
        } else if !writing.is_empty() {
            code = writing.into();
            let suffix = if writing == "cht" { "繁体" } else { "简体" };
            let secondary = [
                (
                    "英",
                    [
                        "eng", "english", "en", "英文", "英语", "英字", "简英", "繁英", "中英",
                    ],
                ),
                (
                    "日",
                    [
                        "jpn", "japanese", "ja", "日文", "日语", "日字", "简日", "繁日", "中日",
                    ],
                ),
                (
                    "韩",
                    [
                        "kor", "korean", "ko", "韩文", "韩语", "韩字", "简韩", "繁韩", "中韩",
                    ],
                ),
            ]
            .iter()
            .find(|(_, w)| markers(&combined, &w.map(str::to_string)))
            .map(|(s, _)| *s);
            display = secondary
                .map(|s| format!("中{s}双语{suffix}"))
                .unwrap_or(format!("中文{suffix}"));
        } else if markers(&combined, &words("sub_cht"))
            || markers(&combined, &["台配", "台湾"].map(str::to_string))
        {
            code = "cht".into();
            display = "中文繁体".into();
        } else if markers(&combined, &words("sub_chs")) {
            code = "chs".into();
            display = "中文简体".into();
        } else if code == "yue" || raw_lang.to_lowercase().starts_with("yue") {
            code = "cht".into();
            display = "中文繁体".into();
        } else if ["chi", "zho", "zh", "cmn", "chs", "cht"].contains(&code.as_str())
            || markers(&combined, &words("sub_chi"))
        {
            code = "chi".into();
            display = "国语".into();
        } else {
            for (key, keywords) in LANGUAGES["keywords"].as_object().unwrap() {
                if key.starts_with("sub_")
                    && markers(
                        &combined,
                        &list(keywords)
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>(),
                    )
                {
                    code = key.trim_start_matches("sub_").into();
                    break;
                }
            }
        }
    } else {
        let yue = markers(&combined, &words("yue"));
        let chi = markers(&combined, &words("chi"));
        if yue && !chi {
            code = "yue".into();
        } else if chi && !yue {
            code = "chi".into();
        } else {
            for (key, keywords) in LANGUAGES["keywords"].as_object().unwrap() {
                if !key.starts_with("sub_")
                    && markers(
                        &combined,
                        &list(keywords)
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>(),
                    )
                {
                    code = key.clone();
                    break;
                }
            }
        }
    }
    if display.is_empty() {
        display = label(&code, mappings);
        if kind == "Subtitle" {
            display = match display.as_str() {
                "简体" | "简中" | "简体中文" => "中文简体".into(),
                "繁体" | "繁中" | "繁体中文" => "中文繁体".into(),
                _ => display
                    .strip_suffix('语')
                    .filter(|_| display != "国语" && display != "粤语" && display != "无语言")
                    .map(|s| format!("{s}文"))
                    .unwrap_or(display),
            };
        }
    }
    if display.is_empty() {
        display = "未知".into();
    }
    for f in features(kind, &format!("{raw_title} {display_title} {raw_lang}")) {
        if !feature.contains(&f) {
            feature.push(f);
        }
    }
    if kind == "Subtitle" && raw_title.contains("智能识别") {
        feature.push("智能识别".into());
    }
    if kind == "Subtitle" && hearing && !feature.contains(&"听障".to_string()) {
        feature.push("听障".into());
    }
    let mut title = clean(&simplified(raw_title));
    if kind == "Subtitle" {
        title = replace(r"画面内.*?（.*?）", &title, "")
            .replace("智能识别", "")
            .replace("简体中文", "中文简体")
            .replace("简中", "中文简体")
            .replace("繁体中文", "中文繁体")
            .replace("繁中", "中文繁体")
            .replace("原盘", "");
        let dual = compact(&title);
        if !dual.is_empty() {
            title = dual;
        }
        title = normalized_subtitle(&title);
        if title == "中文简体"
            || title == "中文繁体"
            || matched(r"^中.+?双语(?:特效)?(简体|繁体)$", &title)
        {
            display = title.clone();
        }
        title = if display != "未知" {
            display.clone()
        } else {
            String::new()
        };
    } else if title.is_empty() {
        title = if display != "未知" {
            display.clone()
        } else {
            raw_title.into()
        };
    } else if display != "未知" {
        if let Some(rest) = title.strip_prefix(&display) {
            title = rest.into();
        }
        if title.is_empty() {
            title = display.clone();
        } else if !title.contains(&display) {
            title = format!("{display}（{title}）");
        }
    }
    if !feature.is_empty() {
        title = normalized_subtitle(&display);
        if feature.contains(&"特效".to_string()) {
            if matched(r"^(中.+?双语)(简体|繁体)$", &title) {
                title = replace(r"^(中.+?双语)(简体|繁体)$", &title, "$1特效$2");
            } else if !title.contains("特效") {
                title.push_str("特效");
            }
            feature.retain(|f| f != "特效");
        }
        if !feature.is_empty() {
            title = format!("{title}（{}）", feature.join("·"));
        }
    }
    if kind == "Subtitle" {
        display = normalized_subtitle(&display);
        title = normalized_subtitle(&title);
    }
    if code.is_empty() {
        code = raw_lang.into();
    }
    (code, display, title)
}
