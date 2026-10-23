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
        db::save_setting(&self.db, "dynamic_app_config", &next).await?;
        self.logs.configure(&next);
        *config = next;
        Ok(())
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
            .query(query);
        drop(cfg);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?.error_for_status()?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(json!({"success":true}));
        }
        let bytes = response.bytes().await?;
        if bytes.is_empty() {
            return Ok(json!({"success":true}));
        }
        serde_json::from_slice(&bytes).map_err(|_| ApiError::bad("Emby 返回了无效 JSON"))
    }
}
pub fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}
