//! GhostID QR login. The server signs each challenge with the provider key (see
//! `ghostid_provider`), the verifier relays it to the wallet, and every protected request
//! re-checks the verifier. Sessions, attempts, and wallet users live in a SQLite file.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use base64::Engine;
use chrono::Utc;
use ring::rand::{SecureRandom, SystemRandom};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Sqlite, SqliteConnection, SqlitePool, Transaction};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::ghostid_provider::{self, SignedChallenge};

/// Cookie that binds a login attempt to the browser that started it.
pub const SESSION_COOKIE: &str = "__Host-ghostid";
const SESSION_TTL_SECS: i64 = 3600;
const ATTEMPT_TTL_SECS: i64 = 180;
const MAX_PENDING_PER_SESSION: i64 = 5;
const DURATION_DAYS: u32 = 30;
const TIER: &str = "basic";
const ACTION: &str = "login";
const PERMISSION: &str = "editor";

const SCHEMA: [&str; 3] = [
    "CREATE TABLE IF NOT EXISTS ghostid_users (address TEXT PRIMARY KEY, user_id TEXT NOT NULL, created_at INTEGER NOT NULL)",
    "CREATE TABLE IF NOT EXISTS ghostid_sessions (id TEXT PRIMARY KEY, user_id TEXT, address TEXT, public_key TEXT, token_id TEXT, challenge_id TEXT, expires_at INTEGER NOT NULL)",
    "CREATE TABLE IF NOT EXISTS ghostid_attempts (challenge_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, expected_json TEXT NOT NULL, expires_at INTEGER NOT NULL, consumed INTEGER NOT NULL DEFAULT 0)",
];

/// GhostID settings. Login stays disabled unless `GHOSTID_DOMAIN` is set.
#[derive(Clone, Debug)]
pub struct GhostIdConfig {
    pub domain: String,
    pub verifier_url: String,
    pub require_token: bool,
    pub db_path: String,
}

impl GhostIdConfig {
    pub fn from_env() -> Self {
        let domain = std::env::var("GHOSTID_DOMAIN")
            .or_else(|_| std::env::var("SITE_DOMAIN"))
            .ok()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "mothership.rest".to_string());
        let verifier_url = std::env::var("GHOSTID_AUTH_URL")
            .or_else(|_| std::env::var("GHOSTID_VERIFIER_URL"))
            .unwrap_or_else(|_| "https://api.ghostid.rest".to_string())
            .trim_end_matches('/')
            .to_string();
        let require_token = std::env::var("REQUIRE_AUTH_TOKEN")
            .or_else(|_| std::env::var("GHOSTID_REQUIRE_TOKEN"))
            .map(|v| v != "false")
            .unwrap_or(true);
        let db_path = std::env::var("SESSION_DB_PATH")
            .or_else(|_| std::env::var("GHOSTID_DB_PATH"))
            .unwrap_or_else(|_| "data/ghostid.db".to_string());
        Self {
            domain,
            verifier_url,
            require_token,
            db_path,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GhostIdError {
    pub status: StatusCode,
    pub message: String,
}

impl GhostIdError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
    }

    fn unauthenticated(message: &str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    fn unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "GhostID verifier is unavailable. Try again; no cached authentication was accepted.",
        )
    }

    fn internal(err: impl std::fmt::Display) -> Self {
        error!("GhostID storage error: {}", err);
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "GhostID login failed")
    }
}

/// Where the browser shows the QR, plus a new session cookie when one was created.
pub struct StartedLogin {
    pub challenge_id: String,
    pub qr_url: String,
    pub expires_at: i64,
    pub new_session_token: Option<String>,
}

