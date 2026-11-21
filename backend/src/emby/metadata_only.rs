use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::{details, list},
    state::string,
};
use quick_xml::{Reader, events::Event};
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

// ItemUpdateService.UpdateItem 会读这些字段；发送完整旧 DTO 保留非编辑值。
const FIELDS: &str = "Settings,SortName,Id,Type,Path,ParentId,SeriesId,Size,Container,Name,OriginalTitle,Overview,LockData,LockedFields,DisplayOrder,Status,ForcedSortName,CriticRating,CommunityRating,IndexNumber,ParentIndexNumber,SortParentIndexNumber,SortIndexNumber,TagItems,Tags,GenreItems,Genres,Taglines,Studios,People,DateCreated,EndDate,PremiereDate,ProductionYear,OfficialRating,CustomRating,ProductionLocations,PreferredMetadataCountryCode,PreferredMetadataLanguage,RunTimeTicks,ProviderIds,Video3DFormat,MediaStreams,MediaSources,Chapters";
const WRITTEN: &[&str] = &[
    "Name",
    "OriginalTitle",
    "Overview",
    "LockData",
    "LockedFields",
    "DisplayOrder",
    "Status",
    "ForcedSortName",
    "CriticRating",
    "CommunityRating",
    "IndexNumber",
    "ParentIndexNumber",
    "SortParentIndexNumber",
    "SortIndexNumber",
    "TagItems",
    "Tags",
    "GenreItems",
    "Genres",
    "Taglines",
    "Studios",
    "People",
    "DateCreated",
    "EndDate",
    "PremiereDate",
    "ProductionYear",
    "OfficialRating",
    "CustomRating",
    "ProductionLocations",
    "PreferredMetadataCountryCode",
    "PreferredMetadataLanguage",
    "RunTimeTicks",
    "ProviderIds",
    "Video3DFormat",
];
const MEDIA: &[&str] = &[
    "Id",
    "Type",
    "Path",
    "ParentId",
    "SeriesId",
    "Size",
    "RunTimeTicks",
    "Container",
    "MediaStreams",
    "MediaSources",
    "Chapters",
];

#[derive(Default)]
struct Element {
    name: String,
    attributes: BTreeMap<String, String>,
    text: String,
    children: Vec<Element>,
}
impl Element {
    fn first(&self, name: &str) -> Option<&str> {
        self.children
            .iter()
            .find(|value| value.name == name)
            .map(|value| value.text.trim())
    }
    fn all(&self, name: &str) -> Vec<&Element> {
        self.children
            .iter()
            .filter(|value| value.name == name)
            .collect()
    }
}

fn parse(xml: &str) -> Result<Element> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<Element> = vec![];
    let mut root = None;
    loop {
        let event = reader
            .read_event()
            .map_err(|error| ApiError::bad(format!("NFO XML 无效：{error}")))?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(value) | Event::Empty(value) => {
                let mut element = Element {
                    name: String::from_utf8_lossy(value.name().as_ref()).to_lowercase(),
                    ..Default::default()
                };
                for attribute in value.attributes() {
                    let attribute = attribute.map_err(|_| ApiError::bad("NFO 属性无效"))?;
                    let text = attribute
                        .decode_and_unescape_value(reader.decoder())
                        .map_err(|_| ApiError::bad("NFO 属性编码无效"))?;
                    element.attributes.insert(
                        String::from_utf8_lossy(attribute.key.as_ref()).to_lowercase(),
                        text.into_owned(),
                    );
                }
                if empty {
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(element);
                    } else if root.replace(element).is_some() {
                        return Err(ApiError::bad("NFO 包含多个根节点"));
                    }
                } else {
                    stack.push(element);
                }
            }
            Event::End(_) => {
                let element = stack.pop().ok_or_else(|| ApiError::bad("NFO 标签不匹配"))?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(element);
                } else if root.replace(element).is_some() {
                    return Err(ApiError::bad("NFO 包含多个根节点"));
                }
            }
            Event::Text(value) => {
                if let Some(element) = stack.last_mut() {
                    let text = value
                        .decode()
                        .map_err(|_| ApiError::bad("NFO 文本编码无效"))?;
                    element.text.push_str(
                        &quick_xml::escape::unescape(&text)
                            .map_err(|_| ApiError::bad("NFO 实体无效"))?,
                    );
                }
            }
            Event::CData(value) => {
                if let Some(element) = stack.last_mut() {
                    element.text.push_str(
                        &value
                            .decode()
                            .map_err(|_| ApiError::bad("NFO 文本编码无效"))?,
                    );
                }
            }
            Event::GeneralRef(value) => {
                let name = value
                    .decode()
                    .map_err(|_| ApiError::bad("NFO 实体编码无效"))?;
                let reference = format!("&{name};");
                let text = quick_xml::escape::unescape(&reference)
                    .map_err(|_| ApiError::bad("NFO 实体无效"))?;
                if let Some(element) = stack.last_mut() {
                    element.text.push_str(&text);
                }
            }
            Event::DocType(_) => return Err(ApiError::bad("NFO 不支持 DTD 声明")),
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(ApiError::bad("NFO 标签未结束"));
    }
    root.ok_or_else(|| ApiError::bad("NFO 没有根节点"))
}

