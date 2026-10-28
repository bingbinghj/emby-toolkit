use crate::{
    AppState,
    error::{ApiError, Result},
    media_edit::list,
    state::string,
    versions,
};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use std::collections::{BTreeSet, HashMap, HashSet};

pub async fn row(state: &AppState, id: &str, kind: &str) -> Result<Option<Value>> {
    Ok(sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id=$1 AND item_type=$2",
    )
    .bind(id)
    .bind(kind)
    .fetch_optional(&state.db)
    .await?)
}
pub async fn upsert(
    state: &AppState,
    records: &mut [Value],
    preserve_top_title: bool,
) -> Result<()> {
    write(state, records, preserve_top_title, false).await
}
pub async fn ingest(state: &AppState, records: &mut [Value], replace_children: bool) -> Result<()> {
    write(state, records, false, replace_children).await
}
async fn write(
    state: &AppState,
    records: &mut [Value],
    preserve_top_title: bool,
    replace_children: bool,
) -> Result<()> {
    tracing::info!(
        "元数据库写入开始：记录 {} 条，顶层标题处理 {}，子项绑定处理 {}",
        records.len(),
        if preserve_top_title {
            "保留现有标题"
        } else {
            "同步本次标题"
        },
        if replace_children {
            "迁移旧绑定"
        } else {
            "保留其他子项绑定"
        }
    );
    let record_count = records.len();
    let mut transaction = state.db.begin().await?;
    let columns = sqlx::query("SELECT column_name,data_type FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='media_metadata'").fetch_all(&mut *transaction).await?;
    let allowed = columns
        .iter()
        .map(|r| r.try_get::<String, _>("column_name"))
        .collect::<std::result::Result<HashSet<_>, sqlx::Error>>()?;
    let json_columns = columns
        .iter()
        .filter(|r| r.get::<String, _>("data_type") == "jsonb")
        .map(|r| r.get::<String, _>("column_name"))
        .collect::<HashSet<_>>();
    for record in records.iter_mut() {
        let kind = string(record, "item_type");
        let id = string(record, "tmdb_id");
        let media_name = string(record, "title");
        let numeric = !id.is_empty()
            && id.bytes().all(|b| b.is_ascii_digit())
            && id.bytes().any(|b| b != b'0');
        let valid = numeric || (matches!(kind.as_str(), "Season" | "Episode") && id.contains('-'));
        if !valid || !matches!(kind.as_str(), "Movie" | "Series" | "Season" | "Episode") {
            tracing::warn!("元数据库记录拒绝：《{media_name}》，类型 {kind}，TMDb {id} 无效");
            return Err(ApiError::bad("媒体记录缺少合法的类型和 TMDb ID"));
        }
        let fields = record
            .as_object_mut()
            .ok_or_else(|| ApiError::bad("媒体记录必须是对象"))?;
        if fields.keys().any(|k| !allowed.contains(k)) {
            return Err(ApiError::bad("媒体记录包含未知列"));
        }
        for (key, value) in fields.iter_mut() {
            if json_columns.contains(key)
                && let Some(encoded) = value.as_str()
            {
                *value = serde_json::from_str(encoded)
                    .map_err(|_| ApiError::bad("媒体记录包含无效 JSON"))?;
            }
        }
        for key in [
            "emby_item_ids_json",
            "asset_details_json",
            "file_sha1_json",
            "file_pickcode_json",
        ] {
            if fields
                .get(key)
                .is_some_and(|v| !v.is_null() && !v.is_array())
            {
                return Err(ApiError::bad("媒体版本字段必须是数组"));
            }
        }
        for key in [
            "in_library",
            "emby_item_ids_json",
            "file_sha1_json",
            "file_pickcode_json",
        ] {
            if !fields.contains_key(key) || fields[key].is_null() {
                fields.insert(
                    key.into(),
                    if key == "in_library" {
                        json!(false)
                    } else {
                        json!([])
                    },
                );
            }
        }
    }
    let mut work = BTreeSet::new();
    for record in records.iter() {
        let kind = string(record, "item_type");
        let id = string(record, "tmdb_id");
        let parent = string(record, "parent_series_tmdb_id");
        work.insert(
            if matches!(kind.as_str(), "Episode" | "Season") && !parent.is_empty() {
                ("Series".into(), parent)
            } else {
                (kind, id)
            },
        );
    }
    for (kind, id) in work {
        versions::lock(&mut transaction, &kind, &id, "").await?;
    }
    if replace_children {
        tracing::info!("元数据库子项绑定迁移开始：记录 {record_count} 条");
        replace_child_bindings(&mut transaction, records).await?;
        tracing::info!("元数据库子项绑定迁移完成：记录 {record_count} 条");
    }
    for record in records {
        let id = string(record, "tmdb_id");
        let kind = string(record, "item_type");
        let media_name = string(record, "title");
        let old: Option<Value> = sqlx::query_scalar(
            "SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id=$1 AND item_type=$2 FOR UPDATE",
        )
        .bind(&id)
        .bind(&kind)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(old) = old {
            if record["in_library"] == false && old["in_library"] == true {
                for key in [
                    "in_library",
                    "emby_item_ids_json",
                    "asset_details_json",
                    "file_sha1_json",
                    "file_pickcode_json",
                ] {
                    record[key] = old[key].clone();
                }
            }
            if record["in_library"] != false {
                versions::merge(&old, record);
            }
        }
        let fields = record
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, v)| {
                (!preserve_top_title || !v.is_null())
                    && !matches!(k.as_str(), "last_synced_at" | "last_updated_at")
            })
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>();
        let columns = fields
            .iter()
            .map(|f| format!("\"{f}\""))
            .collect::<Vec<_>>()
            .join(",");
        let updates=fields.iter().filter(|f|!matches!(f.as_str(),"tmdb_id"|"item_type") && !(preserve_top_title && matches!(kind.as_str(),"Movie"|"Series") && f.as_str()=="title"))
            .map(|f|if f=="total_episodes" {"total_episodes=CASE WHEN media_metadata.total_episodes_locked IS TRUE THEN media_metadata.total_episodes ELSE EXCLUDED.total_episodes END".into()} else {format!("\"{f}\"=EXCLUDED.\"{f}\"")}).collect::<Vec<_>>().join(",");
        let updates = if updates.is_empty() {
            String::new()
        } else {
            format!("{updates},")
        };
        let query = format!(
            "INSERT INTO media_metadata ({columns},last_synced_at) SELECT {columns},NOW() FROM jsonb_populate_record(NULL::media_metadata,$1::jsonb) ON CONFLICT(tmdb_id,item_type) DO UPDATE SET {updates}last_synced_at=NOW()"
        );
        sqlx::query(&query)
            .bind(&*record)
            .execute(&mut *transaction)
            .await
            .inspect_err(|_| {
                tracing::error!("元数据库记录写入失败：《{media_name}》，类型 {kind}，TMDb {id}")
            })?;
        tracing::debug!("元数据库记录已写入事务：《{media_name}》，类型 {kind}，TMDb {id}");
    }
    transaction
        .commit()
        .await
        .inspect_err(|_| tracing::error!("元数据库事务提交失败：记录 {record_count} 条"))?;
    tracing::info!("元数据库写入完成：已提交 {record_count} 条记录");
    Ok(())
}

