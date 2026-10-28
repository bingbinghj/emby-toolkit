use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/database/tables", get(tables_route))
        .route("/api/database/export", post(export))
        .route("/api/database/preview-backup", post(preview))
        .route("/api/database/import", post(import))
        .route("/api/review_items", get(review))
        .route("/api/review_reasons", get(reasons))
        .route("/api/actions/mark_item_processed/{id}", post(mark))
        .route("/api/actions/clear_review_items", post(clear_review))
        .route("/api/actions/clear_tables", post(clear_tables))
        .route("/api/actions/cleanup-offline-media", post(cleanup_offline))
        .route("/api/actions/prepare-for-library-rebuild", post(rebuild))
        .route("/api/database/correct-sequences", post(correct))
        .layer(DefaultBodyLimit::disable())
}
fn identifier(name: &str) -> Result<String> {
    if name.is_empty() || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
        return Err(ApiError::bad("无效的数据库标识符"));
    }
    Ok(format!("\"{name}\""))
}
pub async fn tables(pool: &PgPool) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT table_name FROM information_schema.tables WHERE table_schema=current_schema() AND table_type='BASE TABLE' ORDER BY table_name").fetch_all(pool).await?)
}
async fn selected(pool: &PgPool, value: &Value) -> Result<Vec<String>> {
    let values = value
        .as_array()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad("必须指定非空 tables 数组"))?;
    let allowed: HashSet<_> = tables(pool).await?.into_iter().collect();
    let mut result = vec![];
    for value in values {
        let name = value
            .as_str()
            .ok_or_else(|| ApiError::bad("表名必须为字符串"))?;
        identifier(name)?;
        if !allowed.contains(name) {
            return Err(ApiError::bad(format!("数据表不存在: {name}")));
        }
        if !result.iter().any(|v| v == name) {
            result.push(name.into());
        }
    }
    Ok(result)
}
async fn tables_route(State(state): State<AppState>) -> Result<Json<Value>> {
    Ok(Json(json!(tables(&state.db).await?)))
}
pub async fn export_data(pool: &PgPool, names: &[String]) -> Result<Value> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *transaction)
        .await?;
    let mut data = json!({});
    for name in names {
        let query = format!("SELECT to_jsonb(t) FROM {} t", identifier(name)?);
        let rows: Vec<Value> = sqlx::query_scalar(&query)
            .fetch_all(&mut *transaction)
            .await?;
        data[name] = json!(rows);
    }
    transaction.commit().await?;
    Ok(data)
}
async fn export(State(state): State<AppState>, Json(body): Json<Value>) -> Result<Response> {
    let names = selected(&state.db, &body["tables"]).await?;
    let data = export_data(&state.db, &names).await?;
    let server = state.config.read().await["emby_server_id"].clone();
    let backup = json!({"metadata":{"export_date":chrono::Utc::now().to_rfc3339(),"app_version":"10.7.5","source_emby_server_id":server,"tables":names},"data":data});
    let compressed = tokio::task::spawn_blocking(move || {
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(backup.to_string().as_bytes())?;
        gzip.finish()
    })
    .await
    .map_err(|_| ApiError::bad("备份压缩失败"))??;
    Ok((
        [
            (header::CONTENT_TYPE, "application/gzip".into()),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename=database_backup_{}.json.gz",
                    chrono::Local::now().format("%Y%m%d-%H%M%S")
                ),
            ),
        ],
        compressed,
    )
        .into_response())
}
async fn upload(mut multipart: Multipart) -> Result<(String, Vec<u8>, Option<String>)> {
    let mut filename = None;
    let mut bytes = None;
    let mut names = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::bad("上传文件格式无效"))?
    {
        match field.name() {
            Some("file") => {
                filename = field.file_name().map(str::to_string);
                bytes = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|_| ApiError::bad("读取上传文件失败"))?
                        .to_vec(),
                );
            }
            Some("tables") => {
                names = Some(
                    field
                        .text()
                        .await
                        .map_err(|_| ApiError::bad("读取所选表失败"))?,
                );
            }
            _ => {}
        }
    }
    let filename = filename
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad("未选择文件"))?;
    Ok((
        filename,
        bytes.ok_or_else(|| ApiError::bad("请求中未找到文件部分"))?,
        names,
    ))
}
async fn decode(filename: String, bytes: Vec<u8>, allow_plain_gz: bool) -> Result<Value> {
    tokio::task::spawn_blocking(move || {
        let mut content = bytes;
        if filename.ends_with(".gz") {
            let mut decoded = vec![];
            match flate2::read::GzDecoder::new(content.as_slice()).read_to_end(&mut decoded) {
                Ok(_) => content = decoded,
                Err(_) if allow_plain_gz && !content.starts_with(&[0x1f, 0x8b]) => {}
                Err(_) => return Err(ApiError::bad("文件不是有效的 Gzip 格式")),
            }
        }
        let content = content
            .strip_prefix(&[0xef, 0xbb, 0xbf])
            .unwrap_or(&content);
        serde_json::from_slice(content).map_err(|_| ApiError::bad("无法解析备份文件 JSON"))
    })
    .await
    .map_err(|_| ApiError::bad("无法解析备份文件"))?
}
async fn preview(multipart: Multipart) -> Result<Json<Value>> {
    let (name, bytes, _) = upload(multipart).await?;
    let backup = decode(name, bytes, false).await?;
    let data = backup["data"]
        .as_object()
        .ok_or_else(|| ApiError::bad("备份缺少 data 数据"))?;
    Ok(Json(
        json!({"status":"success","tables":data.keys().collect::<Vec<_>>()}),
    ))
}
async fn import(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<(StatusCode, Json<Value>)> {
    let (name, bytes, names) = upload(multipart).await?;
    if !name.ends_with(".json") && !name.ends_with(".json.gz") {
        return Err(ApiError::bad("文件类型必须是 .json 或 .json.gz"));
    }
    let names = names
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ApiError::bad("必须通过 tables 字段指定要导入的表"))?;
    let names = selected(
        &state.db,
        &json!(names.split(',').map(str::trim).collect::<Vec<_>>()),
    )
    .await?;
    let backup = decode(name, bytes, true).await?;
    validate_backup(&backup, &names)?;
    let manager = state.jobs.clone();
    let task = manager
        .submit("数据库恢复", move |context| async move {
            let _gate = state.ai.cache_gate.write().await;
            if names.iter().any(|n| n == "translation_cache") {
                state
                    .ai
                    .cache_revision
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            import_data(&state.db, &backup, &names, Some(&context)).await?;
            drop(_gate);
            Ok(json!({"message":format!("已恢复 {} 个数据表",names.len())}))
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"message":"文件上传成功，已提交后台任务恢复数据表","task_id":task})),
    ))
}
fn validate_backup(backup: &Value, names: &[String]) -> Result<()> {
    for name in names {
        let rows = backup["data"][name]
            .as_array()
            .ok_or_else(|| ApiError::bad(format!("备份中缺少有效的数据表: {name}")))?;
        if rows
            .iter()
            .any(|r| r.as_object().is_none_or(|v| v.is_empty()))
        {
            return Err(ApiError::bad(format!("备份表 {name} 包含无效记录")));
        }
    }
    Ok(())
}
pub async fn import_data(
    pool: &PgPool,
    backup: &Value,
    names: &[String],
    context: Option<&JobContext>,
) -> Result<()> {
    if names.is_empty() {
        return Err(ApiError::bad("未选择数据表"));
    }
    validate_backup(backup, names)?;
    let mut names = names.to_vec();
    names.sort_by_key(|n| match n.as_str() {
        "person_metadata" => 0,
        "emby_users" => 1,
        "emby_users_extended" => 2,
        _ => 100,
    });
    let mut transaction = pool.begin().await?;
    let allowed:HashSet<String>=sqlx::query_scalar("SELECT table_name FROM information_schema.tables WHERE table_schema=current_schema() AND table_type='BASE TABLE'").fetch_all(&mut *transaction).await?.into_iter().collect();
    if names.iter().any(|name| !allowed.contains(name)) {
        return Err(ApiError::bad("所选表不存在"));
    }
    let quoted = names
        .iter()
        .map(|n| identifier(n))
        .collect::<Result<Vec<_>>>()?;
    sqlx::query(&format!(
        "TRUNCATE TABLE {} RESTART IDENTITY",
        quoted.join(",")
    ))
    .execute(&mut *transaction)
    .await?;
    for (index, name) in names.iter().enumerate() {
        if let Some(context) = context {
            context.checkpoint().await?;
            context
                .update(index as u64 * 90 / names.len() as u64, "恢复数据", name)
                .await;
        }
        let types=sqlx::query("SELECT column_name,data_type FROM information_schema.columns WHERE table_schema=current_schema() AND table_name=$1").bind(name).fetch_all(&mut *transaction).await?;
        let types = types
            .iter()
            .map(|r| {
                Ok((
                    r.try_get::<String, _>("column_name")?,
                    r.try_get::<String, _>("data_type")?,
                ))
            })
            .collect::<std::result::Result<HashMap<_, _>, sqlx::Error>>()?;
        let rows = backup["data"][name].as_array().unwrap();
        let Some(first) = rows.first() else {
            continue;
        };
        let columns = first
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        if columns.iter().any(|c| !types.contains_key(c)) {
            return Err(ApiError::bad(format!("备份表 {name} 包含不存在的列")));
        }
        let column_sql = columns
            .iter()
            .map(|n| identifier(n))
            .collect::<Result<Vec<_>>>()?
            .join(",");
        let query = format!(
            "INSERT INTO {table} ({column_sql}) SELECT {column_sql} FROM jsonb_populate_recordset(NULL::{table},$1::jsonb)",
            table = identifier(name)?
        );
        for chunk in rows.chunks(500) {
            if let Some(context) = context {
                context.checkpoint().await?;
            }
            let mut values = vec![];
            for row in chunk {
                let mut value = json!({});
                for column in &columns {
                    let raw = row[column].clone();
                    value[column] = if !matches!(types[column].as_str(), "json" | "jsonb")
                        && (raw.is_object() || raw.is_array())
                    {
                        json!(raw.to_string())
                    } else {
                        raw
                    };
                }
                values.push(value);
            }
            sqlx::query(&query)
                .bind(json!(values))
                .execute(&mut *transaction)
                .await?;
        }
    }
    transaction.commit().await?;
    if let Err(error) = correct_sequences(pool).await {
        tracing::error!("数据库导入已提交，后续 ID 计数器校准失败：{}", error.1);
    }
    Ok(())
}
pub async fn correct_sequences(pool: &PgPool) -> Result<Vec<String>> {
    let rows=sqlx::query("SELECT table_name,column_name FROM information_schema.columns WHERE table_schema=current_schema() AND (column_default LIKE 'nextval%' OR is_identity='YES')").fetch_all(pool).await?;
    let mut result = vec![];
    for row in rows {
        let table: String = row.try_get("table_name")?;
        let column: String = row.try_get("column_name")?;
        sqlx::query(&format!("SELECT setval(pg_get_serial_sequence($1,$2),COALESCE((SELECT MAX({}) FROM {}),0)+1,false)",identifier(&column)?,identifier(&table)?)).bind(&table).bind(&column).execute(pool).await?;
        result.push(table);
    }
    Ok(result)
}
async fn correct(State(state): State<AppState>) -> Result<Json<Value>> {
    Ok(Json(
        json!({"message":"ID 计数器校准完成","corrected_tables":correct_sequences(&state.db).await?}),
    ))
}
async fn review(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    let number = |key: &str, default: i64| {
        params
            .get(key)
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(default)
            .max(1)
    };
    let page = number("page", 1);
    let size = number("per_page", 20);
    let query = params.get("query").map_or("", String::as_str).trim();
    let reason = params.get("reason").map_or("", String::as_str).trim();
    let kind = params.get("media_type").map_or("", String::as_str).trim();
    let types: Vec<&str> = match kind {
        "" => vec![],
        "Movie" => vec!["Movie"],
        "Series" => vec!["Series", "Season", "Episode"],
        _ => return Err(ApiError::bad("媒体类型仅支持电视剧或电影")),
    };
    let where_sql = "WHERE ($1='' OR item_name ILIKE '%'||$1||'%') AND ($2='' OR reason LIKE $2||'%') AND (cardinality($3::text[])=0 OR item_type=ANY($3))";
    let total: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM failed_log {where_sql}"))
        .bind(query)
        .bind(reason)
        .bind(&types)
        .fetch_one(&state.db)
        .await?;
    let items:Vec<Value>=sqlx::query_scalar(&format!("SELECT to_jsonb(r) FROM (SELECT item_id,item_name,failed_at,reason,item_type,score FROM failed_log {where_sql} ORDER BY failed_at DESC LIMIT $4 OFFSET $5) r")).bind(query).bind(reason).bind(&types).bind(size).bind((page-1)*size).fetch_all(&state.db).await?;
    Ok(Json(
        json!({"items":items,"total_items":total,"total_pages":(total+size-1)/size,"current_page":page,"per_page":size,"query":query,"reason":reason,"media_type":kind}),
    ))
}
async fn reasons(State(state): State<AppState>) -> Result<Json<Value>> {
    let reasons: Vec<Option<String>> =
        sqlx::query_scalar("SELECT DISTINCT reason FROM failed_log WHERE reason IS NOT NULL")
            .fetch_all(&state.db)
            .await?;
    let reasons: std::collections::BTreeSet<_> = reasons
        .into_iter()
        .flatten()
        .map(|s| {
            let s = s.trim();
            if s.starts_with("缺失媒体信息") {
                "缺失媒体信息".to_string()
            } else if s.starts_with("处理评分") {
                "处理评分".to_string()
            } else {
                s.to_string()
            }
        })
        .filter(|s| !s.is_empty())
        .collect();
    Ok(Json(json!({"status":"success","data":reasons})))
}
async fn mark(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>> {
    if state.jobs.status().await["is_running"] == true {
        return Err(ApiError(StatusCode::CONFLICT, "后台有任务正在运行".into()));
    }
    let deleted = sqlx::query("DELETE FROM failed_log WHERE item_id=$1")
        .bind(&id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            format!("未在待复核列表中找到项目 {id}"),
        ));
    }
    Ok(Json(
        json!({"message":format!("项目 {id} 已成功从待复核列表移除")}),
    ))
}
async fn clear_review(State(state): State<AppState>) -> Result<Json<Value>> {
    let count = sqlx::query("DELETE FROM failed_log")
        .execute(&state.db)
        .await?
        .rows_affected();
    Ok(Json(
        json!({"message":format!("已从待复核列表移除 {count} 个项目")}),
    ))
}
async fn clear_tables(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let names = body["tables"]
        .as_array()
        .filter(|names| !names.is_empty())
        .ok_or_else(|| ApiError::bad("请求体必须包含非空 tables 数组"))?;
    let valid = regex::Regex::new(r"^[_\p{XID_Start}]\p{XID_Continue}*$").unwrap();
    let mut count = 0;
    for name in names {
        let Some(name) = name.as_str().filter(|name| valid.is_match(name)) else {
            tracing::warn!("非法表名跳过清空：{name}");
            continue;
        };
        let _gate = if name == "translation_cache" {
            Some(state.ai.cache_gate.write().await)
        } else {
            None
        };
        let quoted = format!("\"{}\"", name.replace('"', "\"\""));
        count += sqlx::query(&format!("DELETE FROM {quoted}"))
            .execute(&state.db)
            .await?
            .rows_affected();
        if name == "translation_cache" {
            state
                .ai
                .cache_revision
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    Ok(Json(
        json!({"message":format!("操作成功！共清空 {} 个表，删除 {count} 行数据。",names.len())}),
    ))
}

async fn cleanup_offline(State(state): State<AppState>) -> Result<Json<Value>> {
    let count = sqlx::query("DELETE FROM media_metadata WHERE in_library=FALSE")
        .execute(&state.db)
        .await?
        .rows_affected();
    Ok(Json(
        json!({"message":format!("已清理 {count} 条离线媒体记录"),"data":{"media_metadata_deleted":count}}),
    ))
}
async fn rebuild(State(state): State<AppState>) -> Result<Json<Value>> {
    let mut transaction = state.db.begin().await?;
    sqlx::query("TRUNCATE TABLE emby_users,emby_users_extended RESTART IDENTITY CASCADE")
        .execute(&mut *transaction)
        .await?;
    let count=sqlx::query("UPDATE media_metadata SET in_library=FALSE,emby_item_ids_json='[]'::jsonb,file_sha1_json='[]'::jsonb,file_pickcode_json='[]'::jsonb,asset_details_json=NULL,date_added=NULL,last_updated_at=NOW() WHERE in_library=TRUE OR emby_item_ids_json::text!='[]' OR file_sha1_json::text!='[]' OR file_pickcode_json::text!='[]' OR asset_details_json IS NOT NULL").execute(&mut *transaction).await?.rows_affected();
    transaction.commit().await?;
    Ok(Json(
        json!({"message":"为 Emby 媒体库重建的准备工作已成功完成","details":{"truncated_tables":["emby_users","emby_users_extended"],"updated_rows":{"media_metadata":count}}}),
    ))
}