/// Reuse the existing XML reader for ingestion and translation preservation.
pub(super) fn scrape_metadata(xml: &str, kind: &str) -> Result<Value> {
    let root = parse(xml)?;
    let expected = match kind {
        "Movie" => "movie", "Series" => "tvshow", "Season" => "season", "Episode" => "episodedetails",
        _ => return Err(ApiError::bad("不支持的 NFO 类型")),
    };
    if root.name != expected { return Err(ApiError::bad("NFO 类型与当前媒体不一致")); }
    let mut data = json!({});
    for (tag, key) in [("title", "name"), ("originaltitle", "original_name"), ("plot", "overview"), ("tagline", "tagline"), ("premiered", "air_date"), ("mpaa", "mpaa")] {
        if let Some(value) = root.first(tag) { data[key] = json!(value); }
    }
    let id = root.all("uniqueid").into_iter().find(|node| node.attributes.get("type").is_some_and(|kind| kind.eq_ignore_ascii_case("tmdb")))
        .map(|node| node.text.trim()).or_else(|| root.first("tmdbid"));
    if let Some(id) = id.and_then(|id| id.parse::<u64>().ok()).filter(|id| *id > 0) { data["id"] = json!(id); }
    for (tag, key) in [("season", "season_number"), ("episode", "episode_number"), ("runtime", "runtime")] {
        if let Some(number) = root.first(tag).and_then(|value| value.parse::<u64>().ok()) { data[key] = json!(number); }
    }
    let cast = root.all("actor").into_iter().filter_map(|actor| {
        if actor.first("type").is_some_and(|kind|kind.eq_ignore_ascii_case("director")) { return None; }
        let id = actor.first("tmdbid").and_then(|id| id.parse::<u64>().ok())?;
        Some(json!({"id":id,"name":actor.first("name"),"character":actor.first("role"),
            "order":actor.first("order").and_then(|order|order.parse::<i64>().ok()).unwrap_or(999),"profile_path":actor.first("thumb")}))
    }).collect::<Vec<_>>();
    if !cast.is_empty() { data["credits"]["cast"] = json!(cast); }
    let directors = root.all("director").into_iter().filter_map(|director| {
        let id = director.attributes.get("tmdbid")?.parse::<u64>().ok()?;
        Some(json!({"id":id,"name":director.text.trim(),"job":"Director"}))
    }).collect::<Vec<_>>();
    if !directors.is_empty() { data["credits"]["crew"] = json!(directors); }
    for (tag, key) in [("genre", "genres"), ("studio", "production_companies")] {
        let values = root.all(tag).into_iter().map(|value|json!({"id":0,"name":value.text.trim()})).collect::<Vec<_>>();
        if !values.is_empty() { data[key] = json!(values); }
    }
    Ok(data)
}