/// The verified wallet identity, as returned by the verifier.
pub struct Identity {
    pub address: String,
    pub public_key: String,
    pub token_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostIdUser {
    pub user_id: String,
    pub address: String,
    pub domain: String,
}

pub enum PollOutcome {
    Pending,
    Verified { session_token: String, user: GhostIdUser },
}

struct SessionRow {
    id: String,
    user_id: Option<String>,
    address: Option<String>,
    public_key: Option<String>,
    token_id: Option<String>,
    challenge_id: Option<String>,
}

#[derive(Clone)]
pub struct GhostIdService {
    config: Arc<GhostIdConfig>,
    pool: SqlitePool,
    http: reqwest::Client,
    #[cfg(test)]
    test_key: Option<Arc<ring::signature::Ed25519KeyPair>>,
}

impl GhostIdService {
    pub async fn connect(config: GhostIdConfig) -> anyhow::Result<Self> {
        if let Some(parent) = Path::new(&config.db_path).parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let options = SqliteConnectOptions::new()
            .filename(&config.db_path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new().max_connections(4).connect_with(options).await?;
        for statement in SCHEMA {
            sqlx::query(statement).execute(&pool).await?;
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let service = Self {
            config: Arc::new(config),
            pool,
            http,
            #[cfg(test)]
            test_key: None,
        };
        let now = Utc::now().timestamp();
        sqlx::query("DELETE FROM ghostid_sessions WHERE expires_at <= ?").bind(now).execute(&service.pool).await?;
        sqlx::query("DELETE FROM ghostid_attempts WHERE expires_at <= ?").bind(now).execute(&service.pool).await?;
        info!("GhostID login enabled for {} via {}", service.config.domain, service.config.verifier_url);
        Ok(service)
    }

    /// Creates a login attempt bound to the caller's session, creating that session if needed.
    pub async fn start_login(&self, cookie: Option<&str>) -> Result<StartedLogin, GhostIdError> {
        #[cfg(test)]
        let using_test_key = self.test_key.is_some();
        #[cfg(not(test))]
        let using_test_key = false;

        if !using_test_key {
            if !ghostid_provider::is_configured() {
                return Err(GhostIdError::new(StatusCode::SERVICE_UNAVAILABLE, "GhostID provider key is not configured"));
            }
            if let Err(reason) = ghostid_provider::verify_published_key(&self.config.domain).await {
                warn!("GhostID provider trust check failed: {}", reason);
                return Err(GhostIdError::new(
                    StatusCode::FORBIDDEN,
                    "GhostID provider trust check failed; the DNS record or provider key does not match",
                ));
            }
        }

        let now = Utc::now().timestamp();
        let (session_id, new_session_token) = match self.load_session(cookie).await? {
            Some(session) => (session.id, None),
            None => {
                let token = random_token()?;
                let id = session_key(&token);
                self.insert_session(&id, now + SESSION_TTL_SECS).await?;
                (id, Some(token))
            }
        };

        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ghostid_attempts WHERE session_id = ? AND consumed = 0 AND expires_at > ?",
        )
        .bind(&session_id)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(GhostIdError::internal)?;
        if pending >= MAX_PENDING_PER_SESSION {
            return Err(GhostIdError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many pending logins. Wait three minutes before trying again.",
            ));
        }

        let challenge = SignedChallenge {
            challenge_id: Uuid::new_v4().to_string(),
            domain: self.config.domain.clone(),
            action: ACTION.to_string(),
            nonce: base64::engine::general_purpose::STANDARD.encode(random_bytes::<32>()?),
            issued_at: now,
            expires_at: now + ATTEMPT_TTL_SECS,
            permissions: vec![PERMISSION.to_string()],
            duration_days: DURATION_DAYS,
            tier: TIER.to_string(),
        };
        let (provider_public_key, signature) = self.sign(&challenge)?;
        let body = json!({
            "challengeId": challenge.challenge_id,
            "domain": challenge.domain,
            "action": challenge.action,
            "nonce": challenge.nonce,
            "issuedAt": challenge.issued_at,
            "expiresAt": challenge.expires_at,
            "permissions": challenge.permissions,
            "duration_days": challenge.duration_days,
            "tier": challenge.tier,
            "providerPublicKey": provider_public_key,
            "providerSignature": signature,
        });

        let issued = self.post_challenge(&body).await?;
        if !echoes(&issued, &body) {
            return Err(GhostIdError::new(
                StatusCode::BAD_GATEWAY,
                "The verifier altered the signed challenge. Check permission normalization and provider configuration.",
            ));
        }

        sqlx::query(
            "INSERT INTO ghostid_attempts (challenge_id, session_id, expected_json, expires_at, consumed) VALUES (?, ?, ?, ?, 0)",
        )
        .bind(&challenge.challenge_id)
        .bind(&session_id)
        .bind(body.to_string())
        .bind(challenge.expires_at)
        .execute(&self.pool)
        .await
        .map_err(GhostIdError::internal)?;

        Ok(StartedLogin {
            qr_url: format!("{}/auth/qr/{}", self.config.verifier_url, challenge.challenge_id),
            challenge_id: challenge.challenge_id,
            expires_at: challenge.expires_at,
            new_session_token,
        })
    }

