use crate::{
    AppState,
    error::{ApiError, Result},
    media_edit::list,
    state::string,
    versions,
};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

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
    write(state, records, preserve_top_title, false)
        .await
        .map(|_| ())
}
pub async fn ingest(state: &AppState, records: &mut [Value], replace_children: bool) -> Result<()> {
    write(state, records, false, replace_children)
        .await
        .map(|_| ())
}

pub(crate) struct WriteReport {
    pub written: usize,
    pub failures: Vec<String>,
}

pub(crate) async fn sync_upsert(state: &AppState, records: &mut [Value]) -> Result<WriteReport> {
    write(state, records, true, true).await
}
async fn write(
    state: &AppState,
    records: &mut [Value],
    preserve_top_title: bool,
    replace_children: bool,
) -> Result<WriteReport> {
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
    let mut written_records = 0;
    let mut failures = vec![];
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
        let type_label = match kind.as_str() {
            "Series" => "电视剧",
            "Movie" => "电影",
            "Season" => "季",
            "Episode" => "分集",
            _ => &kind,
        };
        let id = string(record, "tmdb_id");
        let media_name = string(record, "title");
        let numeric = !id.is_empty()
            && id.bytes().all(|b| b.is_ascii_digit())
            && id.bytes().any(|b| b != b'0');
        let valid = numeric || (matches!(kind.as_str(), "Season" | "Episode") && id.contains('-'));
        if !valid || !matches!(kind.as_str(), "Movie" | "Series" | "Season" | "Episode") {
            tracing::warn!("元数据库记录拒绝：《{media_name}》，类型 {type_label}，TMDb {id} 无效");
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
    if replace_children {
        let ids = records
            .iter()
            .flat_map(|record| list(&record["emby_item_ids_json"]))
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect::<BTreeSet<_>>();
        if !ids.is_empty() {
            let previous: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('tmdb_id',tmdb_id,'item_type',item_type,'parent_series_tmdb_id',parent_series_tmdb_id) FROM media_metadata WHERE emby_item_ids_json ?| $1")
                .bind(ids.into_iter().collect::<Vec<_>>()).fetch_all(&mut *transaction).await?;
            for row in previous {
                let kind = string(&row, "item_type");
                let id = string(&row, "tmdb_id");
                let parent = string(&row, "parent_series_tmdb_id");
                work.insert(
                    if matches!(kind.as_str(), "Episode" | "Season") && !parent.is_empty() {
                        ("Series".into(), parent)
                    } else {
                        (kind, id)
                    },
                );
            }
        }
    }
    for (kind, id) in work {
        versions::lock(&mut transaction, &kind, &id, "").await?;
    }
    let mut sync_records;
    let records = if replace_children && preserve_top_title {
        sync_records = preserve_sync_child_keys(&mut transaction, records).await?;
        sync_records.as_mut_slice()
    } else {
        records
    };
    let record_count = records.len();
    if replace_children && !preserve_top_title {
        tracing::info!("元数据库子项绑定迁移开始：记录 {record_count} 条");
        replace_child_bindings(&mut transaction, records).await?;
        tracing::info!("元数据库子项绑定迁移完成：记录 {record_count} 条");
    }
    for record in records {
        if preserve_top_title {
            sqlx::query("SAVEPOINT sync_record")
                .execute(&mut *transaction)
                .await?;
            if replace_children {
                // Moving the native binding must roll back with a failed destination write.
                replace_child_bindings(&mut transaction, std::slice::from_mut(record)).await?;
            }
        }
        let id = string(record, "tmdb_id");
        let kind = string(record, "item_type");
        let type_label = match kind.as_str() {
            "Series" => "电视剧",
            "Movie" => "电影",
            "Season" => "季",
            "Episode" => "分集",
            _ => &kind,
        };
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
        let result = sqlx::query(&query)
            .bind(&*record)
            .execute(&mut *transaction)
            .await
            .inspect_err(|_| {
                tracing::error!(
                    "元数据库记录写入失败：《{media_name}》，类型 {type_label}，TMDb {id}"
                )
            });
        let written = result.is_ok();
        if let Err(error) = result {
            if !preserve_top_title {
                return Err(error.into());
            }
            sqlx::query("ROLLBACK TO SAVEPOINT sync_record")
                .execute(&mut *transaction)
                .await?;
            let code = error
                .as_database_error()
                .and_then(|error| error.code())
                .map(|code| code.into_owned())
                .unwrap_or_else(|| "未知".into());
            let reason = format!(
                "类型 {kind}，TMDb {id}：元数据库写入失败（数据库错误代码 {code}），已保留原记录"
            );
            tracing::error!("{reason}，继续本批其他媒体项。");
            failures.push(reason);
        }
        if preserve_top_title {
            sqlx::query("RELEASE SAVEPOINT sync_record")
                .execute(&mut *transaction)
                .await?;
        }
        if written {
            written_records += 1;
            tracing::debug!(
                "元数据库记录已写入事务：《{media_name}》，类型 {type_label}，TMDb {id}"
            );
        }
    }
    transaction
        .commit()
        .await
        .inspect_err(|_| tracing::error!("元数据库事务提交失败：记录 {record_count} 条"))?;
    tracing::info!(
        "元数据库写入结束：本批 {record_count} 条，已提交 {written_records} 条，未写入 {} 条",
        record_count - written_records
    );
    Ok(WriteReport {
        written: written_records,
        failures,
    })
}

// Missing TMDb child details produce a synthetic key. Keep each existing native
// version on its known key until TMDb can confirm a replacement.
async fn preserve_sync_child_keys(
    transaction: &mut Transaction<'_, Postgres>,
    records: &[Value],
) -> Result<Vec<Value>> {
    let mut output = vec![];
    for record in records {
        let kind = string(record, "item_type");
        let fallback = string(record, "tmdb_id");
        if !matches!(kind.as_str(), "Season" | "Episode") || !fallback.contains('-') {
            output.push(record.clone());
            continue;
        }
        let ids = list(&record["emby_item_ids_json"]);
        let native_ids = ids.iter().filter_map(Value::as_str).collect::<Vec<_>>();
        let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE parent_series_tmdb_id=$1 AND item_type=$2 AND emby_item_ids_json ?| $3 ORDER BY (tmdb_id ~ '^[0-9]+$') DESC,tmdb_id FOR UPDATE")
            .bind(string(record, "parent_series_tmdb_id")).bind(&kind).bind(&native_ids)
            .fetch_all(&mut **transaction).await?;
        let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, id) in ids.iter().enumerate() {
            let candidates = rows
                .iter()
                .filter(|row| list(&row["emby_item_ids_json"]).contains(id))
                .collect::<Vec<_>>();
            let numeric = candidates
                .iter()
                .filter(|row| !string(row, "tmdb_id").contains('-'))
                .collect::<Vec<_>>();
            if numeric.len() > 1 {
                return Err(ApiError::bad(format!(
                    "原生 {kind} {id} 已有多个真实 TMDb 绑定，读取元数据失败时保留原绑定"
                )));
            }
            let key = numeric
                .first()
                .map(|row| string(row, "tmdb_id"))
                .or_else(|| {
                    candidates
                        .iter()
                        .find(|row| string(row, "tmdb_id") == fallback)
                        .map(|_| fallback.clone())
                })
                .or_else(|| candidates.first().map(|row| string(row, "tmdb_id")))
                .unwrap_or_else(|| fallback.clone());
            groups.entry(key).or_default().push(index);
        }
        if groups.is_empty() {
            output.push(record.clone());
        }
        for (key, indices) in groups {
            let mut resolved = record.clone();
            resolved["tmdb_id"] = json!(key);
            for field in [
                "emby_item_ids_json",
                "asset_details_json",
                "file_sha1_json",
                "file_pickcode_json",
            ] {
                let values = list(&record[field]);
                resolved[field] = json!(
                    indices
                        .iter()
                        .map(|i| values.get(*i).cloned().unwrap_or(Value::Null))
                        .collect::<Vec<_>>()
                );
            }
            if let Some(old) = rows.iter().find(|row| string(row, "tmdb_id") == key) {
                for field in ["season_number", "episode_number", "episode_group_id"] {
                    if !old[field].is_null() {
                        resolved[field] = old[field].clone();
                    }
                }
            }
            output.push(resolved);
        }
    }
    Ok(output)
}

