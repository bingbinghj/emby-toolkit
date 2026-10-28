use crate::{
    AppState, db,
    error::{ApiError, Result},
};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, header},
    middleware::Next,
    response::Response,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{fs::OpenOptions, io::Write, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub user_id: String,
    pub name: String,
    pub setup_mode: bool,
    pub expires: i64,
}
pub struct SessionSigner(Vec<u8>);
impl SessionSigner {
    pub fn load(directory: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        let path = directory.join(".rust_session_secret");
        if !path.exists() {
            let mut key = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut key);
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(&key)?;
        }
        let key = std::fs::read(path)?;
        if key.len() < 32 {
            return Err(std::io::Error::other("会话密钥长度无效"));
        }
        Ok(Self(key))
    }
    pub fn sign(&self, session: &Session) -> String {
        self.sign_value(&serde_json::to_value(session).expect("session serializable"))
    }
    pub fn sign_administrator(&self, session: &Session) -> String {
        let mut value = serde_json::to_value(session).expect("session serializable");
        value["emby_is_admin"] = serde_json::json!(true);
        self.sign_value(&value)
    }
    fn sign_value(&self, value: &serde_json::Value) -> String {
        let payload =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).expect("session serializable"));
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC key");
        mac.update(payload.as_bytes());
        format!(
            "{payload}.{}",
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        )
    }
    pub fn verify(&self, token: &str) -> Option<Session> {
        let (payload, signature) = token.split_once('.')?;
        let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).ok()?;
        mac.update(payload.as_bytes());
        mac.verify_slice(&signature).ok()?;
        let session: Session =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
        (session.expires > chrono::Utc::now().timestamp()).then_some(session)
    }
    pub fn from_headers(&self, headers: &HeaderMap) -> Option<Session> {
        headers
            .get(header::COOKIE)?
            .to_str()
            .ok()?
            .split(';')
            .find_map(|pair| {
                let (key, value) = pair.trim().split_once('=')?;
                (key == "etk_session").then(|| self.verify(value)).flatten()
            })
    }
    pub fn administrator_claim(&self, headers: &HeaderMap) -> Option<bool> {
        headers
            .get(header::COOKIE)?
            .to_str()
            .ok()?
            .split(';')
            .find_map(|pair| {
                let (key, token) = pair.trim().split_once('=')?;
                if key != "etk_session" {
                    return None;
                }
                self.verify(token)?;
                let (payload, _) = token.split_once('.')?;
                let value: serde_json::Value =
                    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
                value["emby_is_admin"].as_bool()
            })
    }
}
pub async fn require_admin(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response> {
    let session = state
        .sessions
        .from_headers(request.headers())
        .ok_or_else(ApiError::forbidden)?;
    if session.user_id.is_empty() || !db::is_admin(&state.db, &session.user_id).await? {
        return Err(ApiError::forbidden());
    }
    Ok(next.run(request).await)
}
pub fn cookie(token: &str) -> String {
    format!("etk_session={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age=2678400")
}
