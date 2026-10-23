use crate::{
    AppState,
    error::{ApiError, Result},
    media_edit::list,
    state::string,
};
use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/search_emby_library", get(search))
}
pub fn display(item: &Value, libraries: &[Value]) -> Value {
    let path = string(item, "Path").replace('\\', "/");
    let path = path.trim_end_matches('/');
    let library = libraries
        .iter()
        .flat_map(|l| {
            list(&l["Locations"]).into_iter().filter_map(move |root| {
                let root = root.as_str()?.replace('\\', "/");
                let root = root.trim_end_matches('/');
                (!root.is_empty() && (path == root || path.starts_with(&format!("{root}/"))))
                    .then_some((root.len(), l))
            })
        })
        .max_by_key(|(length, _)| *length)
        .map(|(_, l)| l)
        .unwrap_or(&Value::Null);
    let tag = item["ImageTags"]["Primary"]
        .as_str()
        .filter(|s| !s.is_empty());
    let fallback = item["SeriesPrimaryImageTag"]
        .as_str()
        .filter(|s| !s.is_empty());
    json!({"library_name":library["Name"].as_str().unwrap_or(""),"image_id":if tag.is_some(){item["Id"].clone()}else if fallback.is_some(){item["SeriesId"].clone()}else{Value::Null},"image_tag":tag.or(fallback),"production_year":item["ProductionYear"],"series_id":if item["Type"]=="Series"{item["Id"].clone()}else{item["SeriesId"].clone()},"series_name":if item["Type"]=="Series"{item["Name"].clone()}else{json!(item["SeriesName"].as_str().unwrap_or(""))},"item_type":item["Type"],"season_number":if item["Type"]=="Episode"{item["ParentIndexNumber"].clone()}else{item["IndexNumber"].clone()},"episode_number":if item["Type"]=="Episode"{item["IndexNumber"].clone()}else{Value::Null}})
}
pub fn sort(items: &mut [Value]) {
    items.sort_by_key(|i| match i["item_type"].as_str() {
        Some("Season") => (1, i["season_number"].as_i64().unwrap_or(1_000_000_000), 0),
        Some("Episode") => (
            2,
            i["season_number"].as_i64().unwrap_or(1_000_000_000),
            i["episode_number"].as_i64().unwrap_or(1_000_000_000),
        ),
        _ => (0, 0, 0),
    });
}
async fn search(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    let result = search_items(&state, &params).await;
    match &result {
        Ok(Json(data)) => tracing::info!(total_items = %data["total_items"], "手动搜索媒体库完成"),
        Err(error) => tracing::warn!(error = %error.1, "手动搜索媒体库失败"),
    }
    result
}
async fn search_items(state: &AppState, params: &HashMap<String, String>) -> Result<Json<Value>> {
    let query = params.get("query").map_or("", String::as_str);
    if query.trim().is_empty() {
        return Err(ApiError::bad("搜索词不能为空"));
    }
    let kind = params.get("media_type").map_or("", |s| s.trim());
    let types = match kind {
        "" => "Movie,Series,Season,Episode",
        "Movie" => "Movie",
        "Series" => "Series,Season,Episode",
        _ => return Err(ApiError::bad("媒体类型仅支持电视剧或电影")),
    };
    let user = string(&*state.config.read().await, "emby_user_id");
    let response=state.emby(reqwest::Method::GET,&format!("Users/{user}/Items"),&[("SearchTerm",query.into()),("IncludeItemTypes",types.into()),("Recursive","true".into()),("Fields","Id,Name,Type,ProductionYear,ProviderIds,Path,ParentIndexNumber,IndexNumber,SeriesName,ImageTags,SeriesId,SeriesPrimaryImageTag".into()),("Limit","50".into())],None).await?;
    let mut details = HashMap::new();
    let mut items = vec![];
    let mut seen = HashSet::new();
    for item in list(&response["Items"]) {
        let id = string(&item, "Id");
        if id.is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        items.push(json!({"item_id":id,"item_name":item["Name"].as_str().filter(|s|!s.is_empty()).map(str::to_string).unwrap_or(format!("ItemID {id}")),"item_type":item["Type"],"failed_at":null,"error_message":format!("Emby 媒体库搜索结果 (年份: {})",item["ProductionYear"].as_i64().map(|n|n.to_string()).unwrap_or("N/A".into())),"score":null,"provider_ids":item["ProviderIds"].as_object().cloned().unwrap_or_default()}));
        details.insert(id, item);
    }
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE (title ILIKE $1 OR original_title ILIKE $1) AND ($2='' OR item_type=ANY(string_to_array($2,','))) LIMIT 50") .bind(format!("%{query}%")).bind(if kind.is_empty(){""}else{types}).fetch_all(&state.db).await?;
    for row in rows {
        let mut ids = list(&row["emby_item_ids_json"]);
        if ids.is_empty() && row["item_type"] == "Series" {
            let child:Option<Value>=sqlx::query_scalar("SELECT emby_item_ids_json FROM media_metadata WHERE parent_series_tmdb_id=$1 AND item_type IN ('Season','Episode') AND in_library=true AND jsonb_array_length(emby_item_ids_json)>0 LIMIT 1").bind(string(&row,"tmdb_id")).fetch_optional(&state.db).await?;
            ids = child.map(|v| list(&v)).unwrap_or_default();
            ids.truncate(1);
        }
        let assets = list(&row["asset_details_json"]);
        for (index, id) in ids.iter().filter_map(Value::as_str).enumerate() {
            if !seen.insert(id.to_string()) {
                continue;
            }
            let asset = assets.get(index).unwrap_or(&Value::Null);
            let inferred = crate::media_display::fields(&string(asset, "path"));
            let version = ["resolution_display", "quality_display"]
                .iter()
                .map(|key| {
                    let s = string(asset, key);
                    if ["", "unknown", "未知"].contains(&s.to_lowercase().as_str()) {
                        string(&inferred, key)
                    } else {
                        s
                    }
                })
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            items.push(json!({"item_id":id,"item_name":format!("{}{}",string(&row,"title"),if version.is_empty(){String::new()}else{format!(" [{version}]")}),"item_type":row["item_type"],"production_year":row["release_year"],"failed_at":null,"error_message":format!("本地数据库搜索结果 (年份: {})",row["release_year"]),"score":null,"provider_ids":{"Tmdb":row["tmdb_id"]},"season_number":row["season_number"],"episode_number":row["episode_number"]}));
            if let Ok(item)=crate::media_edit::details(state,id,"Id,Name,Type,ProductionYear,Path,IndexNumber,ParentIndexNumber,SeriesId,SeriesName,SeriesPrimaryImageTag,ImageTags").await{details.insert(id.to_string(),item);}
        }
    }
    let libraries = list(
        &state
            .emby(reqwest::Method::GET, "Library/VirtualFolders", &[], None)
            .await?,
    );
    for item in &mut items {
        if let Some(data) = details.get(&string(item, "item_id")) {
            let mut display = display(data, &libraries);
            if display["production_year"].is_null() {
                display["production_year"] = item["production_year"].clone();
            }
            item.as_object_mut()
                .unwrap()
                .extend(display.as_object().unwrap().clone());
        }
    }
    if !kind.is_empty() {
        items.retain(|i| types.split(',').any(|t| i["item_type"] == t));
    }
    sort(&mut items);
    Ok(Json(json!({"total_items":items.len(),"items":items})))
}
