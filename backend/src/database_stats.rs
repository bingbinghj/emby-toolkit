use crate::{AppState, error::Result};
use axum::{Json, Router, extract::State, routing::get};
use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/database/stats/core", get(core))
        .route("/api/database/stats/system", get(system))
        .route("/api/database/stats/library", get(library))
        .route("/api/database/stats/rankings", get(rankings))
}
async fn one(state: &AppState, sql: &str) -> Result<Value> {
    Ok(
        sqlx::query_scalar(&format!("SELECT to_jsonb(s) FROM ({sql}) s"))
            .fetch_one(&state.db)
            .await?,
    )
}
async fn many(state: &AppState, sql: &str) -> Result<Vec<Value>> {
    Ok(
        sqlx::query_scalar(&format!("SELECT to_jsonb(s) FROM ({sql}) s"))
            .fetch_all(&state.db)
            .await?,
    )
}
async fn core(State(state): State<AppState>) -> Result<Json<Value>> {
    Ok(Json(
        json!({"status":"success","data":one(&state,include_str!("../assets/statistics/core.sql")).await?}),
    ))
}
async fn system(State(state): State<AppState>) -> Result<Json<Value>> {
    Ok(Json(
        json!({"status":"success","data":one(&state,include_str!("../assets/statistics/system.sql")).await?}),
    ))
}
async fn library(State(state): State<AppState>) -> Result<Json<Value>> {
    let counts = one(&state, include_str!("../assets/statistics/library.sql")).await?;
    let resolution = many(&state, include_str!("../assets/statistics/resolution.sql")).await?;
    let sql = format!(
        "SELECT to_jsonb(s) FROM ({}) s",
        include_str!("../assets/statistics/weekly.sql")
    );
    let weekly: Vec<Value> = sqlx::query_scalar(&sql)
        .bind("Asia/Shanghai")
        .bind(7_i32)
        .bind("%date_added_to_library%")
        .fetch_all(&state.db)
        .await?;
    Ok(Json(
        json!({"status":"success","data":{"movies_in_library":counts["media_movies_in_library"],"series_in_library":counts["media_series_in_library"],"episodes_in_library":counts["media_episodes_in_library"],"resolution_stats":resolution,"weekly_added_stats":weekly}}),
    ))
}
async fn rankings(State(state): State<AppState>) -> Result<Json<Value>> {
    let sql = format!(
        "SELECT to_jsonb(s) FROM ({}) s",
        include_str!("../assets/statistics/daily_groups.sql")
    );
    let daily: Vec<Value> = sqlx::query_scalar(&sql)
        .bind("%date_added_to_library%")
        .bind("Asia/Shanghai")
        .bind("Asia/Shanghai")
        .bind(5_i64)
        .fetch_all(&state.db)
        .await?;
    let sql = format!(
        "SELECT to_jsonb(s) FROM ({}) s",
        include_str!("../assets/statistics/groups.sql")
    );
    let groups: Vec<Value> = sqlx::query_scalar(&sql)
        .bind("%date_added_to_library%")
        .bind(5_i64)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(
        json!({"status":"success","data":{"release_group_ranking":daily,"historical_release_group_ranking":groups}}),
    ))
}