fn date(value: &str) -> Result<Value> {
    if value.is_empty() {
        return Ok(Value::Null);
    }
    if let Ok(value) = chrono::DateTime::parse_from_rfc3339(value) {
        return Ok(json!(value.to_utc().to_rfc3339()));
    }
    let value = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| ApiError::bad("NFO 日期格式无效"))?;
    Ok(json!(
        value.and_hms_opt(0, 0, 0).unwrap().and_utc().to_rfc3339()
    ))
}

fn names(value: &Value) -> Value {
    let mut values = list(value)
        .iter()
        .map(|item| {
            item.as_str()
                .unwrap_or_else(|| item["Name"].as_str().unwrap_or(""))
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    json!(values)
}
fn comparable(field: &str, value: &Value) -> Value {
    match field {
        "Studios" | "Genres" | "Tags" | "GenreItems" | "TagItems" => names(value),
        "Taglines" => json!(list(value).first().and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty()).into_iter().collect::<Vec<_>>()),
        "CommunityRating" | "CriticRating" => value.as_f64().map(|value|json!(value as f32)).unwrap_or_else(||value.clone()),
        "DateCreated" | "EndDate" | "PremiereDate" => value.as_str().and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok()).map(|value|json!(value.to_utc())).unwrap_or_else(||value.clone()),
        "People" => json!(list(value).iter().map(|person|json!({"Name":comparable("Name",&person["Name"]),"Type":person["Type"],"Role":comparable("Overview",&person["Role"])})).collect::<Vec<_>>()),
        "Name" | "OriginalTitle" | "Overview" | "ForcedSortName" | "OfficialRating" | "CustomRating" | "PreferredMetadataCountryCode" | "PreferredMetadataLanguage" => value.as_str().map(|text| if text.trim().is_empty() {Value::Null} else {json!(text.trim())}).unwrap_or_else(||value.clone()),
        _ => value.clone(),
    }
}