    /// Checks one attempt. Only a verified result for this domain completes the login,
    /// and each attempt is claimed exactly once.
    pub async fn poll_login(&self, cookie: Option<&str>, challenge_id: &str) -> Result<PollOutcome, GhostIdError> {
        let session = self.require_session(cookie).await?;
        let attempt: Option<(String, i64, i64)> = sqlx::query_as(
            "SELECT expected_json, consumed, expires_at FROM ghostid_attempts WHERE challenge_id = ? AND session_id = ?",
        )
        .bind(challenge_id)
        .bind(&session.id)
        .fetch_optional(&self.pool)
        .await
        .map_err(GhostIdError::internal)?;
        let Some((expected_json, consumed, expires_at)) = attempt else {
            return Err(GhostIdError::new(
                StatusCode::NOT_FOUND,
                "Login attempt does not belong to this browser session.",
            ));
        };
        if consumed != 0 || expires_at < Utc::now().timestamp() {
            return Err(GhostIdError::new(StatusCode::GONE, "This login attempt is expired or already used."));
        }
        let expected: Value = serde_json::from_str(&expected_json)
            .map_err(|_| GhostIdError::new(StatusCode::GONE, "Website configuration changed. Start a fresh login."))?;
        if expected.get("challengeId").and_then(Value::as_str) != Some(challenge_id)
            || expected.get("domain").and_then(Value::as_str) != Some(self.config.domain.as_str())
            || expected.get("action").and_then(Value::as_str) != Some(ACTION)
        {
            return Err(GhostIdError::new(StatusCode::GONE, "Website configuration changed. Start a fresh login."));
        }

        let Some(result) = self.fetch_poll(challenge_id).await? else {
            return Ok(PollOutcome::Pending);
        };
        if matches!(result.get("status").and_then(Value::as_str), Some("pending") | Some("processing")) {
            return Ok(PollOutcome::Pending);
        }
        let identity = self.verified_identity(&result)?;
        let now = Utc::now().timestamp();
        if expires_at < now {
            return Err(GhostIdError::new(StatusCode::GONE, "Login expired while waiting for verification."));
        }

        let mut tx = self.pool.begin().await.map_err(GhostIdError::internal)?;
        let alive: Option<(String,)> = sqlx::query_as("SELECT id FROM ghostid_sessions WHERE id = ? AND expires_at > ?")
            .bind(&session.id)
            .bind(now)
            .fetch_optional(&mut *tx)
            .await
            .map_err(GhostIdError::internal)?;
        if alive.is_none() {
            return Err(GhostIdError::new(
                StatusCode::CONFLICT,
                "This browser session ended while waiting. Reload and start again.",
            ));
        }
        let user_id = ensure_wallet_user(&mut tx, &identity.address, now).await?;
        let token = random_token()?;
        let new_session_id = session_key(&token);
        let claimed = sqlx::query(
            "UPDATE ghostid_attempts SET consumed = 1, session_id = ? WHERE challenge_id = ? AND session_id = ? AND consumed = 0",
        )
        .bind(&new_session_id)
        .bind(challenge_id)
        .bind(&session.id)
        .execute(&mut *tx)
        .await
        .map_err(GhostIdError::internal)?
        .rows_affected();
        if claimed != 1 {
            return Err(GhostIdError::new(StatusCode::CONFLICT, "This login was already accepted."));
        }

        insert_verified_session(&mut tx, &new_session_id, &user_id, &identity, challenge_id, now + SESSION_TTL_SECS).await?;
        sqlx::query("DELETE FROM ghostid_sessions WHERE id = ?")
            .bind(&session.id)
            .execute(&mut *tx)
            .await
            .map_err(GhostIdError::internal)?;
        tx.commit().await.map_err(GhostIdError::internal)?;

        info!("GhostID login accepted for wallet {}", identity.address);
        Ok(PollOutcome::Verified {
            session_token: token,
            user: GhostIdUser { user_id, address: identity.address, domain: self.config.domain.clone() },
        })
    }

