use crate::error::Result;
use serde_json::Value;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{collections::HashMap, path::Path, str::FromStr};

pub async fn connect(data_dir: &Path) -> anyhow::Result<PgPool> {
    let mut ini = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(data_dir.join("config.ini")) {
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('=') {
                ini.insert(key.trim().to_lowercase(), value.trim().to_string());
            }
        }
    }
    let setting = |env: &str, key: &str, default: &str| {
        std::env::var(env)
            .ok()
            .or_else(|| ini.get(key).cloned())
            .unwrap_or(default.into())
    };
    let options = if let Ok(url) = std::env::var("DATABASE_URL") {
        PgConnectOptions::from_str(&url)?
    } else {
        PgConnectOptions::new()
            .host(&setting("DB_HOST", "db_host", "localhost"))
            .port(setting("DB_PORT", "db_port", "5432").parse()?)
            .username(&setting("DB_USER", "db_user", "postgres"))
            .password(&setting("DB_PASSWORD", "db_password", ""))
            .database(&setting("DB_NAME", "db_name", "emby_toolkit"))
    };
    Ok(PgPoolOptions::new()
        .max_connections(12)
        .connect_with(options)
        .await?)
}

pub async fn initialize(pool: &PgPool) -> Result<()> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('emby-toolkit:schema'))")
        .execute(&mut *transaction)
        .await?;
    sqlx::raw_sql(include_str!("../assets/schema.sql"))
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn setting(pool: &PgPool, key: &str) -> Result<Option<Value>> {
    let value: Option<Option<Value>> =
        sqlx::query_scalar("SELECT value_json FROM app_settings WHERE setting_key = $1")
            .bind(key)
            .fetch_optional(pool)
            .await?;
    Ok(value.flatten().filter(|value| !value.is_null()))
}
pub async fn save_setting(pool: &PgPool, key: &str, value: &Value) -> Result<()> {
    sqlx::query("INSERT INTO app_settings(setting_key,value_json,last_updated_at) VALUES($1,$2,NOW()) ON CONFLICT(setting_key) DO UPDATE SET value_json=EXCLUDED.value_json,last_updated_at=NOW()")
        .bind(key).bind(value).execute(pool).await?;
    Ok(())
}
pub async fn patch_setting(pool: &PgPool, key: &str, patch: &Value) -> Result<()> {
    if !patch.is_object() {
        return Err(crate::error::ApiError::bad("配置更新必须是 JSON 对象"));
    }
    sqlx::query("INSERT INTO app_settings(setting_key,value_json,last_updated_at) VALUES($1,$2,NOW()) ON CONFLICT(setting_key) DO UPDATE SET value_json=(CASE WHEN jsonb_typeof(app_settings.value_json)='object' THEN app_settings.value_json ELSE '{}'::jsonb END) || EXCLUDED.value_json,last_updated_at=NOW()")
        .bind(key).bind(patch).execute(pool).await?;
    Ok(())
}
pub async fn is_admin(pool: &PgPool, user_id: &str) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT COALESCE(is_administrator,FALSE) FROM emby_users WHERE id=$1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(false))
}
