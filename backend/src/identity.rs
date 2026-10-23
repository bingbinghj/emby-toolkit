use crate::error::{ApiError, Result};
use serde_json::{Value, json};

pub fn size(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .filter(|v| *v > 0)
}
fn valid(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    (!matches!(
        text.to_lowercase().as_str(),
        "" | "none" | "null" | "[]" | "{}"
    ))
    .then_some(text)
}
pub fn merge(
    old: &Value,
    new: &Value,
    old_pc: &Value,
    old_sha: &Value,
    new_pc: &Value,
    new_sha: &Value,
) -> (Value, Value, Value) {
    let (old_pc, old_sha, new_pc, new_sha) =
        (valid(old_pc), valid(old_sha), valid(new_pc), valid(new_sha));
    let mut asset = if new.as_object().is_some_and(|v| !v.is_empty()) {
        new.clone()
    } else if old.is_object() {
        old.clone()
    } else {
        json!({})
    };
    let pc_changed =
        new_pc.is_some() && old_pc != new_pc && (old_pc.is_some() || old_sha.is_some());
    let sha_changed = new_sha.is_some()
        && old_sha.as_ref().map(|s| s.to_uppercase()) != new_sha.as_ref().map(|s| s.to_uppercase())
        && (old_pc.is_some() || old_sha.is_some());
    let changed_path = ["path", "fingerprint_target"].iter().any(|key| {
        let before = old[*key].as_str().unwrap_or("");
        let after = asset[*key].as_str().unwrap_or("");
        !before.is_empty() && !after.is_empty() && before != after
    });
    let changed_size =
        matches!((size(&old["size_bytes"]),size(&asset["size_bytes"])),(Some(a),Some(b)) if a!=b);
    let changed = pc_changed || sha_changed || changed_path || changed_size;
    for key in [
        "fingerprint_pending",
        "fingerprint_revision",
        "fingerprint_target",
    ] {
        if asset.get(key).is_none() && old.get(key).is_some() {
            asset[key] = old[key].clone();
        }
    }
    if changed || old["fingerprint_pending"].as_bool().unwrap_or(false) {
        asset["fingerprint_pending"] = json!(true);
        asset["fingerprint_revision"] =
            json!(old["fingerprint_revision"].as_u64().unwrap_or(0) + u64::from(changed));
    }
    if size(&asset["size_bytes"]).is_none() && size(&old["size_bytes"]).is_some() {
        asset["size_bytes"] = old["size_bytes"].clone();
    }
    (
        asset,
        json!(new_pc.or(if sha_changed { None } else { old_pc })),
        json!(new_sha.or(if pc_changed { None } else { old_sha })),
    )
}
fn media_size(value: &Value) -> Option<u64> {
    match value {
        Value::Array(items) => items.iter().find_map(media_size),
        Value::Object(_) => size(&value["Size"])
            .or_else(|| media_size(&value["MediaSourceInfo"]))
            .or_else(|| media_size(&value["MediaSources"])),
        _ => None,
    }
}
pub fn verify(asset: &Value, info: &Value) -> Result<()> {
    if asset["fingerprint_pending"].as_bool().unwrap_or(false) {
        return Err(ApiError::bad("文件身份待校验，请先重新校验指纹"));
    }
    if matches!((size(&asset["size_bytes"]),media_size(info)),(Some(a),Some(b)) if a!=b) {
        return Err(ApiError::bad(
            "媒体信息大小与当前文件不一致，请重新提取媒体信息",
        ));
    }
    Ok(())
}