// An episode-group change moves only the incoming Emby versions between metadata keys.
// Fingerprints follow the Emby ID; versions in other directories stay on their old keys.
async fn replace_child_bindings(
    transaction: &mut Transaction<'_, Postgres>,
    records: &mut [Value],
) -> Result<()> {
    let parents = records
        .iter()
        .filter(|r| matches!(r["item_type"].as_str(), Some("Episode" | "Season")))
        .map(|r| string(r, "parent_series_tmdb_id"))
        .filter(|p| !p.is_empty())
        .collect::<BTreeSet<_>>();
    for parent in parents {
        let rows:Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE parent_series_tmdb_id=$1 AND item_type IN ('Season','Episode') FOR UPDATE")
            .bind(&parent).fetch_all(&mut **transaction).await?;
        let mut destinations = HashMap::new();
        let mut slots = HashMap::new();
        for old in &rows {
            let ids = list(&old["emby_item_ids_json"]);
            for (index, id) in ids.iter().enumerate() {
                slots.insert(
                    (string(old, "item_type"), id.clone()),
                    json!({
                    "emby_item_ids_json":[id],
                    "asset_details_json":[list(&old["asset_details_json"]).get(index)],
                    "file_sha1_json":[list(&old["file_sha1_json"]).get(index)],
                    "file_pickcode_json":[list(&old["file_pickcode_json"]).get(index)]}),
                );
            }
        }
        for record in records
            .iter_mut()
            .filter(|r| string(r, "parent_series_tmdb_id") == parent)
        {
            let kind = string(record, "item_type");
            let tmdb = string(record, "tmdb_id");
            let ids = list(&record["emby_item_ids_json"]);
            for id in &ids {
                let key = (kind.clone(), id.clone());
                if destinations
                    .insert(key.clone(), tmdb.clone())
                    .is_some_and(|before| before != tmdb)
                {
                    return Err(ApiError::bad("同一 Emby 版本不能绑定到多个元数据记录"));
                }
                if let Some(old) = slots.get(&key) {
                    versions::merge(old, record);
                }
            }
        }
        for old in rows {
            let kind = string(&old, "item_type");
            let tmdb = string(&old, "tmdb_id");
            let ids = list(&old["emby_item_ids_json"]);
            let retained = ids
                .iter()
                .enumerate()
                .filter(|(_, id)| {
                    destinations
                        .get(&(kind.clone(), (*id).clone()))
                        .is_none_or(|to| *to == tmdb)
                })
                .collect::<Vec<_>>();
            if retained.len() == ids.len() {
                continue;
            }
            if retained.is_empty() {
                sqlx::query("DELETE FROM media_metadata WHERE tmdb_id=$1 AND item_type=$2")
                    .bind(&tmdb)
                    .bind(&kind)
                    .execute(&mut **transaction)
                    .await?;
            } else {
                let keep = |key: &str| {
                    let values = list(&old[key]);
                    json!(
                        retained
                            .iter()
                            .map(|(i, _)| values.get(*i).cloned().unwrap_or(Value::Null))
                            .collect::<Vec<_>>()
                    )
                };
                sqlx::query("UPDATE media_metadata SET emby_item_ids_json=$1,asset_details_json=$2,file_sha1_json=$3,file_pickcode_json=$4,last_updated_at=NOW() WHERE tmdb_id=$5 AND item_type=$6")
                    .bind(json!(retained.iter().map(|(_,id)|id).collect::<Vec<_>>())).bind(keep("asset_details_json")).bind(keep("file_sha1_json")).bind(keep("file_pickcode_json")).bind(&tmdb).bind(&kind).execute(&mut **transaction).await?;
            }
        }
    }
    Ok(())
}
pub fn reconstruct(row: &Value) -> Value {
    let kind = string(row, "item_type");
    let mut output = json!({"id":string(row,"tmdb_id").parse::<i64>().unwrap_or(0),
        "overview":row["overview"],"original_language":row["original_language"],
        "backdrop_path":row["backdrop_path"],"homepage":row["homepage"],"vote_average":row["rating"],
        "poster_path":row["poster_path"],"imdb_id":row["imdb_id"],"tagline":row["tagline"]});
    if let Some(date) = row["date_added"].as_str().filter(|s| !s.is_empty()) {
        output["date_added"] = json!(date);
    }
    let date = row["release_date"].as_str().unwrap_or("");
    if kind == "Movie" {
        output["title"] = row["title"].clone();
        output["original_title"] = row["original_title"].clone();
        output["release_date"] = json!(date);
        output["runtime"] = row["runtime_minutes"].clone();
    } else {
        output["name"] = row["title"].clone();
        output["original_name"] = row["original_title"].clone();
        output["first_air_date"] = json!(date);
        output["last_air_date"] = json!(row["last_air_date"].as_str().unwrap_or(""));
        output["number_of_episodes"] = row.get("total_episodes").cloned().unwrap_or(json!(0));
        output["number_of_seasons"] = json!(1);
    }
    let genres = list(&row["genres_json"]);
    if !genres.is_empty() {
        output["genres"] = if genres[0].is_string() {
            json!(
                genres
                    .iter()
                    .map(|g| json!({"id":0,"name":g}))
                    .collect::<Vec<_>>()
            )
        } else {
            json!(genres)
        };
    }
    if kind == "Movie" {
        let companies = list(&row["production_companies_json"]);
        if !companies.is_empty() {
            output["production_companies"] = json!(companies);
        }
    } else if kind == "Series" {
        let mut seen = HashSet::new();
        let networks = list(&row["networks_json"])
            .into_iter()
            .chain(list(&row["production_companies_json"]))
            .filter(|v| !v["id"].is_null() && v["id"] != 0 && seen.insert(v["id"].to_string()))
            .collect::<Vec<_>>();
        if !networks.is_empty() {
            output["networks"] = json!(networks);
        }
    }
    let directors = list(&row["directors_json"]);
    if !directors.is_empty() {
        if kind == "Series" {
            output["created_by"] = json!(directors);
        } else {
            output["casts"] = json!({"crew":directors.iter().map(|d|json!({"id":d["id"],
            "name":d["name"],"job":"Director","department":"Directing"})).collect::<Vec<_>>()});
        }
    }
    let countries = list(&row["countries_json"]);
    if !countries.is_empty() {
        if kind == "Series" {
            output["origin_country"] = json!(countries);
        } else {
            output["production_countries"] = json!(
                countries
                    .iter()
                    .map(|v| json!({"iso_3166_1":v,"name":""}))
                    .collect::<Vec<_>>()
            );
        }
    }
    let keywords = list(&row["keywords_json"]);
    if !keywords.is_empty() {
        output["keywords"] = json!(keywords);
    }
    let ratings = if let Some(encoded) = row["official_rating_json"].as_str() {
        serde_json::from_str::<Value>(encoded).unwrap_or(Value::Null)
    } else {
        row["official_rating_json"].clone()
    };
    if let Some(us) = ratings["US"].as_str().filter(|s| !s.is_empty()) {
        output["mpaa"] = json!(us);
        output["certification"] = json!(us);
    }
    output
}