// Move only incoming native versions; unrelated directories retain their bindings.
async fn replace_child_bindings(
    transaction: &mut Transaction<'_, Postgres>,
    records: &mut [Value],
) -> Result<()> {
    let native_ids = records
        .iter()
        .flat_map(|record| list(&record["emby_item_ids_json"]))
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect::<BTreeSet<_>>();
    if native_ids.is_empty() {
        return Ok(());
    }
    let kinds = records
        .iter()
        .map(|record| string(record, "item_type"))
        .collect::<BTreeSet<_>>();
    let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE item_type=ANY($1) AND emby_item_ids_json ?| $2 ORDER BY (tmdb_id ~ '^[0-9]+$') DESC,tmdb_id,item_type FOR UPDATE")
        .bind(kinds.into_iter().collect::<Vec<_>>()).bind(native_ids.into_iter().collect::<Vec<_>>())
        .fetch_all(&mut **transaction).await?;
    let mut destinations = HashMap::new();
    let mut slots = HashMap::<(String, Value), (bool, Value)>::new();
    for old in &rows {
        let source_id = string(old, "tmdb_id");
        let numeric = !source_id.is_empty() && source_id.bytes().all(|b| b.is_ascii_digit());
        for (index, id) in list(&old["emby_item_ids_json"]).iter().enumerate() {
            let slot = json!({"emby_item_ids_json":[id],
                "asset_details_json":[list(&old["asset_details_json"]).get(index)],
                "file_sha1_json":[list(&old["file_sha1_json"]).get(index)],
                "file_pickcode_json":[list(&old["file_pickcode_json"]).get(index)]});
            let key = (string(old, "item_type"), id.clone());
            if let Some((known_real, before)) = slots.get_mut(&key) {
                // Existing real keys remain authoritative over stale synthetic duplicates.
                if *known_real && !numeric {
                    continue;
                }
                for field in ["file_sha1_json", "file_pickcode_json"] {
                    let a = &before[field][0];
                    let b = &slot[field][0];
                    if !crate::fingerprints::missing(a)
                        && !crate::fingerprints::missing(b)
                        && if field == "file_sha1_json" {
                            match (a.as_str(), b.as_str()) {
                                (Some(a), Some(b)) => !a.eq_ignore_ascii_case(b),
                                _ => a != b,
                            }
                        } else {
                            a != b
                        }
                    {
                        return Err(ApiError::bad("同一原生版本的指纹缓存冲突，已保留原绑定"));
                    }
                }
                let a = &before["asset_details_json"][0];
                let b = &slot["asset_details_json"][0];
                for field in [
                    "path",
                    "fingerprint_target",
                    "fingerprint_pending",
                    "fingerprint_revision",
                ] {
                    if !a[field].is_null() && !b[field].is_null() && a[field] != b[field] {
                        return Err(ApiError::bad("同一原生版本的身份缓存冲突，已保留原绑定"));
                    }
                }
                if matches!((crate::identity::size(&a["size_bytes"]),crate::identity::size(&b["size_bytes"])),(Some(x),Some(y)) if x!=y)
                {
                    return Err(ApiError::bad(
                        "同一原生版本的文件大小缓存冲突，已保留原绑定",
                    ));
                }
                versions::merge(&slot, before);
            } else {
                slots.insert(key, (numeric, slot));
            }
        }
    }
    for record in records.iter_mut() {
        let kind = string(record, "item_type");
        let tmdb = string(record, "tmdb_id");
        let parent = string(record, "parent_series_tmdb_id");
        for id in list(&record["emby_item_ids_json"]) {
            let key = (kind.clone(), id.clone());
            if destinations
                .insert(key.clone(), (tmdb.clone(), parent.clone()))
                .is_some_and(|before| before != (tmdb.clone(), parent.clone()))
            {
                return Err(ApiError::bad("同一 Emby 版本不能绑定到多个元数据记录"));
            }
            if let Some((_, slot)) = slots.get(&key) {
                versions::merge(slot, record);
            }
        }
    }
    for old in rows {
        let kind = string(&old, "item_type");
        let tmdb = string(&old, "tmdb_id");
        let parent = string(&old, "parent_series_tmdb_id");
        let ids = list(&old["emby_item_ids_json"]);
        let retained = ids
            .iter()
            .enumerate()
            .filter(|(_, id)| {
                destinations
                    .get(&(kind.clone(), (*id).clone()))
                    .is_none_or(|to| to.0 == tmdb)
            })
            .collect::<Vec<_>>();
        if retained.len() == ids.len() {
            continue;
        }
        let moved_parent = ids.iter().any(|id| {
            destinations
                .get(&(kind.clone(), id.clone()))
                .is_some_and(|to| to.1 != parent)
        });
        if retained.is_empty() && !matches!(kind.as_str(), "Movie" | "Series") && !moved_parent {
            sqlx::query("DELETE FROM media_metadata WHERE tmdb_id=$1 AND item_type=$2")
                .bind(&tmdb)
                .bind(&kind)
                .execute(&mut **transaction)
                .await?;
            continue;
        }
        let keep = |key: &str| {
            let values = list(&old[key]);
            json!(
                retained
                    .iter()
                    .map(|(i, _)| values.get(*i).cloned().unwrap_or(Value::Null))
                    .collect::<Vec<_>>()
            )
        };
        sqlx::query("UPDATE media_metadata SET emby_item_ids_json=$1,asset_details_json=$2,file_sha1_json=$3,file_pickcode_json=$4,in_library=$5,last_updated_at=NOW() WHERE tmdb_id=$6 AND item_type=$7")
            .bind(json!(retained.iter().map(|(_,id)|id).collect::<Vec<_>>())).bind(if retained.is_empty(){Value::Null}else{keep("asset_details_json")})
            .bind(keep("file_sha1_json")).bind(keep("file_pickcode_json")).bind(!retained.is_empty()).bind(&tmdb).bind(&kind).execute(&mut **transaction).await?;
    }
    Ok(())
}

