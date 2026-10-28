use crate::{
    AppState, db,
    error::{ApiError, Result},
    media_edit::list,
    state::string,
};
use ab_glyph::FontArc;
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::path::PathBuf;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/config/cover_generator", get(config).post(save))
        .route("/api/config/cover_generator/libraries", get(libraries))
        .route("/api/config/cover_generator/preview", post(preview))
        .layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024))
}
pub fn defaults() -> Value {
    serde_json::from_str(include_str!("../assets/cover_defaults.json")).unwrap()
}
async fn config(State(state): State<AppState>) -> Result<Json<Value>> {
    let mut config = defaults();
    if let Some(stored) = db::setting(&state.db, "cover_generator_config").await?
        && let Some(stored) = stored.as_object()
    {
        config.as_object_mut().unwrap().extend(stored.clone());
    }
    Ok(Json(config))
}
async fn save(State(state): State<AppState>, Json(config): Json<Value>) -> Result<Json<Value>> {
    if !config.is_object() {
        return Err(ApiError::bad("封面配置必须是对象"));
    }
    db::save_setting(&state.db, "cover_generator_config", &config).await?;
    Ok(Json(json!({"message":"配置已成功保存！"})))
}
async fn libraries(State(state): State<AppState>) -> Result<Json<Value>> {
    let user = string(&*state.config.read().await, "emby_user_id");
    let views = state
        .emby(
            reqwest::Method::GET,
            &format!("emby/Users/{user}/Views"),
            &[],
            None,
        )
        .await?;
    Ok(Json(json!(
        list(&views["Items"])
            .iter()
            .filter(|l| !string(l, "Name").is_empty() && !string(l, "Id").is_empty())
            .map(|l| json!({"label":l["Name"],"value":l["Id"]}))
            .collect::<Vec<_>>()
    )))
}
async fn font(state: &AppState, config: &Value, language: &str, multi: bool) -> Result<FontArc> {
    let base = format!("{language}_font");
    let name = format!("{base}{}", if multi { "_multi_1" } else { "" });
    let local = string(config, &format!("{name}_path_local"));
    // Existing configuration uses *_font_path_multi_1_local for multi fonts.
    let local = if multi {
        let alternate = string(config, &format!("{base}_path_multi_1_local"));
        if alternate.is_empty() {
            local
        } else {
            alternate
        }
    } else {
        local
    };
    if !local.is_empty()
        && let Ok(bytes) = tokio::fs::read(local).await
        && let Ok(font) = decode_font(bytes)
    {
        return Ok(font);
    }
    let cached = state.data_dir.join("cover_generator/fonts").join(format!(
        "{name}.{}",
        if multi && language == "en" {
            "otf"
        } else {
            "ttf"
        }
    ));
    if let Ok(bytes) = tokio::fs::read(&cached).await
        && let Ok(font) = decode_font(bytes)
    {
        return Ok(font);
    }
    let url = string(
        config,
        &format!("{base}_url{}", if multi { "_multi_1" } else { "" }),
    );
    if !url.is_empty() {
        let bytes = state
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        let font = decode_font(bytes.to_vec())?;
        tokio::fs::create_dir_all(cached.parent().unwrap()).await?;
        tokio::fs::write(cached, bytes).await?;
        return Ok(font);
    }
    let bytes: &[u8] = match (language, multi) {
        ("zh", false) => include_bytes!("../../fonts/zh_font.ttf"),
        ("en", false) => include_bytes!("../../fonts/en_font.ttf"),
        ("zh", true) => include_bytes!("../../fonts/zh_font_multi_1.ttf"),
        _ => include_bytes!("../../fonts/en_font_multi_1.otf"),
    };
    decode_font(bytes.to_vec())
}
pub fn decode_font(bytes: Vec<u8>) -> Result<FontArc> {
    let bytes = if bytes.starts_with(b"wOF2") {
        woff2_patched::convert_woff2_to_ttf(&mut std::io::Cursor::new(bytes))
            .map_err(|_| ApiError::bad("WOFF2字体解压失败"))?
    } else {
        bytes
    };
    FontArc::try_from_vec(bytes).map_err(|_| ApiError::bad("字体文件无效"))
}
async fn preview(State(state): State<AppState>, Json(body): Json<Value>) -> Result<Json<Value>> {
    let encoded = body["base_image"]
        .as_str()
        .ok_or_else(|| ApiError::bad("缺少基础图片数据"))?;
    let encoded = encoded.split_once(',').map_or(encoded, |(_, s)| s);
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| ApiError::bad("图片Base64无效"))?;
    let font = font(&state, &defaults(), "en", false).await?;
    let count = body["item_count"].as_i64().unwrap_or(99);
    let style = body["badge_style"].as_str().unwrap_or("badge").to_string();
    let ratio = body["badge_size_ratio"].as_f64().unwrap_or(0.12) as f32;
    let bytes = tokio::task::spawn_blocking(move || {
        let mut image = image::load_from_memory(&bytes)
            .map_err(|_| ApiError::bad("图片无效"))?
            .to_rgba8();
        crate::cover_render::badge(&mut image, count, &font, &style, ratio);
        let mut bytes = std::io::Cursor::new(vec![]);
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .map_err(|_| ApiError::bad("图片编码失败"))?;
        Ok::<_, ApiError>(bytes.into_inner())
    })
    .await
    .map_err(|_| ApiError::bad("图片生成失败"))??;
    Ok(Json(
        json!({"image":format!("data:image/png;base64,{}",STANDARD.encode(bytes))}),
    ))
}
pub fn title(config: &Value, name: &str) -> [String; 2] {
    if let Ok(value) = serde_yaml::from_str::<Value>(&string(config, "title_config"))
        && let Some(titles) = value[name].as_array().filter(|v| v.len() >= 2)
    {
        return [
            titles[0].as_str().unwrap_or(name).into(),
            titles[1].as_str().unwrap_or("").into(),
        ];
    }
    [name.into(), String::new()]
}
pub async fn generate(
    state: &AppState,
    library_ids: Option<&[String]>,
    context: &crate::jobs::JobContext,
) -> Result<Value> {
    let Some(config) = db::setting(&state.db, "cover_generator_config").await? else {
        return Err(ApiError::bad("未找到封面生成器配置"));
    };
    if config["enabled"] != true {
        return Ok(json!({"updated":0,"message":"封面生成器未启用"}));
    }
    context
        .activity("读取媒体库", "正在获取需要生成封面的媒体库列表")
        .await;
    let user = string(&*state.config.read().await, "emby_user_id");
    let views = state
        .emby(
            reqwest::Method::GET,
            &format!("emby/Users/{user}/Views"),
            &[],
            None,
        )
        .await?;
    let multi = config["cover_style"] == "multi_1";
    let multi_fonts = multi && config["multi_1_use_main_font"] != true;
    let fonts = [
        font(state, &config, "zh", multi_fonts).await?,
        font(state, &config, "en", multi_fonts).await?,
    ];
    let mut updated = 0;
    let mut failures = vec![];
    for library in list(&views["Items"]) {
        let id = string(&library, "Id");
        let name = string(&library, "Name");
        if id.is_empty()
            || list(&config["exclude_libraries"]).contains(&json!(id))
            || library_ids.is_some_and(|ids| !ids.contains(&id))
        {
            continue;
        }
        if !matches!(
            library["CollectionType"].as_str(),
            Some("movies" | "tvshows" | "boxsets" | "mixed" | "music" | "audiobooks")
        ) && library["Type"] != "CollectionFolder"
        {
            continue;
        }
        context.checkpoint().await?;
        context
            .activity(
                "生成媒体库封面",
                &format!("正在处理媒体库《{name}》，已完成 {updated} 个"),
            )
            .await;
        let result = async {
            let mut paths = vec![];
            let custom_root=string(&config,"covers_input");
            let custom = PathBuf::from(&custom_root).join(&name);
            if !custom_root.is_empty()&&custom.is_dir() {
                let mut entries = tokio::fs::read_dir(custom).await?;
                while let Some(entry) = entries.next_entry().await? {
                    if entry.file_type().await?.is_file()
                        && entry.path().extension().is_some_and(|e| {
                            ["jpg", "jpeg", "png"]
                                .contains(&e.to_string_lossy().to_lowercase().as_str())
                        })
                    {
                        paths.push(entry.path());
                    }
                }
                paths.sort();
            }
            let kind = match library["CollectionType"].as_str() {
                Some("movies") => "Movie",
                Some("tvshows") => "Series",
                Some("music") => "MusicAlbum",
                Some("audiobooks") => "AudioBook",
                _ => "Movie,Series",
            };
            let whitelist=["R级","限制","成人","Adult","Porn","18+"].iter().any(|s|name.to_lowercase().contains(&s.to_lowercase()));
            let max=config["max_safe_rating"].as_i64().unwrap_or(8);
            let cached:Vec<Value>=if paths.is_empty() {
                match sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE in_library=true AND EXISTS(SELECT 1 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(asset_details_json)='array' THEN asset_details_json ELSE '[]' END) a WHERE a->>'source_library_id'=$1) AND item_type=ANY(string_to_array($2,',')) ORDER BY CASE WHEN $3 THEN random() END, date_added DESC").bind(&id).bind(kind).bind(config["sort_by"]=="Random").fetch_all(&state.db).await {
                    Ok(rows)=>rows,
                    Err(error)=>{tracing::warn!("[封面生成] 媒体库《{name}》查询图片缓存失败，回退 Emby 图片选择：{}", error);vec![]}
                }
            }else{vec![]};
            let rating_mapping=db::setting(&state.db,"rating_mapping").await?.unwrap_or_else(||serde_json::from_str::<Value>(include_str!("../assets/defaults.json")).unwrap()["DEFAULT_RATING_MAPPING"].clone());
            let selected=cached.iter().filter(|m|whitelist||max>=999||rating(m,&rating_mapping)<=max).filter_map(|m|list(&m["emby_item_ids_json"]).first().and_then(|v|v.as_str()).map(str::to_string)).take(if multi{9}else{1}).collect::<Vec<_>>();
            let mut query=vec![("ParentId",id.clone()),("Recursive","true".into()),("IncludeItemTypes",kind.into()),("Limit",if multi{"45"}else{"5"}.into()),("SortBy",if config["sort_by"]=="Random"{"Random"}else{"DateCreated"}.into()),("SortOrder","Descending".into()),("Fields","ImageTags,BackdropImageTags,PrimaryImageTag,PrimaryImageItemId".into())];
            if !selected.is_empty(){query=vec![("Ids",selected.join(",")),("Fields","Id,Name,Type,ImageTags,BackdropImageTags,PrimaryImageTag,PrimaryImageItemId".into())];}else if config["sort_by"]=="Random"{query.retain(|(key,_)|*key!="SortOrder");}
            let items = if paths.is_empty() {
                if selected.is_empty() {
                    state.emby(reqwest::Method::GET,&format!("Users/{user}/Items"),&query,None).await?
                } else {
                    let app_config = state.config.read().await.clone();
                    state.http.get(format!("{}/Users/{user}/Items",string(&app_config,"emby_server_url").trim_end_matches('/')))
                        .header("X-Emby-Token",string(&app_config,"emby_api_key")).query(&query)
                        .timeout(std::time::Duration::from_secs(30)).send().await?.error_for_status()?.json().await?
                }
            } else { json!({"Items":[]}) };
            let count=state.emby(reqwest::Method::GET,&format!("Users/{user}/Items"),&[("ParentId",id.clone()),("Recursive","true".into()),("IncludeItemTypes",kind.into()),("Limit","0".into())],None).await?;
            let total = count["TotalRecordCount"].as_i64().unwrap_or(0);
            let folder = state.data_dir.join("cover_generator/covers").join(&name);
            tokio::fs::create_dir_all(&folder).await?;
            if paths.is_empty() {
                let mut items=list(&items["Items"]);
                if config["sort_by"]=="Random"{use rand::seq::SliceRandom;items.shuffle(&mut rand::thread_rng());}
                for (index,item) in items.into_iter().filter(|item|image_path(item,&config,multi).is_some()).take(if multi{9}else{1}).enumerate() {
                    context.activity("读取封面来源", &format!("媒体库《{name}》正在读取《{}》的封面（第 {} 张）", string(&item, "Name"), index + 1)).await;
                    let image_path=image_path(&item,&config,multi).unwrap();
                    let app_config = state.config.read().await.clone();
                    let response = crate::emby::send(state
                        .http
                        .get(format!(
                            "{}{image_path}",
                            string(&app_config, "emby_server_url").trim_end_matches('/')
                        ))
                        .header("X-Emby-Token", string(&app_config, "emby_api_key")))
                        .await?;
                    if !response.status().is_success() {
                        continue;
                    }
                    let path = folder.join(format!("{}.jpg", index + 1));
                    tokio::fs::write(&path, response.bytes().await?).await?;
                    paths.push(path);
                    if paths.len() == if multi { 9 } else { 1 } {
                        break;
                    }
                }
            }
            if multi && !paths.is_empty() {
                use rand::seq::SliceRandom;
                for index in 1..=9 {
                    let destination=folder.join(format!("{index}.jpg"));
                    if !destination.is_file() {
                        let source=paths.choose(&mut rand::thread_rng()).unwrap();
                        tokio::fs::copy(source,&destination).await?;
                    }
                }
                paths=(1..=9).map(|i|folder.join(format!("{i}.jpg"))).collect();
            }
            let mut images = vec![];
            for path in paths {
                let bytes = tokio::fs::read(path).await?;
                let image=image::load_from_memory(&bytes).map_err(|_|ApiError::bad("封面来源图片无效"))?;
                images.push(image.to_rgba8());
            }
            let title = title(&config, &name);
            context.activity("渲染封面", &format!("正在渲染媒体库《{name}》的封面")).await;
            let render_config = config.clone();
            let fonts = fonts.clone();
            let bytes = tokio::task::spawn_blocking(move || {
                crate::cover_render::render(&images, &title, &fonts, &render_config, total)
            })
            .await
            .map_err(|_| ApiError::bad("封面渲染失败"))??;
            let output = string(&config, "covers_output");
            if !output.is_empty() {
                let saved = async {
                    tokio::fs::create_dir_all(&output).await?;
                    tokio::fs::write(PathBuf::from(output).join(format!("{name}.jpg")), &bytes).await
                }.await;
                if let Err(error) = saved { tracing::error!("另存封面失败：{error}，继续上传媒体库《{name}》封面。"); }
            }
            let app_config = state.config.read().await.clone();
            context.activity("上传封面", &format!("正在将媒体库《{name}》的封面上传到 Emby")).await;
            state
                .http
                .post(format!(
                    "{}/Items/{id}/Images/Primary",
                    string(&app_config, "emby_server_url").trim_end_matches('/')
                ))
                .header("X-Emby-Token", string(&app_config, "emby_api_key"))
                .header("Content-Type", "image/jpeg")
                .body(bytes)
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await?
                .error_for_status()?;
            Ok::<_, ApiError>(())
        }
        .await;
        match result {
            Ok(()) => {
                updated += 1;
                tracing::info!("[封面生成] 媒体库《{name}》封面已生成并上传");
            }
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) => {
                tracing::warn!("[封面生成] 媒体库《{name}》生成失败：{}", error.1);
                failures.push(format!("{name}: {}", error.1));
            }
        }
    }
    context
        .update(
            100,
            "封面生成",
            &format!(
                "封面生成完成：成功 {updated} 个媒体库，失败 {} 个",
                failures.len()
            ),
        )
        .await;
    Ok(
        json!({"updated":updated,"failures":failures,"message":format!("封面生成完成：成功 {updated} 个媒体库，失败 {} 个", failures.len())}),
    )
}
fn rating(row: &Value, mapping: &Value) -> i64 {
    let value = row["custom_rating"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| row["official_rating_json"]["US"].as_str())
        .unwrap_or("");
    for rules in mapping.as_object().into_iter().flat_map(|o| o.values()) {
        for rule in list(rules) {
            if rule["code"] == value {
                return rule["emby_value"].as_i64().unwrap_or(0);
            }
        }
    }
    value
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

pub fn image_path(item: &Value, config: &Value, multi: bool) -> Option<String> {
    let id = item["Id"].as_str().filter(|s| !s.is_empty())?;
    let primary = if let Some(tag) = item["ImageTags"]["Primary"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        Some(format!("/Items/{id}/Images/Primary?tag={tag}"))
    } else {
        item["PrimaryImageItemId"]
            .as_str()
            .zip(item["PrimaryImageTag"].as_str())
            .filter(|(id, tag)| !id.is_empty() && !tag.is_empty())
            .map(|(id, tag)| format!("/Items/{id}/Images/Primary?tag={tag}"))
    };
    let backdrop = list(&item["BackdropImageTags"])
        .first()
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|tag| format!("/Items/{id}/Images/Backdrop/0?tag={tag}"));
    let use_primary = if multi {
        config["multi_1_use_primary"] == true
    } else {
        config["single_use_primary"] == true
    };
    if use_primary {
        primary.or(backdrop)
    } else {
        backdrop.or(primary)
    }
}