fn project(current: &Value, xml: &str) -> Result<(Value, Vec<String>)> {
    if !current["LockData"].is_boolean()
        || current["Type"] == "Series"
            && current["DisplayOrder"].as_str().is_none_or(|order| {
                !["Aired", "Dvd", "Absolute"]
                    .iter()
                    .any(|known| order.eq_ignore_ascii_case(known))
            })
    {
        return Err(ApiError::bad(
            "原生锁定状态或剧集显示顺序未确认，未提交可能影响子项的元数据更新",
        ));
    }
    let root = parse(xml)?;
    let expected = match current["Type"].as_str() {
        Some("Movie") => "movie",
        Some("Series") => "tvshow",
        Some("Season") => "season",
        Some("Episode") => "episodedetails",
        _ => return Err(ApiError::bad("仅支持明确的影视元数据目标")),
    };
    if root.name != expected {
        return Err(ApiError::bad("NFO 类型与原生媒体目标不一致"));
    }
    let mut payload = current.clone();
    for (tag, field) in [
        ("title", "Name"),
        ("originaltitle", "OriginalTitle"),
        ("plot", "Overview"),
        ("mpaa", "OfficialRating"),
        ("customrating", "CustomRating"),
        ("sorttitle", "ForcedSortName"),
    ] {
        if let Some(value) = root.first(tag) {
            payload[field] = json!(value);
        }
    }
    for (tag, field) in [
        ("year", "ProductionYear"),
        ("rating", "CommunityRating"),
        ("criticrating", "CriticRating"),
    ] {
        if let Some(value) = root.first(tag) {
            payload[field] = if value.is_empty() {
                Value::Null
            } else if tag == "year" {
                json!(
                    value
                        .parse::<i64>()
                        .map_err(|_| ApiError::bad("NFO 年份无效"))?
                )
            } else {
                let number = value
                    .parse::<f64>()
                    .map_err(|_| ApiError::bad("NFO 评分无效"))?;
                if !number.is_finite() {
                    return Err(ApiError::bad("NFO 评分无效"));
                }
                json!(number)
            };
        }
    }
    for (tag, field) in [("premiered", "PremiereDate"), ("enddate", "EndDate")] {
        if let Some(value) = root.first(tag) {
            payload[field] = date(value)?;
        }
    }
    if current["Type"] == "Series" {
        if let Some(status) = root.first("status") {
            for known in ["Continuing", "Ended"] {
                if status.eq_ignore_ascii_case(known) {
                    payload["Status"] = json!(known);
                }
            }
        }
    }
    for (tag, field, ids) in [
        ("genre", "Genres", Some("GenreItems")),
        ("tag", "Tags", Some("TagItems")),
        ("tagline", "Taglines", None),
    ] {
        let elements = root.all(tag);
        if !elements.is_empty() {
            payload[field] = json!(
                elements
                    .iter()
                    .take(if field == "Taglines" {
                        1
                    } else {
                        elements.len()
                    })
                    .map(|value| value.text.trim())
                    .collect::<Vec<_>>()
            );
            if let Some(ids) = ids {
                payload[ids] = Value::Null;
            }
        }
    }
    let studios = root.all("studio");
    if !studios.is_empty() {
        let previous = list(&current["Studios"]);
        payload["Studios"] = json!(
            studios
                .iter()
                .map(|studio| {
                    let name = studio.text.trim();
                    previous
                        .iter()
                        .find(|value| value["Name"] == name)
                        .cloned()
                        .unwrap_or_else(|| json!({"Name":name}))
                })
                .collect::<Vec<_>>()
        );
    }
    if current["Type"] == "Episode" || current["Type"] == "Season" {
        for (tag, field) in if current["Type"] == "Episode" {
            vec![
                ("season", "ParentIndexNumber"),
                ("episode", "IndexNumber"),
                ("displayseason", "SortParentIndexNumber"),
                ("displayepisode", "SortIndexNumber"),
            ]
        } else {
            vec![("seasonnumber", "IndexNumber")]
        } {
            if let Some(value) = root.first(tag) {
                payload[field] = json!(
                    value
                        .parse::<i64>()
                        .map_err(|_| ApiError::bad("NFO 季集号无效"))?
                );
            }
        }
    }
    let mut providers = current["ProviderIds"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    for element in root.all("uniqueid") {
        let name = element
            .attributes
            .get("type")
            .ok_or_else(|| ApiError::bad("NFO 标识缺少来源类型"))?;
        let field = match name.to_lowercase().as_str() {
            "tmdb" => "Tmdb",
            "imdb" => "Imdb",
            "tvdb" => "Tvdb",
            "douban" => "Douban",
            _ => name,
        };
        let old = providers
            .keys()
            .find(|key| key.eq_ignore_ascii_case(field))
            .cloned();
        if let Some(old) = old {
            providers.remove(&old);
        }
        if !element.text.trim().is_empty() {
            providers.insert(field.into(), json!(element.text.trim()));
        }
    }
    for (tag, field) in [
        ("tmdbid", "Tmdb"),
        ("imdbid", "Imdb"),
        ("imdb_id", "Imdb"),
        ("tvdbid", "Tvdb"),
    ] {
        if let Some(value) = root.first(tag) {
            if !value.is_empty() {
                providers.insert(field.into(), json!(value));
            }
        }
    }
    payload["ProviderIds"] = json!(providers);
    let actors = root.all("actor");
    let directors = root.all("director");
    if !actors.is_empty() || !directors.is_empty() {
        let previous = list(&current["People"]);
        let mut people = vec![];
        for actor in actors {
            let name = actor
                .first("name")
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ApiError::bad("NFO 人物缺少姓名"))?;
            let kind = actor.first("type").unwrap_or("Actor");
            let tmdb = actor.first("tmdbid");
            let mut person = previous
                .iter()
                .find(|person| {
                    tmdb.is_some_and(|id| person["ProviderIds"]["Tmdb"] == id)
                        || person["Name"] == name && person["Type"] == kind
                })
                .cloned()
                .unwrap_or_else(|| json!({}));
            person["Name"] = json!(name);
            person["Type"] = json!(kind);
            if let Some(role) = actor.first("role") {
                person["Role"] = json!(role);
            }
            if let Some(order) = actor.first("order") {
                person["SortOrder"] = json!(
                    order
                        .parse::<i64>()
                        .map_err(|_| ApiError::bad("NFO 人物顺序无效"))?
                );
            }
            people.push(person);
        }
        for director in directors {
            let name = director.text.trim();
            if name.is_empty() {
                return Err(ApiError::bad("NFO 导演缺少姓名"));
            }
            if people
                .iter()
                .any(|person| person["Name"] == name && person["Type"] == "Director")
            {
                continue;
            }
            let mut person = previous
                .iter()
                .find(|person| person["Name"] == name && person["Type"] == "Director")
                .cloned()
                .unwrap_or_else(|| json!({}));
            person["Name"] = json!(name);
            person["Type"] = json!("Director");
            people.push(person);
        }
        people.sort_by_key(|person| person["SortOrder"].as_i64().unwrap_or(i64::MAX));
        payload["People"] = json!(people);
    }
    let changed = WRITTEN
        .iter()
        .copied()
        .filter(|field| {
            !matches!(*field, "GenreItems" | "TagItems")
                && comparable(field, &current[*field]) != comparable(field, &payload[*field])
        })
        .map(|field| field.to_string())
        .collect();
    Ok((payload, changed))
}