    /// Re-checks the signed-in session against the verifier on every call. Any doubt
    /// clears the local session or denies access; a cached login is never accepted.
    pub async fn current_user(&self, cookie: Option<&str>) -> Result<GhostIdUser, GhostIdError> {
        let session = self.require_session(cookie).await?;
        let (Some(user_id), Some(address), Some(public_key), Some(challenge_id)) =
            (session.user_id.clone(), session.address.clone(), session.public_key.clone(), session.challenge_id.clone())
        else {
            return Err(GhostIdError::unauthenticated("Sign in first."));
        };

        let fresh = match self.fetch_poll(&challenge_id).await? {
            Some(result) => self.verified_identity(&result),
            None => Err(GhostIdError::unauthenticated("Authentication was not verified for this website.")),
        };
        match fresh {
            Ok(identity)
                if identity.address == address && identity.public_key == public_key && identity.token_id == session.token_id =>
            {
                Ok(GhostIdUser { user_id, address, domain: self.config.domain.clone() })
            }
            Ok(_) => {
                self.delete_session(&session.id).await?;
                Err(GhostIdError::unauthenticated("The session identity or token changed."))
            }
            Err(err) => {
                self.delete_session(&session.id).await?;
                Err(err)
            }
        }
    }

    /// Deletes only the local session. The wallet and verifier are not contacted.
    pub async fn logout(&self, cookie: Option<&str>) -> Result<(), GhostIdError> {
        if let Some(token) = cookie.filter(|t| !t.is_empty()) {
            self.delete_session(&session_key(token)).await?;
        }
        Ok(())
    }

    fn verified_identity(&self, result: &Value) -> Result<Identity, GhostIdError> {
        let rejected = || GhostIdError::unauthenticated("Authentication was not verified for this website.");
        let status_ok = result.get("status").and_then(Value::as_str) == Some("verified");
        let domain_ok = result.get("domain").and_then(Value::as_str) == Some(self.config.domain.as_str());
        let permission_ok = result
            .get("permissions")
            .and_then(Value::as_array)
            .is_some_and(|permissions| permissions.iter().any(|p| p.as_str() == Some(PERMISSION)));
        if !(status_ok && domain_ok && permission_ok) {
            return Err(rejected());
        }

        let public_key = result.get("publicKey").and_then(Value::as_str).ok_or_else(rejected)?;
        let address = result.get("address").and_then(Value::as_str).ok_or_else(rejected)?;
        let key_bytes = base64::engine::general_purpose::STANDARD.decode(public_key).map_err(|_| rejected())?;
        if key_bytes.len() != 32 || wallet_address(&key_bytes) != address {
            return Err(GhostIdError::unauthenticated("Wallet identity does not match."));
        }

        let token_id = result.get("token_id").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string);
        if self.config.require_token && token_id.is_none() {
            return Err(GhostIdError::unauthenticated(
                "This website requires a current GhostID token. Enable token mediation on your verifier.",
            ));
        }
        if token_id.is_some() {
            let expiry = result
                .get("expires_at")
                .and_then(Value::as_str)
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
            if expiry.map_or(true, |e| e.timestamp() <= Utc::now().timestamp()) {
                return Err(GhostIdError::unauthenticated("GhostID token has expired."));
            }
        }

        Ok(Identity { address: address.to_string(), public_key: public_key.to_string(), token_id })
    }

    async fn post_challenge(&self, body: &Value) -> Result<Value, GhostIdError> {
        let url = format!("{}/auth/challenge", self.config.verifier_url);
        let response = self.http.post(url).json(body).send().await.map_err(|e| {
            warn!("GhostID verifier unavailable: {}", e);
            GhostIdError::unavailable()
        })?;
        read_verifier_json(response).await
    }

