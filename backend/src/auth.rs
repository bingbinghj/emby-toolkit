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

// MoviePilot v3 ACCESS_TOKEN_EXPIRE_MINUTES defaults to 60 * 24 * 8.
pub const LOGIN_SESSION_SECONDS: i64 = 8 * 24 * 60 * 60;

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
        self.sign_administrator_with_remember(session, true)
    }
    pub fn sign_administrator_with_remember(&self, session: &Session, remember: bool) -> String {
        let mut value = serde_json::to_value(session).expect("session serializable");
        value["emby_is_admin"] = serde_json::json!(true);
        value["remember"] = serde_json::json!(remember);
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
        self.claims_from_headers(headers)?["emby_is_admin"].as_bool()
    }
    pub fn remember_claim(&self, headers: &HeaderMap) -> Option<bool> {
        // Sessions issued before this preference existed were persistent.
        Some(
            self.claims_from_headers(headers)?["remember"]
                .as_bool()
                .unwrap_or(true),
        )
    }
    fn claims_from_headers(&self, headers: &HeaderMap) -> Option<serde_json::Value> {
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
                serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
            })
    }
}
pub async fn require_admin(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response> {
    if administrator_session(&state, request.headers())
        .await?
        .is_none()
    {
        return Err(ApiError::forbidden());
    }
    Ok(next.run(request).await)
}
pub async fn administrator_session(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<Session>> {
    let Some(session) = state.sessions.from_headers(headers) else {
        return Ok(None);
    };
    if session.user_id.is_empty() || !db::is_admin(&state.db, &session.user_id).await? {
        return Ok(None);
    }
    Ok(Some(session))
}
pub fn cookie(token: &str, expires: i64, remember: bool) -> String {
    let mut value = format!("etk_session={token}; HttpOnly; SameSite=Lax; Path=/");
    if remember {
        value.push_str(&format!(
            "; Max-Age={}",
            (expires - chrono::Utc::now().timestamp()).max(0)
        ));
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_preference_is_signed_and_legacy_sessions_remain_persistent() {
        let signer = SessionSigner(vec![7; 32]);
        let session = Session {
            user_id: "test-admin".into(),
            name: "Admin".into(),
            setup_mode: false,
            expires: chrono::Utc::now().timestamp() + LOGIN_SESSION_SECONDS,
        };
        for remember in [false, true] {
            let token = signer.sign_administrator_with_remember(&session, remember);
            let headers = HeaderMap::from_iter([(
                header::COOKIE,
                format!("etk_session={token}").parse().unwrap(),
            )]);
            assert_eq!(signer.remember_claim(&headers), Some(remember));
            assert_eq!(signer.administrator_claim(&headers), Some(true));
            assert_eq!(signer.from_headers(&headers).unwrap().user_id, "test-admin");

            let (payload, signature) = token.split_once('.').unwrap();
            let mut value: serde_json::Value =
                serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
            value["remember"] = serde_json::json!(!remember);
            let forged = format!(
                "{}.{}",
                URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap()),
                signature
            );
            let headers = HeaderMap::from_iter([(
                header::COOKIE,
                format!("etk_session={forged}").parse().unwrap(),
            )]);
            assert_eq!(signer.remember_claim(&headers), None);
        }
        let mut old_value = serde_json::to_value(session).unwrap();
        old_value["emby_is_admin"] = serde_json::json!(true);
        let old_token = signer.sign_value(&old_value);
        let headers = HeaderMap::from_iter([(
            header::COOKIE,
            format!("etk_session={old_token}").parse().unwrap(),
        )]);
        assert_eq!(signer.remember_claim(&headers), Some(true));
    }

    #[test]
    fn unchecked_login_cookie_is_session_only_and_remembered_cookie_matches_token_expiry() {
        let now = chrono::Utc::now().timestamp();
        let transient = cookie("fixture", now + LOGIN_SESSION_SECONDS, false);
        assert_eq!(
            transient,
            "etk_session=fixture; HttpOnly; SameSite=Lax; Path=/"
        );
        let persistent = cookie("fixture", now + LOGIN_SESSION_SECONDS, true);
        let age: i64 = persistent
            .rsplit("Max-Age=")
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!((LOGIN_SESSION_SECONDS - 1..=LOGIN_SESSION_SECONDS).contains(&age));
        assert_eq!(
            cookie("fixture", now - 1, true),
            "etk_session=fixture; HttpOnly; SameSite=Lax; Path=/; Max-Age=0"
        );
    }
}