pub fn reconstruct(row: &Value) -> Value {
    let kind = string(row, "item_type");
    let mut output = json!({"id":string(row,"tmdb_id").parse::<i64>().unwrap_or(0),
        "overview":row["overview"],"original_language":row["original_language"],
        "backdrop_path":row["backdrop_path"],"homepage":row["homepage"],
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
    let type_label = match kind {
        "Series" => "电视剧",
        "Movie" => "电影",
        "Season" => "季",
        "Episode" => "分集",
        _ => kind,
    };
    tracing::debug!("元数据缓存恢复开始：类型 {type_label}，TMDb {id}");
    let Some(top) = row(state, id, kind).await? else {
        tracing::info!("元数据缓存未命中：类型 {type_label}，TMDb {id}，缺少主记录");
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
    let mut people_by_id = HashMap::new();
    for person in &people {
        if let Some(id) = person["id"].as_i64() {
            people_by_id.entry(id).or_insert(person);
        }
    }
    let expand = |row: &Value| {
        let mut cast = list(&row["actors_json"])
            .iter()
            .filter_map(|link| {
                let mut person = (**people_by_id.get(&link["tmdb_id"].as_i64()?)?).clone();
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
            "元数据缓存不可复用：《{media_name}》，类型 {type_label}，TMDb {id}，缺少演员缓存"
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
                episodes[format!("S{}E{}", child["season_number"], child["episode_number"])] = json!({"id":id,"name":child["title"],"overview":child["overview"],"season_number":child["season_number"],"episode_number":child["episode_number"],"air_date":child["release_date"],"still_path":child["poster_path"],"credits":{"cast":expand(&child),"crew":list(&child["directors_json"]).iter().map(|d|json!({"id":d["id"],"name":d["name"],"job":"Director"})).collect::<Vec<_>>()}});
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
        "元数据缓存恢复完成：《{media_name}》，类型 {type_label}，TMDb {id}，复用演员 {} 位，季 {} 个，分集 {} 个",
        cast.len(),
        list(&payload["seasons_details"]).len(),
        payload["episodes_details"]
            .as_object()
            .map_or(0, |o| o.len())
    );
    Ok(Some((payload, cast)))
}

#[cfg(test)]
#[path = "../tests/support/sync_child_bindings.rs"]
mod sync_child_bindings;