    /// Returns `None` while the wallet has not responded yet (the verifier answers 404).
    async fn fetch_poll(&self, challenge_id: &str) -> Result<Option<Value>, GhostIdError> {
        let url = format!("{}/auth/poll/{}", self.config.verifier_url, urlencoding::encode(challenge_id));
        let response = self.http.get(url).send().await.map_err(|e| {
            warn!("GhostID verifier unavailable: {}", e);
            GhostIdError::unavailable()
        })?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        read_verifier_json(response).await.map(Some)
    }

    async fn insert_session(&self, id: &str, expires_at: i64) -> Result<(), GhostIdError> {
        sqlx::query("INSERT INTO ghostid_sessions (id, expires_at) VALUES (?, ?)")
            .bind(id)
            .bind(expires_at)
            .execute(&self.pool)
            .await
            .map_err(GhostIdError::internal)?;
        Ok(())
    }

    async fn delete_session(&self, id: &str) -> Result<(), GhostIdError> {
        sqlx::query("DELETE FROM ghostid_sessions WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(GhostIdError::internal)?;
        Ok(())
    }

    async fn load_session(&self, cookie: Option<&str>) -> Result<Option<SessionRow>, GhostIdError> {
        let Some(token) = cookie.filter(|t| !t.is_empty()) else {
            return Ok(None);
        };
        let row: Option<(String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)> =
            sqlx::query_as(
                "SELECT id, user_id, address, public_key, token_id, challenge_id FROM ghostid_sessions WHERE id = ? AND expires_at > ?",
            )
            .bind(session_key(token))
            .bind(Utc::now().timestamp())
            .fetch_optional(&self.pool)
            .await
            .map_err(GhostIdError::internal)?;
        Ok(row.map(|(id, user_id, address, public_key, token_id, challenge_id)| SessionRow {
            id,
            user_id,
            address,
            public_key,
            token_id,
            challenge_id,
        }))
    }

    async fn require_session(&self, cookie: Option<&str>) -> Result<SessionRow, GhostIdError> {
        self.load_session(cookie).await?.ok_or_else(|| GhostIdError::unauthenticated("Sign in first."))
    }

    fn sign(&self, challenge: &SignedChallenge) -> Result<(String, String), GhostIdError> {
        #[cfg(test)]
        if let Some(key) = &self.test_key {
            use ring::signature::KeyPair;
            let public_key = base64::engine::general_purpose::STANDARD.encode(key.public_key().as_ref());
            let sig = key.sign(ghostid_provider::provider_message(challenge).as_bytes());
            let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.as_ref());
            return Ok((public_key, sig_b64));
        }
        let public_key = ghostid_provider::public_key_b64()
            .ok_or_else(|| GhostIdError::new(StatusCode::SERVICE_UNAVAILABLE, "GhostID provider key is not configured"))?;
        let signature = ghostid_provider::sign_challenge(challenge)
            .ok_or_else(|| GhostIdError::new(StatusCode::SERVICE_UNAVAILABLE, "GhostID provider key is not configured"))?;
        Ok((public_key, signature))
    }
}

async fn read_verifier_json(response: reqwest::Response) -> Result<Value, GhostIdError> {
    if !response.status().is_success() {
        let code = response.status().as_u16();
        let status = match code {
            400 => StatusCode::BAD_REQUEST,
            403 => StatusCode::FORBIDDEN,
            _ => StatusCode::BAD_GATEWAY,
        };
        warn!("GhostID verifier rejected request: {}", code);
        return Err(GhostIdError::new(
            status,
            format!("GhostID verifier rejected the request ({code}). Check provider trust and service configuration."),
        ));
    }
    response
        .json::<Value>()
        .await
        .map_err(|_| GhostIdError::new(StatusCode::BAD_GATEWAY, "GhostID verifier returned an invalid response."))
}

/// True when the verifier accepted the challenge unchanged.
fn echoes(issued: &Value, sent: &Value) -> bool {
    if issued.get("protocolVersion") != Some(&json!(1)) || issued.get("used") != Some(&json!(false)) {
        return false;
    }
    sent.as_object().map_or(false, |fields| fields.iter().all(|(key, value)| issued.get(key) == Some(value)))
}