fn compatible(original: &Value, desired: &Value, current: &Value) -> bool {
    MEDIA
        .iter()
        .all(|field| current[*field] == original[*field])
        && WRITTEN
            .iter()
            .filter(|field| !matches!(**field, "GenreItems" | "TagItems"))
            .all(|field| {
                let current = comparable(field, &current[*field]);
                current == comparable(field, &original[*field])
                    || current == comparable(field, &desired[*field])
            })
}

/// 用定点元数据更新消费当前 NFO，不调用会展开关联版本的刷新接口。
async fn read_nfos(paths: &[PathBuf]) -> Result<String> {
    let mut value = None;
    for path in paths {
        let text = tokio::fs::read_to_string(path).await.map_err(|error| {
            ApiError::bad(format!(
                "本次已生成的 NFO 无法读取：{}（{error}）",
                path.display()
            ))
        })?;
        if value.as_ref().is_some_and(|value| value != &text) {
            return Err(ApiError::bad(
                "同季所选目录的 NFO 内容不一致，未猜选季元数据",
            ));
        }
        value = Some(text);
    }
    value.ok_or_else(|| ApiError::bad("本次元数据 NFO 范围为空，未猜测原更新内容"))
}

pub(crate) async fn apply(
    state: &AppState,
    target: &Value,
    context: &JobContext,
    nfo_paths: &[PathBuf],
) -> Result<Value> {
    context.checkpoint().await?;
    let id = string(target, "Id");
    let context = context.scoped(&format!("metadata-only:{id}"));
    let before = details(state, &id, FIELDS).await?;
    if ["Id", "Type", "Path"]
        .iter()
        .any(|field| before[*field] != target[*field])
    {
        return Err(ApiError::bad("元数据目标身份或路径已变化，未提交更新"));
    }
    // Emby writes the NFO back after Items/{id}. A later image retry must
    // resume after this completed step, not reinterpret that native write.
    if let Some(done) = context.completed("done").await? {
        let intent = context
            .plan("intent")
            .await?
            .ok_or_else(|| ApiError::bad("已完成元数据更新缺少原目标记录"))?;
        if MEDIA.iter().any(|field| before[*field] != intent["original"][*field]) {
            return Err(ApiError::bad("已完成元数据更新的媒体身份或资源已变化"));
        }
        return Ok(done);
    }
    let path = string(&before, "Path");
    if !Path::new(&path).is_absolute()
        || crate::media_save::protected(&*state.config.read().await, &path)
    {
        return Err(ApiError::bad("目标 NFO 未位于可确认的本地媒体路径"));
    }
    let native_nfo = match before["Type"].as_str() {
        Some("Series") => Path::new(&path).join("tvshow.nfo"),
        Some("Season") => Path::new(&path).join("season.nfo"),
        _ => Path::new(&path).with_extension("nfo"),
    };
    let nfos = if nfo_paths.is_empty() {
        vec![native_nfo]
    } else {
        nfo_paths.to_vec()
    };
    for nfo in &nfos {
        if !nfo.is_absolute()
            || crate::media_save::protected(&*state.config.read().await, nfo.to_str().unwrap_or(""))
        {
            return Err(ApiError::bad("本次 NFO 来源不属于可确认的本地元数据路径"));
        }
    }
    let xml = read_nfos(&nfos).await?;
    let media_json = matches!(before["Type"].as_str(), Some("Movie" | "Episode")).then(|| {
        Path::new(&path).with_file_name(format!(
            "{}-mediainfo.json",
            Path::new(&path)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
        ))
    });
    let media_bytes = if let Some(path) = &media_json {
        Some(tokio::fs::read(path).await?)
    } else {
        None
    };
    let intent = if let Some(intent) = context.plan("intent").await? {
        if intent["nfo"] != xml || intent["nfo_paths"] != json!(nfos) {
            return Err(ApiError::bad(
                "NFO 在元数据任务中断期间变化，未扩大原更新内容",
            ));
        }
        intent
    } else {
        let (desired, fields) = project(&before, &xml)?;
        let intent = json!({"original":before,"desired":desired,"fields":fields,"nfo":xml,"nfo_paths":nfos,
            "media_json":media_bytes.as_ref().map(|bytes|base64::Engine::encode(&base64::engine::general_purpose::STANDARD,bytes))});
        context.save_plan("intent", &intent).await?;
        intent
    };
    let original = &intent["original"];
    let original_media = intent["media_json"]
        .as_str()
        .map(|value| base64::Engine::decode(&base64::engine::general_purpose::STANDARD, value))
        .transpose()
        .map_err(|_| ApiError::bad("原媒体 JSON 保护记录无效，未提交元数据"))?;
    if original_media != media_bytes {
        return Err(ApiError::bad(
            "媒体 JSON 在元数据任务中断期间变化，未覆盖原更新意图",
        ));
    }
    let (payload, fields) = project(original, &xml)?;
    if payload != intent["desired"]
        || json!(fields) != intent["fields"]
        || !compatible(original, &payload, &before)
    {
        return Err(ApiError::bad(
            "原生元数据、媒体身份或原更新计划发生变化，未覆盖并发修改",
        ));
    }
    let changed = intent["fields"]
        .as_array()
        .ok_or_else(|| ApiError::bad("原元数据字段计划无效"))?;
    let needs_update = WRITTEN
        .iter()
        .filter(|field| !matches!(**field, "GenreItems" | "TagItems"))
        .any(|field| comparable(field, &before[*field]) != comparable(field, &payload[*field]));
    if needs_update {
        let latest = details(state, &id, FIELDS).await?;
        if !compatible(original, &payload, &latest) || read_nfos(&nfos).await? != xml {
            return Err(ApiError::bad(
                "NFO 或原生元数据在提交前变化，未覆盖并发修改",
            ));
        }
        context.checkpoint().await?;
        state
            .emby(
                Method::POST,
                &format!("Items/{id}"),
                &[],
                Some(payload.clone()),
            )
            .await?;
    }
    crate::emby::idle(state, &context).await?;
    let after = details(state, &id, FIELDS).await?;
    if let Some(path) = &media_json {
        if Some(tokio::fs::read(path).await?) != original_media {
            return Err(ApiError::bad(
                "定点元数据更新后媒体 JSON 被修改，未记录入库成功",
            ));
        }
    }
    if MEDIA.iter().any(|field| after[*field] != original[*field]) {
        return Err(ApiError::bad(
            "定点元数据更新后原媒体信息或身份发生变化，未报告入库成功",
        ));
    }
    let readback_differences = WRITTEN
        .iter()
        .copied()
        .filter(|field| {
            !(matches!(*field, "GenreItems" | "TagItems") && payload[*field].is_null())
                && comparable(field, &after[*field]) != comparable(field, &payload[*field])
        })
        .map(|field| field.to_string())
        .collect::<Vec<_>>();
    if !readback_differences.is_empty() {
        // Emby 会规范化排序名和人物列表；原入库流程以原生更新完成为准，
        // 不将 DTO 逐字段一致当作入库门槛。媒体身份和原资源仍在上方验收。
        tracing::debug!(
            "元数据定点更新已完成，Emby {id} 的原生回读差异：{}",
            readback_differences.join("、")
        );
    }
    let report = json!({"completed":true,"updated":usize::from(!changed.is_empty()),"fields":changed,"item_id":id,"method":"metadata_update_by_id","readback_differences":readback_differences});
    context.complete("done", &report).await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn completed_metadata_replay_ignores_rewritten_nfo_but_checks_media_identity() {
        use std::sync::Arc;
        let directory = tempfile::tempdir().unwrap();
        let target = json!({"Id":"episode","Type":"Episode","Path":directory.path().join("episode.strm"),"Size":123});
        let live = Arc::new(tokio::sync::RwLock::new(target.clone()));
        let app = axum::Router::new().route("/Users/user/Items/episode", axum::routing::get({
            let live=live.clone(); move || {let live=live.clone(); async move {axum::Json(live.read().await.clone())}}
        }));
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url=format!("http://{}",listener.local_addr().unwrap());
        let server=tokio::spawn(async move {axum::serve(listener,app).await.unwrap()});
        let db=sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap(); db.close().await;
        let state=AppState {
            db, config:Arc::new(tokio::sync::RwLock::new(json!({"emby_server_url":url,"emby_api_key":"mock","emby_user_id":"user"}))),
            http:reqwest::Client::new(), sessions:Arc::new(crate::auth::SessionSigner::load(directory.path()).unwrap()),
            recovery_tokens:Arc::default(), data_dir:directory.path().into(), notifications:Arc::default(),webhooks:Arc::default(),
            ai:Arc::default(),jobs:Arc::default(),logs:crate::logs::LogStore::open(&directory.path().join("logs")).unwrap(),
            p115:Arc::default(),strm:Arc::default(),douban:Arc::default(),docker:Arc::default(),
        };
        let work=state.clone();
        let task=state.jobs.submit("元数据完成后重试",move |context| async move {
            let scope=context.scoped("metadata-only:episode");
            let report=json!({"completed":true,"item_id":"episode"});
            scope.save_plan("intent",&json!({"original":target,"nfo":"original NFO"})).await?;
            scope.complete("done",&report).await?;
            // The native NFO need not even remain readable after completion.
            assert_eq!(apply(&work,&target,&context,&[]).await?,report);
            live.write().await["Size"]=json!(456);
            assert!(apply(&work,&target,&context,&[]).await.is_err());
            Ok(json!({"verified":true}))
        }).await.unwrap();
        let result=tokio::time::timeout(std::time::Duration::from_secs(5),async {
            loop {let result=state.jobs.result(&task).await.unwrap(); if result["is_running"]==false {break result;}
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
        }).await.unwrap();
        assert_eq!(result["state"],"completed","{result}");
        server.abort();
    }

    #[tokio::test]
    async fn selected_season_nfos_must_agree_across_all_confirmed_directories() {
        let directory = tempfile::tempdir().unwrap();
        let paths = [
            directory.path().join("first-season.nfo"),
            directory.path().join("second-season.nfo"),
        ];
        for path in &paths {
            tokio::fs::write(path, "<season><title>第1季</title></season>")
                .await
                .unwrap();
        }
        assert_eq!(
            read_nfos(&paths).await.unwrap(),
            "<season><title>第1季</title></season>"
        );
        tokio::fs::write(&paths[1], "<season><title>冲突季</title></season>")
            .await
            .unwrap();
        assert!(read_nfos(&paths).await.unwrap_err().1.contains("不一致"));
    }
    fn original() -> Value {
        json!({"Id":"1","Type":"Series","Path":"/media/show","Name":"旧名","LockData":true,"LockedFields":["Runtime"],"DisplayOrder":"Dvd","Status":"Continuing","Tags":["旧标签"],"TagItems":[{"Name":"旧标签","Id":"10"}],"Genres":["旧类别"],"GenreItems":[{"Name":"旧类别","Id":"11"}],"ProviderIds":{"Tmdb":"123","Imdb":"tt456"},"MediaStreams":[{"Type":"Video","Codec":"hevc"}],"MediaSources":[{"Id":"source-1","Path":"/media/file.mkv"}],"Chapters":[{"MarkerType":"IntroStart","StartPositionTicks":0}],"People":[{"Id":"7","Name":"导演","Type":"Director","Role":"Director","SortOrder":1000}],"Studios":[{"Id":"8","Name":"工作室"}],"PremiereDate":"2020-01-01T00:00:00Z","CustomRating":"保留"})
    }
    #[test]
    fn declared_nfo_values_preserve_series_controls_and_missing_fields() {
        let current = original();
        let (payload,changed) = project(&current,"<tvshow><title>新名 &amp; 更多</title><plot><![CDATA[简介 <b>正文</b>]]></plot><lockdata>false</lockdata><displayorder>aired</displayorder><year>2026</year><premiered>2026-11-15</premiered></tvshow>").unwrap();
        assert_eq!(payload["Name"], "新名 & 更多");
        assert_eq!(payload["Overview"], "简介 <b>正文</b>");
        for field in MEDIA.iter().chain(
            [
                "LockData",
                "LockedFields",
                "DisplayOrder",
                "Status",
                "CustomRating",
                "People",
                "Studios",
                "ProviderIds",
                "Tags",
            ]
            .iter(),
        ) {
            assert_eq!(payload[*field], current[*field], "{field}");
        }
        assert!(changed.contains(&"Name".into()));
        assert!(!changed.contains(&"LockData".into()));
        assert_eq!(payload["PremiereDate"], "2026-11-15T00:00:00+00:00");
    }
    #[test]
    fn explicit_name_lists_override_old_identifier_lists_and_keep_missing_provider_ids() {
        let (payload,_) = project(&original(),"<tvshow><genre>科幻</genre><tag>动画</tag><studio>工作室</studio><uniqueid type='tmdb'>999</uniqueid></tvshow>").unwrap();
        assert_eq!(payload["GenreItems"], Value::Null);
        assert_eq!(payload["TagItems"], Value::Null);
        assert_eq!(payload["Genres"], json!(["科幻"]));
        assert_eq!(payload["ProviderIds"], json!({"Tmdb":"999","Imdb":"tt456"}));
        assert_eq!(payload["Studios"][0]["Id"], "8");
    }
    #[test]
    fn director_actor_and_director_tag_reuse_native_person_without_duplicates() {
        let (payload,_) = project(&original(),"<tvshow><director>导演</director><actor><name>导演</name><type>Director</type><role>Director</role><order>1000</order></actor></tvshow>").unwrap();
        assert_eq!(payload["People"], original()["People"]);
    }
    #[test]
    fn unknown_series_controls_and_invalid_nfo_refuse_without_projection() {
        for field in ["LockData", "DisplayOrder"] {
            let mut current = original();
            current[field] = Value::Null;
            assert!(project(&current, "<tvshow><title>新名</title></tvshow>").is_err());
        }
        let mut unknown_order = original();
        unknown_order["DisplayOrder"] = json!("unknown");
        assert!(project(&unknown_order, "<tvshow><title>新名</title></tvshow>").is_err());
        for xml in [
            "<movie><title>错类型</title></movie>",
            "<tvshow><title>未闭合</tvshow>",
            "<!DOCTYPE tvshow [<!ENTITY x SYSTEM 'file:///secret'>]><tvshow><title>&x;</title></tvshow>",
        ] {
            assert!(project(&original(), xml).is_err());
        }
    }
    #[test]
    fn restart_accepts_only_original_or_intended_fields_and_unchanged_media() {
        let original = original();
        let (desired, _) = project(
            &original,
            "<tvshow><title>新名</title><plot>新简介</plot></tvshow>",
        )
        .unwrap();
        assert!(compatible(&original, &desired, &original));
        assert!(compatible(&original, &desired, &desired));
        let mut partial = original.clone();
        partial["Name"] = desired["Name"].clone();
        assert!(compatible(&original, &desired, &partial));
        partial["Name"] = json!("用户并发修改");
        assert!(!compatible(&original, &desired, &partial));
        partial = desired.clone();
        partial["MediaStreams"][0]["Codec"] = json!("av1");
        assert!(!compatible(&original, &desired, &partial));
    }
}