pub async fn reconstruct_full(
    state: &AppState,
    id: &str,
    kind: &str,
    allow_empty_cast: bool,
) -> Result<Option<(Value, Vec<Value>)>> {
    tracing::debug!("元数据缓存恢复开始：类型 {kind}，TMDb {id}");
    let Some(top) = row(state, id, kind).await? else {
        tracing::info!("元数据缓存未命中：类型 {kind}，TMDb {id}，缺少主记录");
        return Ok(None);
    };
    let media_name = string(&top, "title");
    let children: Vec<Value> = if kind == "Series" {
        sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE parent_series_tmdb_id=$1 AND item_type IN ('Season','Episode') ORDER BY season_number,episode_number,tmdb_id").bind(id).fetch_all(&state.db).await?
    } else {
        vec![]
    };
    let ids = std::iter::once(&top)
        .chain(children.iter())
        .flat_map(|r| {
            list(&r["actors_json"])
                .into_iter()
                .filter_map(|p| p["tmdb_id"].as_i64().and_then(|n| i32::try_from(n).ok()))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let people:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(p)||jsonb_build_object('name',primary_name,'tmdb_id',tmdb_person_id,'id',tmdb_person_id) FROM person_metadata p WHERE tmdb_person_id=ANY($1)").bind(ids).fetch_all(&state.db).await?;
    let expand = |row: &Value| {
        let mut cast = list(&row["actors_json"])
            .iter()
            .filter_map(|link| {
                let mut person = people.iter().find(|p| p["id"] == link["tmdb_id"])?.clone();
                person["character"] = link["character"].clone();
                person["order"] = link.get("order").cloned().unwrap_or(json!(999));
                Some(person)
            })
            .collect::<Vec<_>>();
        cast.sort_by_key(|p| p["order"].as_i64().unwrap_or(999));
        cast
    };
    let cast = expand(&top);
    if cast.is_empty() && !allow_empty_cast {
        tracing::info!(
            "元数据缓存不可复用：《{media_name}》，类型 {kind}，TMDb {id}，缺少演员缓存"
        );
        return Ok(None);
    }
    let mut payload = reconstruct(&top);
    for key in ["season_number", "episode_number"] {
        if kind == "Episode" || (kind == "Season" && key == "season_number") {
            payload[key] = top[key].clone();
        }
    }
    if matches!(kind, "Season" | "Episode") {
        payload["air_date"] = top["release_date"].clone();
    }
    if kind == "Series" {
        if !string(&top, "episode_group_id").is_empty() {
            payload["episode_group_id"] = top["episode_group_id"].clone();
        }
        let mut seasons = vec![];
        let mut episodes = json!({});
        for child in children {
            let id = string(&child, "tmdb_id");
            let id = id.parse::<u64>().map(|n| json!(n)).unwrap_or(json!(id));
            if child["item_type"] == "Season" {
                seasons.push(json!({"id":id,"name":child["title"],"overview":child["overview"],"season_number":child["season_number"],"air_date":child["release_date"],"poster_path":child["poster_path"]}));
            } else {
                episodes[format!("S{}E{}", child["season_number"], child["episode_number"])] = json!({"id":id,"name":child["title"],"overview":child["overview"],"season_number":child["season_number"],"episode_number":child["episode_number"],"air_date":child["release_date"],"vote_average":child["rating"],"still_path":child["poster_path"],"credits":{"cast":expand(&child),"crew":list(&child["directors_json"]).iter().map(|d|json!({"id":d["id"],"name":d["name"],"job":"Director"})).collect::<Vec<_>>()}});
            }
        }
        if !seasons.is_empty() {
            payload["seasons_details"] = json!(seasons);
        }
        if episodes.as_object().is_some_and(|o| !o.is_empty()) {
            payload["episodes_details"] = episodes;
        }
    }
    tracing::info!(
        "元数据缓存恢复完成：《{media_name}》，类型 {kind}，TMDb {id}，演员 {} 位，季 {} 个，分集 {} 个",
        cast.len(),
        list(&payload["seasons_details"]).len(),
        payload["episodes_details"]
            .as_object()
            .map_or(0, |o| o.len())
    );
    Ok(Some((payload, cast)))
}
