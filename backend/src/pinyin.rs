use std::{collections::HashMap, io::Read, sync::OnceLock};

struct Dictionary {
    characters: HashMap<String, String>,
    phrases: HashMap<String, String>,
    max_phrase_length: usize,
}
fn dictionary() -> &'static Dictionary {
    static DATA: OnceLock<Dictionary> = OnceLock::new();
    DATA.get_or_init(|| {
        let mut json = String::new();
        flate2::read::GzDecoder::new(&include_bytes!("../assets/pinyin.json.gz")[..])
            .read_to_string(&mut json)
            .expect("bundled pinyin data");
        let (characters, phrases): (HashMap<String, String>, HashMap<String, String>) =
            serde_json::from_str(&json).expect("bundled pinyin dictionaries");
        let max_phrase_length = phrases.keys().map(|s| s.chars().count()).max().unwrap_or(1);
        Dictionary {
            characters,
            phrases,
            max_phrase_length,
        }
    })
}
// These ranges are pypinyin's RE_HANS, which also determines non-Chinese runs.
fn is_han(c: char) -> bool {
    matches!(c as u32, 0x3007 | 0xe815..=0xe864 | 0xfa18 | 0x3400..=0x4dbf |
        0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x2a6df |
        0x2a703..=0x2b73f | 0x2b740..=0x2b81d | 0x2b825..=0x2bf6e |
        0x2c029..=0x2ce93 | 0x2d016 | 0x2d11b..=0x2ebd9 |
        0x2f80a..=0x2fa1f | 0x30000..=0x3134a | 0x31350..=0x32389)
}
pub fn initials(title: &str) -> String {
    let data = dictionary();
    let mut rest = title;
    let mut output = String::new();
    while let Some(first) = rest.chars().next() {
        let han = is_han(first);
        let end = rest
            .char_indices()
            .find(|(_, c)| is_han(*c) != han)
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        let mut run = &rest[..end];
        rest = &rest[end..];
        if !han {
            if run.chars().all(char::is_alphanumeric) {
                output.push_str(&run.to_uppercase());
            }
            continue;
        }
        while !run.is_empty() {
            let ends = run
                .char_indices()
                .skip(1)
                .map(|(i, _)| i)
                .chain(std::iter::once(run.len()))
                .take(data.max_phrase_length)
                .collect::<Vec<_>>();
            if let Some((end, initials)) = ends
                .iter()
                .rev()
                .find_map(|end| data.phrases.get(&run[..*end]).map(|v| (*end, v)))
            {
                output.push_str(&initials.to_uppercase());
                run = &run[end..];
            } else {
                let end = ends[0];
                let value = data
                    .characters
                    .get(&run[..end])
                    .map(String::as_str)
                    .unwrap_or(&run[..end]);
                if value.chars().all(char::is_alphanumeric) {
                    output.push_str(&value.to_uppercase());
                }
                run = &run[end..];
            }
        }
    }
    if output.is_empty() {
        title.to_string()
    } else {
        output
    }
}