async fn ensure_wallet_user(tx: &mut Transaction<'_, Sqlite>, address: &str, now: i64) -> Result<String, GhostIdError> {
    let conn: &mut SqliteConnection = tx;
    sqlx::query("INSERT INTO ghostid_users (address, user_id, created_at) VALUES (?, ?, ?) ON CONFLICT(address) DO NOTHING")
        .bind(address)
        .bind(Uuid::new_v4().to_string())
        .bind(now)
        .execute(&mut *conn)
        .await
        .map_err(GhostIdError::internal)?;
    let (user_id,): (String,) = sqlx::query_as("SELECT user_id FROM ghostid_users WHERE address = ?")
        .bind(address)
        .fetch_one(&mut *conn)
        .await
        .map_err(GhostIdError::internal)?;
    Ok(user_id)
}

async fn insert_verified_session(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
    user_id: &str,
    identity: &Identity,
    challenge_id: &str,
    expires_at: i64,
) -> Result<(), GhostIdError> {
    sqlx::query(
        "INSERT INTO ghostid_sessions (id, user_id, address, public_key, token_id, challenge_id, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(user_id)
    .bind(&identity.address)
    .bind(&identity.public_key)
    .bind(&identity.token_id)
    .bind(challenge_id)
    .bind(expires_at)
    .execute(&mut **tx)
    .await
    .map_err(GhostIdError::internal)?;
    Ok(())
}

/// Wallet address as the GhostID protocol derives it: "ghostid" + base58(sha256(pk)[..20]).
pub fn wallet_address(public_key: &[u8]) -> String {
    format!("ghostid{}", bs58::encode(&Sha256::digest(public_key)[..20]).into_string())
}

fn session_key(token: &str) -> String {
    to_hex(&Sha256::digest(token.as_bytes()))
}

fn random_bytes<const N: usize>() -> Result<[u8; N], GhostIdError> {
    let mut buf = [0u8; N];
    SystemRandom::new()
        .fill(&mut buf)
        .map_err(|_| GhostIdError::internal("system random source unavailable"))?;
    Ok(buf)
}

fn random_token() -> Result<String, GhostIdError> {
    Ok(to_hex(&random_bytes::<32>()?))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::{Path, State},
        routing::{get, post},
        Json, Router,
    };
    use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey};
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    #[derive(Clone, Default)]
    struct MockVerifierState {
        challenges: Arc<Mutex<HashMap<String, Value>>>,
        polls: Arc<Mutex<HashMap<String, (StatusCode, Value)>>>,
        provider_pk: Arc<Mutex<String>>,
    }

    async fn mock_post_challenge(
        State(state): State<MockVerifierState>,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        let expected_pk = state.provider_pk.lock().await.clone();
        if body.get("providerPublicKey").and_then(Value::as_str) != Some(expected_pk.as_str()) {
            return (StatusCode::FORBIDDEN, Json(json!({"error": "provider key mismatch"})));
        }
        let challenge = SignedChallenge {
            challenge_id: body["challengeId"].as_str().unwrap_or_default().to_string(),
            domain: body["domain"].as_str().unwrap_or_default().to_string(),
            action: body["action"].as_str().unwrap_or_default().to_string(),
            nonce: body["nonce"].as_str().unwrap_or_default().to_string(),
            issued_at: body["issuedAt"].as_i64().unwrap_or_default(),
            expires_at: body["expiresAt"].as_i64().unwrap_or_default(),
            permissions: body["permissions"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            duration_days: body["duration_days"].as_u64().unwrap_or_default() as u32,
            tier: body["tier"].as_str().unwrap_or_default().to_string(),
        };
        let pk_bytes = base64::engine::general_purpose::STANDARD.decode(&expected_pk).unwrap();
        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(body["providerSignature"].as_str().unwrap_or_default())
            .unwrap_or_default();
        if UnparsedPublicKey::new(&ring::signature::ED25519, &pk_bytes)
            .verify(ghostid_provider::provider_message(&challenge).as_bytes(), &sig_bytes)
            .is_err()
        {
            return (StatusCode::FORBIDDEN, Json(json!({"error": "invalid signature"})));
        }

        let mut issued = body.clone();
        issued["protocolVersion"] = json!(1);
        issued["used"] = json!(false);
        state
            .challenges
            .lock()
            .await
            .insert(challenge.challenge_id.clone(), issued.clone());
        (StatusCode::OK, Json(issued))
    }

    async fn mock_get_poll(
        State(state): State<MockVerifierState>,
        Path(id): Path<String>,
    ) -> (StatusCode, Json<Value>) {
        let polls = state.polls.lock().await;
        match polls.get(&id) {
            Some((status, body)) => (*status, Json(body.clone())),
            None => (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))),
        }
    }

    async fn spawn_mock_verifier(provider_pk_b64: String) -> (String, MockVerifierState, tokio::task::JoinHandle<()>) {
        let state = MockVerifierState {
            provider_pk: Arc::new(Mutex::new(provider_pk_b64)),
            ..Default::default()
        };
        let app = Router::new()
            .route("/auth/challenge", post(mock_post_challenge))
            .route("/auth/poll/:id", get(mock_get_poll))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{}", addr), state, handle)
    }

    fn make_keypair() -> Arc<Ed25519KeyPair> {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Arc::new(Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap())
    }

    fn verified_poll_payload(domain: &str, wallet_key: &Ed25519KeyPair, token_id: &str) -> Value {
        let pk_bytes = wallet_key.public_key().as_ref();
        let pk_b64 = base64::engine::general_purpose::STANDARD.encode(pk_bytes);
        let addr = wallet_address(pk_bytes);
        let expires_at = (Utc::now() + chrono::Duration::days(10)).to_rfc3339();
        json!({
            "status": "verified",
            "domain": domain,
            "address": addr,
            "publicKey": pk_b64,
            "token_id": token_id,
            "expires_at": expires_at,
            "permissions": ["editor"],
        })
    }

    #[tokio::test]
    async fn full_ghostid_login_flow_and_edge_cases() {
        let provider_key = make_keypair();
        let provider_pk_b64 = base64::engine::general_purpose::STANDARD.encode(provider_key.public_key().as_ref());
        let (verifier_url, mock_state, server_handle) = spawn_mock_verifier(provider_pk_b64).await;

        let db_file = std::env::temp_dir().join(format!("ghostid-test-{}.db", Uuid::new_v4()));
        let config = GhostIdConfig {
            domain: "mothership.rest".to_string(),
            verifier_url: verifier_url.clone(),
            require_token: true,
            db_path: db_file.to_string_lossy().to_string(),
        };
        let mut service = GhostIdService::connect(config.clone()).await.unwrap();
        service.test_key = Some(provider_key.clone());

        // 1. Start login -> gets challengeId, qrUrl, and new session cookie
        let started = service.start_login(None).await.expect("start login");
        let browser_cookie = started.new_session_token.expect("new session cookie");
        assert_eq!(started.qr_url, format!("{}/auth/qr/{}", verifier_url, started.challenge_id));

        // 2. Poll before wallet responds (404 from verifier) -> Pending
        assert!(matches!(
            service.poll_login(Some(&browser_cookie), &started.challenge_id).await.unwrap(),
            PollOutcome::Pending
        ));

        // 3. Poll while pending (200 {"status":"pending"}) -> Pending
        mock_state.polls.lock().await.insert(
            started.challenge_id.clone(),
            (StatusCode::OK, json!({"status": "pending"})),
        );
        assert!(matches!(
            service.poll_login(Some(&browser_cookie), &started.challenge_id).await.unwrap(),
            PollOutcome::Pending
        ));

        // 4. Wrong-browser attempt is rejected with 404
        let other_started = service.start_login(None).await.expect("other browser start");
        let other_cookie = other_started.new_session_token.unwrap();
        let wrong_browser = service.poll_login(Some(&other_cookie), &started.challenge_id).await;
        assert_eq!(wrong_browser.err().unwrap().status, StatusCode::NOT_FOUND);

        // 5. Wrong domain in verifier response is rejected with 401 and does not consume challenge
        let wallet_key = make_keypair();
        mock_state.polls.lock().await.insert(
            started.challenge_id.clone(),
            (StatusCode::OK, verified_poll_payload("evil.example", &wallet_key, "tok-1")),
        );
        let bad_domain = service.poll_login(Some(&browser_cookie), &started.challenge_id).await;
        assert_eq!(bad_domain.err().unwrap().status, StatusCode::UNAUTHORIZED);

        // 6. Valid verified response succeeds, rotates session cookie, and maps wallet to user
        mock_state.polls.lock().await.insert(
            started.challenge_id.clone(),
            (StatusCode::OK, verified_poll_payload("mothership.rest", &wallet_key, "tok-1")),
        );
        let PollOutcome::Verified { session_token: rotated_cookie, user } = service
            .poll_login(Some(&browser_cookie), &started.challenge_id)
            .await
            .expect("verified poll")
        else {
            panic!("expected verified outcome");
        };
        assert_ne!(rotated_cookie, browser_cookie);
        assert_eq!(user.domain, "mothership.rest");
        assert_eq!(user.address, wallet_address(wallet_key.public_key().as_ref()));

        // Old pre-login session cookie is invalidated by rotation
        assert_eq!(
            service.current_user(Some(&browser_cookie)).await.err().unwrap().status,
            StatusCode::UNAUTHORIZED
        );

        // 7. Reusing the same challenge is rejected with 410
        let reused = service.poll_login(Some(&rotated_cookie), &started.challenge_id).await;
        assert_eq!(reused.err().unwrap().status, StatusCode::GONE);

        // 8. Protected request (`current_user`) re-checks with verifier and succeeds
        let me = service.current_user(Some(&rotated_cookie)).await.expect("current user");
        assert_eq!(me.user_id, user.user_id);

        // 9. Restart persistence: reconnect to the same SQLite file and verify session still works
        let mut restarted = GhostIdService::connect(config.clone()).await.unwrap();
        restarted.test_key = Some(provider_key.clone());
        let me_after_restart = restarted.current_user(Some(&rotated_cookie)).await.expect("after restart");
        assert_eq!(me_after_restart.user_id, user.user_id);

        // 10. Expired QR challenge is rejected with 410
        let exp_started = restarted.start_login(None).await.unwrap();
        let exp_cookie = exp_started.new_session_token.unwrap();
        sqlx::query("UPDATE ghostid_attempts SET expires_at = ? WHERE challenge_id = ?")
            .bind(Utc::now().timestamp() - 10)
            .bind(&exp_started.challenge_id)
            .execute(&restarted.pool)
            .await
            .unwrap();
        let expired_res = restarted.poll_login(Some(&exp_cookie), &exp_started.challenge_id).await;
        assert_eq!(expired_res.err().unwrap().status, StatusCode::GONE);

        // 11. Logout deletes only the local session
        restarted.logout(Some(&rotated_cookie)).await.expect("logout");
        assert_eq!(
            restarted.current_user(Some(&rotated_cookie)).await.err().unwrap().status,
            StatusCode::UNAUTHORIZED
        );

        // 12. If verifier goes down, protected request fails closed with 503 (never cached success)
        let started2 = restarted.start_login(None).await.unwrap();
        let cookie2 = started2.new_session_token.unwrap();
        mock_state.polls.lock().await.insert(
            started2.challenge_id.clone(),
            (StatusCode::OK, verified_poll_payload("mothership.rest", &wallet_key, "tok-1")),
        );
        let PollOutcome::Verified { session_token: rotated2, .. } = restarted
            .poll_login(Some(&cookie2), &started2.challenge_id)
            .await
            .unwrap()
        else {
            panic!("expected verified");
        };
        // 12. If verifier goes down, protected request fails closed with 503 (never cached success)
        let mut offline_cfg = config.clone();
        offline_cfg.verifier_url = "http://127.0.0.1:1".to_string();
        let offline_service = GhostIdService::connect(offline_cfg).await.unwrap();
        let outage_res = offline_service.current_user(Some(&rotated2)).await;
        assert_eq!(outage_res.err().unwrap().status, StatusCode::SERVICE_UNAVAILABLE);

        // 13. When token is revoked at verifier, protected request returns 401 and clears local session
        mock_state.polls.lock().await.insert(
            started2.challenge_id.clone(),
            (StatusCode::OK, json!({"status": "failed", "error": "token_revoked_or_invalid"})),
        );
        let revoked_res = restarted.current_user(Some(&rotated2)).await;
        assert_eq!(revoked_res.err().unwrap().status, StatusCode::UNAUTHORIZED);
        server_handle.abort();

        drop(service);
        drop(restarted);
        let _ = std::fs::remove_file(db_file);
    }
}
