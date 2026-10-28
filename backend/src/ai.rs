use crate::{
    AppState, db,
    error::{ApiError, Result},
    state::string,
};
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::LazyLock, time::Duration};
use tokio::sync::{Mutex, RwLock};

#[derive(Default)]
pub struct Service {
    pub cache_gate: RwLock<()>,
    pub cache_revision: std::sync::atomic::AtomicU64,
    last_request: Mutex<Option<std::time::Instant>>,
    translations: Mutex<(u64, BTreeMap<String, String>)>,
}
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/ai/test", post(test_connection))
        .route("/api/ai/models", post(models))
}
pub fn model_ids(payload: &Value, provider: &str) -> Vec<String> {
    let entries = if payload.is_array() {
        payload.as_array()
    } else {
        ["data", "models", "items"]
            .iter()
            .find_map(|key| payload[*key].as_array())
    };
    let mut output = vec![];
    for entry in entries.into_iter().flatten() {
        if provider == "gemini"
            && ["supportedGenerationMethods", "supported_generation_methods"]
                .iter()
                .find_map(|k| entry[*k].as_array())
                .is_some_and(|m| !m.contains(&json!("generateContent")))
        {
            continue;
        }
        let id = entry
            .as_str()
            .or_else(|| {
                ["id", "name", "model"]
                    .iter()
                    .find_map(|k| entry[*k].as_str().filter(|s| !s.is_empty()))
            })
            .unwrap_or("")
            .trim();
        let id = if provider == "gemini" {
            id.trim_start_matches("models/")
        } else {
            id
        };
        if !id.is_empty() && !output.iter().any(|s| s == id) {
            output.push(id.to_string());
        }
    }
    output
}
async fn models(State(state): State<AppState>, Json(body): Json<Value>) -> Result<Json<Value>> {
    let provider = body["ai_provider"].as_str().unwrap_or("openai");
    let key = string(&body, "ai_api_key");
    if key.trim().is_empty() {
        return Err(ApiError::bad("请先填写API Key"));
    }
    let mut base = string(&body, "ai_base_url")
        .trim()
        .trim_end_matches('/')
        .to_string();
    if base.is_empty() {
        base = match provider {
            "gemini" => "https://generativelanguage.googleapis.com/v1beta",
            "zhipuai" => "https://open.bigmodel.cn/api/paas/v4",
            _ => "https://api.openai.com/v1",
        }
        .into();
    }
    let mut request = if provider == "gemini" {
        let url = if base.ends_with("/models") {
            base
        } else {
            format!("{base}/models")
        };
        state.http.get(url).query(&[("key", key)])
    } else {
        for suffix in ["/chat/completions", "/completions", "/responses", "/models"] {
            if let Some(value) = base.strip_suffix(suffix) {
                base = value.into();
            }
        }
        state.http.get(format!("{base}/models")).bearer_auth(key)
    };
    request = request
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(20));
    let response = request.send().await?;
    if matches!(response.status().as_u16(), 401 | 403) {
        return Err(ApiError::bad("API Key无效或无权限访问模型列表"));
    }
    let payload: Value = response.error_for_status()?.json().await?;
    let models = model_ids(&payload, provider);
    Ok(Json(
        json!({"success":true,"count":models.len(),"models":models}),
    ))
}
async fn test_connection(
    State(state): State<AppState>,
    Json(config): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if config.as_object().is_none_or(|v| v.is_empty()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"success":false,"message":"缺少配置数据"})),
        );
    }
    let defaults: Value = serde_json::from_str(include_str!("../assets/defaults.json")).unwrap();
    let prompts = db::setting(&state.db, "ai_user_prompts")
        .await
        .ok()
        .flatten()
        .unwrap_or(json!({}));
    let system = prompts["fast_mode"].as_str().unwrap_or(
        defaults["DEFAULT_AI_PROMPTS"]["fast_mode"]
            .as_str()
            .unwrap(),
    );
    match request(&state, &config, system, r#"["Bald Qiang"]"#, 0.0).await {
        Ok(result) if result["Bald Qiang"].as_str().is_some_and(|s| !s.is_empty()) => {
            let result = result["Bald Qiang"].as_str().unwrap();
            (
                StatusCode::OK,
                Json(
                    json!({"success":true,"message":if result=="Bald Qiang"{"连接成功，但 AI 返回了原词（可能模型认为无需翻译）。".to_string()}else{format!("连接成功！测试翻译结果: 'Bald Qiang' ➜ '{result}'")}}),
                ),
            )
        }
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success":false,"message":"AI 未返回有效结果。"})),
        ),
    }
}
pub fn simplified(text: &str) -> String {
    static CONVERTER: LazyLock<ferrous_opencc::OpenCC> = LazyLock::new(|| {
        ferrous_opencc::OpenCC::from_config(ferrous_opencc::config::BuiltinConfig::T2s)
            .expect("bundled t2s dictionary")
    });
    CONVERTER.convert(text)
}
pub fn parse_json(raw: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str(raw) {
        return Some(value);
    }
    let pattern = regex::Regex::new(r"(?s)```(?:json)?\s*(\{.*?\})\s*```").unwrap();
    let text = pattern
        .captures(raw)
        .map(|c| c[1].to_string())
        .unwrap_or_else(|| raw.into());
    if let Ok(value) = serde_json::from_str(&text) {
        return Some(value);
    }
    let last_quote = text.rfind('"');
    let last_brace = text.rfind('}');
    let fixed = if last_brace.is_some_and(|b| last_quote.is_none_or(|q| b > q)) {
        text[..=last_brace.unwrap()].to_string()
    } else if let Some(q) = last_quote {
        if let Some(previous) = text[..q].rfind('"') {
            if let Some(comma) = text[..previous].rfind(',') {
                format!("{}\n}}", &text[..comma])
            } else {
                "{}".into()
            }
        } else {
            text.clone()
        }
    } else {
        text.clone()
    };
    (fixed != text)
        .then(|| serde_json::from_str(&fixed).ok())
        .flatten()
}
fn python_json(value: &Value) -> String {
    match value {
        Value::Array(v) => format!(
            "[{}]",
            v.iter().map(python_json).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(v) => format!(
            "{{{}}}",
            v.iter()
                .map(|(k, v)| format!("{}: {}", json!(k), python_json(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => value.to_string(),
    }
}
pub fn cache_key(
    config: &Value,
    purpose: &str,
    prompt: &str,
    source: &Value,
    context: &Value,
    actor: &Value,
) -> String {
    let identity = json!([
        string(config, "ai_provider").to_lowercase(),
        config["ai_base_url"],
        config["ai_model_name"],
        purpose,
        prompt,
        source,
        context,
        actor
    ]);
    format!(
        "ai-result:v1:{:x}",
        Sha256::digest(python_json(&identity).as_bytes())
    )
}
pub async fn request(
    state: &AppState,
    config: &Value,
    system: &str,
    user: &str,
    temperature: f64,
) -> Result<Value> {
    let provider = string(config, "ai_provider").to_lowercase();
    let key = string(config, "ai_api_key");
    let model = string(config, "ai_model_name");
    if key.is_empty() || model.is_empty() {
        return Err(ApiError::bad("AI API Key 或模型未配置"));
    }
    let timeout = Duration::from_secs(
        config["ai_request_timeout"]
            .as_u64()
            .filter(|n| *n > 0)
            .unwrap_or(180),
    );
    let base = string(config, "ai_base_url");
    let (url, body) = if provider == "gemini" {
        (
            format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"
            ),
            json!({"systemInstruction":{"parts":[{"text":system}]},"contents":[{"parts":[{"text":user}]}],"generationConfig":{"responseMimeType":"application/json","temperature":temperature}}),
        )
    } else if matches!(provider.as_str(), "openai" | "zhipuai") {
        let default = if provider == "openai" {
            "https://api.openai.com/v1"
        } else {
            "https://open.bigmodel.cn/api/paas/v4"
        };
        let base = if provider == "zhipuai" || base.is_empty() {
            default
        } else {
            &base
        };
        (
            format!("{}/chat/completions", base.trim_end_matches('/')),
            json!({"model":model,"messages":[{"role":"system","content":system},{"role":"user","content":user}],"response_format":{"type":"json_object"},"temperature":temperature}),
        )
    } else {
        tracing::warn!("AI 请求拒绝：不支持的服务 {provider}");
        return Err(ApiError::bad("不支持的 AI 服务"));
    };
    let retries = if provider == "openai" {
        config["ai_max_retries"].as_u64().unwrap_or(0)
    } else {
        0
    };
    for attempt in 0..=retries {
        tracing::info!(
            "AI 请求开始：服务 {provider}，模型 {model}，尝试 {}/{}",
            attempt + 1,
            retries + 1
        );
        let mut request = state.http.post(&url).timeout(timeout).json(&body);
        request = if provider == "gemini" {
            request.header("x-goog-api-key", &key)
        } else {
            request.bearer_auth(&key)
        };
        let response = request.send().await;
        let retry = match &response {
            Ok(r) => matches!(r.status().as_u16(), 408 | 409 | 429 | 500..=599),
            Err(e) => e.is_connect() || e.is_timeout(),
        };
        if retry && attempt < retries {
            tracing::warn!(
                "AI 请求暂时失败：服务 {provider}，{}，准备重试 {}/{}",
                response
                    .as_ref()
                    .map(|r| format!("HTTP {}", r.status().as_u16()))
                    .unwrap_or_else(|error| if error.is_timeout() {
                        "网络超时"
                    } else if error.is_connect() {
                        "网络连接失败"
                    } else {
                        "网络请求失败"
                    }
                    .to_string()),
                attempt + 1,
                retries
            );
            tokio::time::sleep(Duration::from_millis(
                (500 * 2_u64.saturating_pow(attempt.min(8) as u32)).min(8000),
            ))
            .await;
            continue;
        }
        let response: Value = response
            .inspect_err(|_| {
                tracing::error!(
                    "AI 请求失败：服务 {provider}，网络错误，尝试 {} 次",
                    attempt + 1
                )
            })?
            .error_for_status()
            .inspect_err(|e| {
                tracing::error!(
                    "AI 请求失败：服务 {provider}，{}",
                    e.status()
                        .map(|status| format!("HTTP {}", status.as_u16()))
                        .unwrap_or_else(|| "未收到 HTTP 响应".to_string())
                )
            })?
            .json()
            .await
            .inspect_err(|_| tracing::error!("AI 请求失败：服务 {provider}，响应 JSON 无效"))?;
        let content = if provider == "gemini" {
            response["candidates"][0]["content"]["parts"][0]["text"].as_str()
        } else {
            response["choices"][0]["message"]["content"].as_str()
        };
        return content
            .and_then(parse_json)
            .ok_or_else(|| ApiError::bad("AI 未返回有效 JSON"))
            .inspect(|value| {
                tracing::info!(
                    "AI 请求完成：服务 {provider}，模型 {model}，返回条目 {} 个",
                    value.as_object().map_or(0, |o| o.len())
                )
            })
            .inspect_err(|_| tracing::warn!("AI 响应解析失败：服务 {provider}，未返回有效 JSON"));
    }
    unreachable!()
}
pub(crate) fn purpose_label(purpose: &str) -> &str {
    match purpose {
        "fast_mode" => "翻译模式",
        "transliterate_mode" => "音译模式",
        "quality_mode" => "顾问模式",
        "overview" | "overview_translation" => "简介翻译",
        "title" | "title_translation" => "标题翻译",
        "tagline" => "标语翻译",
        "batch_overview_translation" => "批量简介翻译",
        "batch_title_translation" => "批量标题翻译",
        "batch_joke_fallback" => "老六占位简介",
        _ => purpose,
    }
}
pub async fn generate_jokes(
    state: &AppState,
    inputs: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, Value>> {
    if inputs.is_empty() {
        return Ok(BTreeMap::new());
    }
    let config = state.config.read().await.clone();
    let defaults: Value = serde_json::from_str(include_str!("../assets/defaults.json")).unwrap();
    let prompts = db::setting(&state.db, "ai_user_prompts")
        .await?
        .unwrap_or(json!({}));
    let prompt = prompts["batch_joke_fallback"]
        .as_str()
        .or_else(|| defaults["DEFAULT_AI_PROMPTS"]["batch_joke_fallback"].as_str())
        .unwrap_or("");
    // 原版笑话请求整批直发，不读写翻译缓存，也不净化或校验返回文本。
    Ok(
        match request(state, &config, prompt, &python_json(&json!(inputs)), 0.8).await {
            Ok(value) => value
                .as_object()
                .map(|values| values.clone().into_iter().collect())
                .unwrap_or_default(),
            Err(error) => {
                tracing::warn!("[老六笑话生成] AI 翻车了：{}", error.1);
                BTreeMap::new()
            }
        },
    )
}
pub async fn translate(
    state: &AppState,
    inputs: &BTreeMap<String, String>,
    purpose: &str,
    context: &Value,
    actors: &Value,
) -> Result<BTreeMap<String, String>> {
    translate_inner(state, inputs, purpose, context, actors, true).await
}
pub async fn translate_preview(
    state: &AppState,
    inputs: &BTreeMap<String, String>,
    purpose: &str,
    context: &Value,
    actors: &Value,
) -> Result<BTreeMap<String, String>> {
    translate_inner(state, inputs, purpose, context, actors, false).await
}
async fn translate_inner(
    state: &AppState,
    inputs: &BTreeMap<String, String>,
    purpose: &str,
    context: &Value,
    actors: &Value,
    persist: bool,
) -> Result<BTreeMap<String, String>> {
    if inputs.is_empty() {
        return Ok(BTreeMap::new());
    }
    if purpose == "batch_joke_fallback" {
        return Ok(generate_jokes(state, inputs)
            .await?
            .into_iter()
            .filter_map(|(key, value)| value.as_str().map(|text| (key, text.to_string())))
            .collect());
    }
    let mut memory = state.ai.translations.lock().await;
    let revision = state
        .ai
        .cache_revision
        .load(std::sync::atomic::Ordering::SeqCst);
    if memory.0 != revision {
        memory.0 = revision;
        memory.1.clear();
    }
    let config = state.config.read().await.clone();
    let purpose_name = purpose_label(purpose);
    let media_context = context["title"]
        .as_str()
        .filter(|title| !title.is_empty())
        .map(|title| format!("《{title}》，"))
        .unwrap_or_default();
    tracing::info!(
        "AI 翻译开始：{media_context}用途 {purpose_name}，输入 {} 条，缓存方式 {}",
        inputs.len(),
        if persist {
            "正常写入缓存"
        } else {
            "预览结果，不写入缓存"
        }
    );
    let defaults: Value = serde_json::from_str(include_str!("../assets/defaults.json")).unwrap();
    let prompts = db::setting(&state.db, "ai_user_prompts")
        .await?
        .unwrap_or(json!({}));
    let prompt = prompts[purpose]
        .as_str()
        .or_else(|| defaults["DEFAULT_AI_PROMPTS"][purpose].as_str())
        .unwrap_or("");
    let mut system = prompt.to_string();
    if let Some(title) = context["title"].as_str() {
        system = system.replace("{context_title}", title);
    }
    if let Some(context) = context.as_object() {
        for (key, value) in context {
            system = system.replace(&format!("{{{key}}}"), value.as_str().unwrap_or(""));
        }
    }
    if purpose != "batch_joke_fallback" {
        system = system.replace("{{", "{").replace("}}", "}");
    }
    let mut output = BTreeMap::new();
    let mut missing = BTreeMap::new();
    let mut keys = BTreeMap::new();
    for (id, source) in inputs {
        let key = cache_key(
            &config,
            purpose,
            prompt,
            &json!(source),
            context,
            &actors[id],
        );
        let cached: Option<String> = if let Some(value) = memory.1.get(&key) {
            Some(value.clone())
        } else {
            sqlx::query_scalar(
                "SELECT translated_text FROM translation_cache WHERE original_text=$1",
            )
            .bind(&key)
            .fetch_optional(&state.db)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!("读取翻译结果缓存失败，继续翻译：{error}");
                None
            })
            .flatten()
        };
        if let Some(cached) = cached.filter(|s| {
            !s.is_empty() && (purpose != "batch_joke_fallback" || s.starts_with("【老六占位简介】"))
        }) {
            if persist {
                memory.1.insert(key.clone(), cached.clone());
            }
            output.insert(id.clone(), cached);
        } else {
            missing.insert(id.clone(), source.clone());
        }
        keys.insert(id.clone(), key);
    }
    tracing::info!(
        "AI 翻译缓存检查完成：用途 {purpose_name}，命中 {} 条，待请求 {} 条",
        output.len(),
        missing.len()
    );
    let batch = if matches!(purpose, "fast_mode" | "transliterate_mode" | "quality_mode") {
        config["ai_translation_batch_size"]
            .as_u64()
            .filter(|n| *n > 0)
            .unwrap_or(10) as usize
    } else {
        20
    };
    let entries = missing.into_iter().collect::<Vec<_>>();
    for (index, chunk) in entries.chunks(batch).enumerate() {
        tracing::info!(
            "AI 翻译批次开始：用途 {purpose_name}，批次 {}/{}，条目 {} 条",
            index + 1,
            entries.len().div_ceil(batch),
            chunk.len()
        );
        let terms = chunk
            .iter()
            .map(|(_, source)| source.clone())
            .collect::<Vec<_>>();
        let payload = if purpose == "quality_mode" {
            let actors = terms
                .iter()
                .map(|term| {
                    (
                        term.clone(),
                        actors[term].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            json!({"context":context,"terms":terms,"actors":actors})
        } else if matches!(purpose, "fast_mode" | "transliterate_mode") {
            json!(terms)
        } else {
            json!(chunk.iter().cloned().collect::<BTreeMap<_, _>>())
        };
        let mut rate = if purpose.ends_with("_mode") {
            Some(state.ai.last_request.lock().await)
        } else {
            None
        };
        if let Some(last) = rate.as_ref().and_then(|r| **r) {
            let interval = Duration::from_secs_f64(
                config["ai_name_role_request_interval"]
                    .as_f64()
                    .filter(|n| n.is_finite() && *n >= 0.0)
                    .unwrap_or(3.0),
            );
            if let Some(remaining) = interval.checked_sub(last.elapsed()) {
                tracing::debug!(
                    "AI 翻译请求间隔等待：用途 {purpose_name}，{} 毫秒",
                    remaining.as_millis()
                );
                tokio::time::sleep(remaining).await;
            }
        }
        let response = request(
            state,
            &config,
            &system,
            &python_json(&payload),
            if purpose == "batch_joke_fallback" {
                0.8
            } else if purpose.ends_with("_mode") {
                0.0
            } else {
                0.3
            },
        )
        .await;
        if let Some(rate) = rate.as_mut() {
            **rate = Some(std::time::Instant::now());
        }
        drop(rate);
        if let Ok(response) = &response {
            let before = output.len();
            for (id, source) in chunk {
                let lookup = if purpose.ends_with("_mode") {
                    source
                } else {
                    id
                };
                if let Some(value) = response[lookup].as_str() {
                    let value = simplified(value).trim().to_string();
                    let parts =
                        |s: &str| s.split(['/', '|']).filter(|s| !s.trim().is_empty()).count();
                    let dots = value
                        .chars()
                        .collect::<Vec<_>>()
                        .windows(2)
                        .any(|pair| pair.iter().all(|c| ['·', '•'].contains(c)));
                    let quality = (purpose != "quality_mode"
                        || (!dots && (parts(source) <= 1 || parts(source) == parts(&value))))
                        && (purpose != "batch_joke_fallback"
                            || value.starts_with("【老六占位简介】"));
                    if value != *source
                        && value
                            .chars()
                            .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
                        && quality
                    {
                        let _gate = state.ai.cache_gate.read().await;
                        if persist
                            && revision
                                == state
                                    .ai
                                    .cache_revision
                                    .load(std::sync::atomic::Ordering::SeqCst)
                        {
                            memory.1.insert(keys[id].clone(), value.clone());
                            match sqlx::query("INSERT INTO translation_cache(original_text,translated_text,engine_used) VALUES($1,$2,$3) ON CONFLICT(original_text) DO UPDATE SET translated_text=EXCLUDED.translated_text,engine_used=EXCLUDED.engine_used,last_updated_at=NOW()")
                            .bind(&keys[id]).bind(&value).bind(string(&config,"ai_provider")).execute(&state.db).await {
                                Ok(_) => tracing::debug!("AI 翻译结果缓存已写入：用途 {purpose_name}，批次 {}", index + 1),
                                Err(error) => tracing::warn!("保存翻译结果缓存失败，本次进程内仍可复用：{error}"),
                            }
                        } else {
                            tracing::debug!(
                                "AI 翻译结果缓存跳过：用途 {purpose_name}，批次 {}，预览或缓存版本已变化",
                                index + 1
                            );
                        }
                        output.insert(id.clone(), value);
                    } else {
                        tracing::debug!(
                            "AI 翻译结果拒绝：用途 {purpose_name}，批次 {}，原词不变、缺少中文或质量检查未通过",
                            index + 1
                        );
                    }
                } else {
                    tracing::debug!(
                        "AI 翻译结果缺失：用途 {purpose_name}，批次 {}，没有对应文本",
                        index + 1
                    );
                }
            }
            tracing::info!(
                "AI 翻译批次完成：用途 {purpose_name}，批次 {}，接受 {} 条，拒绝或缺失 {} 条",
                index + 1,
                output.len() - before,
                chunk.len() - (output.len() - before)
            );
        } else {
            tracing::warn!(
                "AI 翻译批次失败，保留此前完成的结果：用途 {purpose_name}，批次 {}，{}",
                index + 1,
                response.unwrap_err().1
            );
        }
    }
    tracing::info!(
        "AI 翻译完成：用途 {purpose_name}，可用 {} 条，未完成 {} 条",
        output.len(),
        inputs.len() - output.len()
    );
    Ok(output)
}
