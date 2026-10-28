use crate::{
    auth::SessionSigner,
    db,
    error::{ApiError, Result},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub config: Arc<RwLock<Value>>,
    pub http: reqwest::Client,
    pub sessions: Arc<SessionSigner>,
    pub recovery_tokens: Arc<RwLock<HashMap<String, i64>>>,
    pub data_dir: PathBuf,
    pub notifications: Arc<crate::notifications::Service>,
    pub webhooks: Arc<crate::webhook::Service>,
    pub ai: Arc<crate::ai::Service>,
    pub jobs: Arc<crate::jobs::JobManager>,
    pub logs: Arc<crate::logs::LogStore>,
    pub p115: Arc<crate::p115::Service>,
    pub strm: Arc<crate::strm::Service>,
    pub douban: Arc<crate::douban::Service>,
    pub docker: Arc<crate::docker_host::Service>,
}
impl AppState {
    pub async fn new(db: PgPool, data_dir: PathBuf) -> anyhow::Result<Self> {
        db::initialize(&db)
            .await
            .map_err(|e| anyhow::anyhow!(e.1))?;
        let mut config: Value =
            serde_json::from_str(include_str!("../assets/config_defaults.json"))?;
        if let Some(Value::Object(stored)) = db::setting(&db, "dynamic_app_config")
            .await
            .map_err(|e| anyhow::anyhow!(e.1))?
        {
            config.as_object_mut().unwrap().extend(stored);
        }
        migrate_cookie_priority(&db, &mut config).await;
        config["emby_server_id"] = Value::Null;
        let logs = crate::logs::LogStore::open(&data_dir.join("logs"))?;
        logs.configure(&config);
        Ok(Self {
            db,
            config: Arc::new(RwLock::new(config)),
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .build()?,
            sessions: Arc::new(SessionSigner::load(&data_dir)?),
            recovery_tokens: Arc::new(RwLock::new(HashMap::new())),
            notifications: Arc::new(crate::notifications::Service::default()),
            webhooks: Arc::new(crate::webhook::Service::default()),
            ai: Arc::new(crate::ai::Service::default()),
            jobs: Arc::new(crate::jobs::JobManager::default()),
            logs,
            p115: Arc::new(crate::p115::Service::default()),
            strm: Arc::new(crate::strm::Service::default()),
            douban: Arc::new(crate::douban::Service::default()),
            docker: Arc::new(crate::docker_host::Service::default()),
            data_dir,
        })
    }
    pub async fn configured(&self) -> bool {
        let config = self.config.read().await;
        !string(&config, "emby_server_url").is_empty()
            && !string(&config, "emby_api_key").is_empty()
    }
    pub async fn reload_config(&self) -> Result<()> {
        let mut config: Value =
            serde_json::from_str(include_str!("../assets/config_defaults.json"))
                .expect("bundled defaults");
        if let Some(Value::Object(stored)) = db::setting(&self.db, "dynamic_app_config").await? {
            config.as_object_mut().unwrap().extend(stored);
        }
        migrate_cookie_priority(&self.db, &mut config).await;
        config["emby_server_id"] = self.config.read().await["emby_server_id"].clone();
        self.logs.configure(&config);
        *self.config.write().await = config;
        Ok(())
    }
    pub async fn save_config(&self, updates: &Value) -> Result<()> {
        let updates = updates
            .as_object()
            .ok_or_else(|| ApiError::bad("配置必须是 JSON 对象"))?;
        let mut config = self.config.write().await;
        let mut next = config.clone();
        next.as_object_mut().unwrap().extend(updates.clone());
        for key in [
            "emby_api_key",
            "emby_admin_pass",
            "tmdb_api_key",
            "douban_cookie",
            "ai_api_key",
            "telegram_bot_token",
            "emby_webhook_secret",
        ] {
            if next[key] == "********" {
                next[key] = config[key].clone();
            }
        }
        next["emby_server_id"] = config["emby_server_id"].clone();
        let mut persisted = next.clone();
        persisted.as_object_mut().unwrap().remove("emby_server_id");
        db::save_setting(&self.db, "dynamic_app_config", &persisted).await?;
        self.logs.configure(&next);
        *config = next;
        Ok(())
    }
    pub async fn refresh_server_identity(&self) {
        let config = self.config.read().await.clone();
        let base = string(&config, "emby_server_url");
        let key = string(&config, "emby_api_key");
        let mut id = Value::Null;
        if !base.is_empty() && !key.is_empty() {
            let result = async {
                let cached = db::setting(&self.db, "emby_server_id_cache")
                    .await?
                    .unwrap_or(Value::Null);
                let truthy = |value: &Value| match value {
                    Value::Null => false,
                    Value::Bool(value) => *value,
                    Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
                    Value::String(value) => !value.is_empty(),
                    Value::Array(value) => !value.is_empty(),
                    Value::Object(value) => !value.is_empty(),
                };
                let timeout = if truthy(&cached) { 5 } else { 20 };
                tracing::debug!("正在尝试连接 Emby 获取 Server ID (超时设定: {timeout}s)...");
                let info = async {
                    crate::emby::send(
                        self.http
                            .get(format!("{}/System/Info", base.trim_end_matches('/')))
                            .query(&[("api_key", &key)])
                            .timeout(Duration::from_secs(timeout)),
                    )
                    .await?
                    .error_for_status()?
                    .json::<Value>()
                    .await
                }
                .await
                .inspect_err(|error| {
                    tracing::warn!("获取 Emby 服务器信息失败 (可能是服务器离线): {error}");
                });
                if let Ok(info) = info
                    && truthy(&info["Id"])
                {
                    id = info["Id"].clone();
                    tracing::trace!("成功获取到 Emby Server ID: {id}");
                    db::save_setting(&self.db, "emby_server_id_cache", &id).await?;
                } else if truthy(&cached) {
                    id = cached;
                    tracing::warn!("无法连接 Emby 服务器，已使用缓存的 Server ID: {id} 继续启动。");
                } else {
                    tracing::error!("无法连接 Emby 且本地无缓存 Server ID，部分功能可能受限。");
                }
                Ok::<_, ApiError>(())
            }
            .await;
            if let Err(error) = result {
                tracing::warn!("缓存 Emby Server ID 失败：{}", error.1);
            }
        }
        self.config.write().await["emby_server_id"] = id;
    }
    pub async fn emby(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Value> {
        let cfg = self.config.read().await;
        let base = string(&cfg, "emby_server_url")
            .trim_end_matches('/')
            .to_string();
        let key = string(&cfg, "emby_api_key");
        if base.is_empty() || key.is_empty() {
            return Err(ApiError::bad("Emby 服务器配置不完整"));
        }
        let mut request = self
            .http
            .request(method, format!("{base}/{path}"))
            .header("X-Emby-Token", key)
            .timeout(Duration::from_secs(
                cfg["emby_api_timeout"]
                    .as_u64()
                    .filter(|seconds| *seconds > 0)
                    .unwrap_or(60),
            ))
            .query(query);
        drop(cfg);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = crate::emby::send(request).await?.error_for_status()?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(json!({"success":true}));
        }
        let bytes = response.bytes().await?;
        if bytes.is_empty() {
            return Ok(json!({"success":true}));
        }
        serde_json::from_slice(&bytes).map_err(|_| ApiError::bad("Emby 返回了无效 JSON"))
    }
    pub async fn item_delay(&self, context: &crate::jobs::JobContext) -> Result<()> {
        let seconds = self.config.read().await["delay_between_items_sec"]
            .as_f64()
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
            .unwrap_or(0.0);
        context.sleep(Duration::from_secs_f64(seconds)).await
    }
}
async fn migrate_cookie_priority(db: &PgPool, config: &mut Value) {
    let migrated = db::setting(db, "p115_cookie_priority_default_v1")
        .await
        .unwrap_or(None);
    if migrated.is_some_and(|value| value == true) {
        return;
    }
    if config["p115_api_priority"] != "cookie" {
        config["p115_api_priority"] = json!("cookie");
        if let Err(error) = db::save_setting(db, "dynamic_app_config", config).await {
            tracing::warn!(
                "保存 115 Cookie 优先级迁移失败，将在下次启动重试：{}",
                error.1
            );
            return;
        }
        tracing::info!("115 接口默认优先级已迁移为 Cookie。");
    }
    if let Err(error) = db::save_setting(db, "p115_cookie_priority_default_v1", &json!(true)).await
    {
        tracing::warn!(
            "保存 115 Cookie 优先级迁移标记失败，将在下次启动重试：{}",
            error.1
        );
    }
}
pub fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}
