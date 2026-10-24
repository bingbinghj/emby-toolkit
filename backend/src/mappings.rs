use crate::{
    AppState, db,
    error::{ApiError, Result},
    media_edit::list,
    routes::default_mapping,
    state::string,
};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::get,
};
use reqwest::Method;
use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/mappings/config/tags", get(tags))
        .route("/api/mappings/config/movie_genres", get(genres))
        .route("/api/mappings/config/tv_genres", get(genres))
        .route("/api/mappings/config/emby_libraries", get(libraries))
        .route("/api/mappings/config/keywords", get(keywords))
        .route("/api/mappings/config/studios", get(studios))
        .route("/api/mappings/config/languages", get(languages))
        .route("/api/mappings/config/tmdb_countries", get(countries))
        .route("/api/mappings/config/unified_ratings_options", get(ratings))
        .route("/api/mappings/config/tmdb_movie_genres", get(movie_genres))
        .route("/api/mappings/config/tmdb_tv_genres", get(tv_genres))
        .route(
            "/api/mappings/config/tmdb_search_persons",
            get(search_persons),
        )
        .route(
            "/api/mappings/config/tmdb_search_companies",
            get(search_companies),
        )
}
async fn external_genres(state: &AppState, kind: &str) -> Result<Json<Value>> {
    if string(&*state.config.read().await, "tmdb_api_key").is_empty() {
        return Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "TMDb API Key 未配置".into(),
        ));
    }
    let error = || {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "从 TMDb 获取{}类型失败",
                if kind == "movie" {
                    "电影"
                } else {
                    "电视剧"
                }
            ),
        )
    };
    let data = crate::tmdb::get(state, &format!("/genre/{kind}/list"), &[])
        .await
        .map_err(|_| error())?;
    let mut genres = data["genres"].as_array().cloned().ok_or_else(error)?;
    for genre in &mut genres {
        let name = string(genre, "name");
        genre["name"] = json!(match name.as_str() {
            "Sci-Fi & Fantasy" => "科幻奇幻",
            "War & Politics" => "战争政治",
            _ => &name,
        });
    }
    Ok(Json(json!(genres)))
}
async fn movie_genres(State(state): State<AppState>) -> Result<Json<Value>> {
    external_genres(&state, "movie").await
}
async fn tv_genres(State(state): State<AppState>) -> Result<Json<Value>> {
    external_genres(&state, "tv").await
}
async fn search_persons(
    State(state): State<AppState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>> {
    let query = query.get("q").map_or("", String::as_str);
    if query.is_empty() {
        return Ok(Json(json!([])));
    }
    let data = crate::tmdb::get(
        &state,
        "/search/person",
        &[("query", query.into()), ("include_adult", "false".into())],
    )
    .await
    .unwrap_or(Value::Null);
    Ok(Json(json!(list(&data["results"]).iter().map(|p|json!({"id":p["id"],"name":p["name"],"profile_path":p["profile_path"],"department":p["known_for_department"],"known_for":list(&p["known_for"]).iter().filter_map(|i|i["title"].as_str().filter(|s|!s.is_empty()).or_else(||i["name"].as_str())).collect::<Vec<_>>().join("、")})).collect::<Vec<_>>())))
}
async fn search_companies(
    State(state): State<AppState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>> {
    let q = query.get("q").map_or("", String::as_str);
    if q.is_empty() {
        return Ok(Json(json!([])));
    }
    if query.get("type").is_some_and(|s| s == "network") {
        let mut parameters = vec![
            ("query", q.into()),
            ("include_adult", "false".into()),
            ("page", "1".into()),
        ];
        let mut data = crate::tmdb::get(&state, "/search/tv", &parameters)
            .await
            .unwrap_or(Value::Null);
        if data.as_object().is_some_and(|v| !v.is_empty()) && list(&data["results"]).is_empty() {
            parameters.push(("language", "en-US".into()));
            data = crate::tmdb::get(&state, "/search/tv", &parameters)
                .await
                .unwrap_or(Value::Null);
        }
        let mut networks = vec![];
        let mut seen = std::collections::HashSet::new();
        for tv in list(&data["results"]).iter().take(3) {
            let Ok(details) = crate::tmdb::details(
                &state,
                "Series",
                &tv["id"].as_i64().unwrap_or(0).to_string(),
            )
            .await
            else {
                continue;
            };
            for net in list(&details["networks"]) {
                if seen.insert(net["id"].clone().to_string()) {
                    networks.push(json!({"id":net["id"],"name":net["name"],"logo_path":net["logo_path"],"origin_country":net["origin_country"],"source_show":tv["name"]}));
                }
            }
        }
        return Ok(Json(json!(networks)));
    }
    let data = crate::tmdb::get(&state, "/search/company", &[("query", q.into())])
        .await
        .unwrap_or(Value::Null);
    Ok(Json(json!(list(&data["results"]))))
}
pub fn ensure_list(value: Value, default: Value) -> Value {
    match value {
        Value::Array(list) if !list.is_empty() => json!(list),
        Value::Object(values) if !values.is_empty() => {
            let mut list = values
                .into_iter()
                .map(|(label, info)| {
                    let mut item = json!({"label":label});
                    if let Some(fields) = info.as_object() {
                        item.as_object_mut().unwrap().extend(fields.clone());
                    }
                    item
                })
                .collect::<Vec<_>>();
            list.sort_by_key(|v| string(v, "label"));
            json!(list)
        }
        _ => default,
    }
}
async fn mapping(state: &AppState, key: &str) -> Result<Vec<Value>> {
    let default = default_mapping(key).ok_or_else(|| ApiError::bad("未知映射"))?;
    Ok(ensure_list(
        db::setting(&state.db, key)
            .await?
            .unwrap_or(default.clone()),
        default,
    )
    .as_array()
    .cloned()
    .unwrap_or_default())
}
async fn distinct(state: &AppState, column: &str) -> Result<Json<Value>> {
    let sql = format!(
        "SELECT DISTINCT NULLIF(COALESCE(elem->>'name',elem->>'label',elem->>'value',elem #>> '{{}}'),'') AS label FROM media_metadata CROSS JOIN LATERAL jsonb_array_elements(COALESCE({column},'[]'::jsonb)) elem WHERE {column} IS NOT NULL ORDER BY label"
    );
    let labels: Vec<Option<String>> = match sqlx::query_scalar(&sql).fetch_all(&state.db).await {
        Ok(labels) => labels,
        Err(error) => {
            tracing::error!(%error, "读取媒体元数据 {column} 去重列表失败");
            return Ok(Json(json!([])));
        }
    };
    Ok(Json(json!(
        labels.into_iter().flatten().collect::<Vec<_>>()
    )))
}
async fn tags(State(state): State<AppState>) -> Result<Json<Value>> {
    distinct(&state, "tags_json").await
}
async fn genres(State(state): State<AppState>) -> Result<Json<Value>> {
    distinct(&state, "genres_json").await
}
async fn libraries(State(state): State<AppState>) -> Result<Json<Value>> {
    let config = state.config.read().await.clone();
    let user = string(&config, "emby_user_id");
    if ["emby_server_url", "emby_api_key", "emby_user_id"]
        .iter()
        .any(|key| string(&config, key).is_empty())
    {
        return Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Emby 服务器配置不完整".into(),
        ));
    }
    let views = state
        .emby(Method::GET, &format!("emby/Users/{user}/Views"), &[], None)
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "无法从 Emby 获取媒体库列表".into(),
            )
        })?;
    Ok(Json(json!(
        views["Items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|v| matches!(v["CollectionType"].as_str(), Some("movies" | "tvshows")))
            .map(|v| json!({"label":v["Name"],"value":v["Id"]}))
            .collect::<Vec<_>>()
    )))
}
async fn keywords(State(state): State<AppState>) -> Result<Json<Value>> {
    Ok(Json(json!(
        mapping(&state, "keyword_mapping")
            .await?
            .iter()
            .filter(|v| !string(v, "label").is_empty())
            .map(|v| json!({"label":v["label"],"value":v["label"]}))
            .collect::<Vec<_>>()
    )))
}
async fn languages(State(state): State<AppState>) -> Result<Json<Value>> {
    Ok(Json(json!(
        mapping(&state, "language_mapping")
            .await?
            .iter()
            .filter(|v| !string(v, "label").is_empty() && !string(v, "value").is_empty())
            .map(|v| json!({"label":v["label"],"value":v["value"]}))
            .collect::<Vec<_>>()
    )))
}
async fn countries(State(state): State<AppState>) -> Result<Json<Value>> {
    Ok(Json(json!(
        mapping(&state, "country_mapping")
            .await?
            .iter()
            .filter(|v| !string(v, "label").is_empty() && !string(v, "value").is_empty())
            .map(|v| json!({"label":v["label"],"value":v["value"]}))
            .collect::<Vec<_>>()
    )))
}
async fn ratings(State(state): State<AppState>) -> Result<Json<Value>> {
    Ok(Json(json!(
        mapping(&state, "unified_ratings")
            .await?
            .iter()
            .filter(|v| !string(v, "label").is_empty())
            .map(|v| v["label"].clone())
            .collect::<Vec<_>>()
    )))
}
fn ids(value: &Value) -> String {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .unwrap_or_else(|| v.to_string())
        })
        .collect::<Vec<_>>()
        .join(",")
}
async fn studios(State(state): State<AppState>) -> Result<Json<Value>> {
    let mut result = vec![];
    for item in mapping(&state, "studio_mapping").await? {
        if string(&item, "label").is_empty() {
            continue;
        }
        let mut types = vec![];
        let mut params = json!({});
        let company = item
            .get("company_ids")
            .filter(|v| v.as_array().is_some_and(|v| !v.is_empty()))
            .unwrap_or(&item["ids"]);
        if !ids(company).is_empty() {
            types.push("movie");
            params["with_companies"] = json!(ids(company));
        }
        if !ids(&item["network_ids"]).is_empty() {
            types.push("tv");
            params["with_networks"] = json!(ids(&item["network_ids"]));
        }
        result.push(
            json!({"label":item["label"],"value":item["label"],"types":types,"tmdb_params":params}),
        );
    }
    Ok(Json(json!(result)))
}
