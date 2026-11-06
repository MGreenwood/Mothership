//! Browser-assisted CLI login. The CLI opens a browser and the user signs in (GhostID or
//! Google). The server then sends a one-time code to a loopback listener in the CLI, and the
//! CLI exchanges that code for a token. The token never appears in a URL or on a page.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json, Redirect, Response},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::Utc;
use mothership_common::auth::Claims;
use mothership_common::UserRole;
use ring::rand::{SecureRandom, SystemRandom};
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;

const PENDING_TTL: Duration = Duration::from_secs(300);
const CODE_TTL: Duration = Duration::from_secs(60);
const CLI_COOKIE: &str = "mothership_cli";
const CLI_TOKEN_SECS: i64 = 30 * 24 * 3600;

struct PendingLogin {
    port: u16,
    created: Instant,
}

struct CodeEntry {
    state: String,
    user_id: Uuid,
    username: String,
    email: String,
    admin: bool,
    machine_id: &'static str,
    ttl_secs: i64,
    created: Instant,
}

struct Identity {
    user_id: Uuid,
    /// Wallet address or OAuth username, used for whitelist checks.
    username: String,
    /// Name shown on the token: the whitelist nickname when set, otherwise `username`.
    display_name: String,
    email: String,
    admin: bool,
    machine_id: &'static str,
    ttl_secs: i64,
}

lazy_static::lazy_static! {
    static ref PENDING: Mutex<HashMap<String, PendingLogin>> = Mutex::new(HashMap::new());
    static ref CODES: Mutex<HashMap<String, CodeEntry>> = Mutex::new(HashMap::new());
}

fn web_base_url() -> String {
    std::env::var("WEB_UI_BASE_URL").unwrap_or_else(|_| "https://app.mothership.rest".to_string())
}

fn valid_state(value: &str) -> bool {
    (32..=128).contains(&value.len()) && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn random_hex() -> Result<String, StatusCode> {
    let mut buf = [0u8; 32];
    SystemRandom::new().fill(&mut buf).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

fn error_page(status: StatusCode, message: &str) -> Response {
    (status, Html(format!("<!DOCTYPE html><html><body><h1>Mothership CLI login</h1><p>{message}</p></body></html>"))).into_response()
}

/// POST /auth/cli/start (API). The CLI registers its loopback port and a random state.
#[derive(Deserialize)]
pub struct CliStartRequest {
    pub callback_port: u16,
    pub state: String,
}

pub async fn api_start(Json(req): Json<CliStartRequest>) -> Result<Json<serde_json::Value>, StatusCode> {
    if req.callback_port < 1024 || !valid_state(&req.state) {
        return Err(StatusCode::BAD_REQUEST);
    }
    {
        let mut pending = PENDING.lock().unwrap();
        pending.retain(|_, p| p.created.elapsed() < PENDING_TTL);
        pending.insert(req.state.clone(), PendingLogin { port: req.callback_port, created: Instant::now() });
    }
    Ok(Json(json!({
        "auth_url": format!("{}/cli/auth/start?state={}", web_base_url(), req.state),
        "expires_in": PENDING_TTL.as_secs(),
    })))
}

/// POST /auth/cli/exchange (API). Trades a one-time code for a token. Codes are single-use.
#[derive(Deserialize)]
pub struct CliExchangeRequest {
    pub code: String,
    pub state: String,
}

pub async fn api_exchange(
    State(state): State<AppState>,
    Json(req): Json<CliExchangeRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let entry = CODES.lock().unwrap().remove(&req.code).ok_or(StatusCode::UNAUTHORIZED)?;
    if entry.created.elapsed() > CODE_TTL || entry.state != req.state {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let now = Utc::now();
    let exp = (now + chrono::Duration::seconds(entry.ttl_secs)).timestamp();
    let claims = Claims {
        sub: entry.user_id.to_string(),
        machine_id: entry.machine_id.to_string(),
        username: entry.username.clone(),
        email: Some(entry.email.clone()),
        iat: now.timestamp(),
        exp,
        aud: "mothership".to_string(),
        iss: "mothership-server".to_string(),
    };
    let token = state.auth.encode_token(&claims).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    info!("CLI token issued for {} ({})", entry.username, entry.machine_id);
    let mut success_url = url::Url::parse(&format!("{}/auth/success", web_base_url()))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    success_url
        .query_pairs_mut()
        .append_pair("user", &entry.username)
        .append_pair("email", &entry.email);
    Ok(Json(json!({
        "token": token,
        "username": entry.username,
        "email": entry.email,
        "admin": entry.admin,
        "expires_at": exp,
        "success_url": success_url.as_str(),
    })))
}

/// The signed-in browser user, from a live GhostID session (re-verified) or an OAuth session.
async fn identify(state: &AppState, jar: &CookieJar) -> Option<Identity> {
    if let (Some(service), Some(token)) = (state.ghostid.as_ref(), crate::web_ui::ghostid_cookie_value(jar)) {
        // A stale or revoked GhostID cookie must not block a valid OAuth session, so fall through on failure.
        if let Ok(user) = service.current_user(Some(&token)).await {
            let email = format!("{}@ghostid.wallet", user.address);
            if let Ok(user_id) = ensure_user(state, &user.address, &email).await {
                let grant = whitelist_grant(state, &user.address, &email);
                return Some(Identity {
                    user_id,
                    display_name: grant.nickname.clone().unwrap_or_else(|| user.address.clone()),
                    username: user.address,
                    email,
                    admin: grant.admin,
                    machine_id: "ghostid-cli",
                    ttl_secs: CLI_TOKEN_SECS,
                });
            }
        }
    }
    let session_id = jar.get("mothership_session")?.value().to_string();
    let session = state.sessions.read().await.get(&session_id).cloned()?;
    if Utc::now() > session.expires_at {
        return None;
    }
    let grant = whitelist_grant(state, &session.username, &session.email);
    Some(Identity {
        user_id: session.user_id,
        display_name: grant.nickname.clone().unwrap_or_else(|| session.username.clone()),
        username: session.username,
        email: session.email,
        admin: grant.admin,
        machine_id: "oauth-cli",
        ttl_secs: CLI_TOKEN_SECS,
    })
}

fn whitelist_grant(state: &AppState, username: &str, email: &str) -> crate::config::Grant {
    state
        .whitelist
        .as_ref()
        .map(|whitelist| whitelist.grant_for(username, email))
        .unwrap_or_default()
}

/// Returns the local user for a wallet, creating the record on first login.
async fn ensure_user(state: &AppState, address: &str, email: &str) -> Result<Uuid, StatusCode> {
    match state.db.get_user_by_username(address).await {
        Ok(Some(user)) => Ok(user.id),
        Ok(None) => state
            .db
            .create_user_with_id(Uuid::new_v4(), address.to_string(), email.to_string(), UserRole::User)
            .await
            .map(|user| user.id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// GET /cli/auth/start (web). Binds the browser to a pending CLI login, then signs in or skips ahead.
#[derive(Deserialize)]
pub struct BrowserStartQuery {
    pub state: String,
    pub provider: Option<String>,
}

pub async fn browser_start(
    State(state): State<AppState>,
    Query(query): Query<BrowserStartQuery>,
    jar: CookieJar,
) -> Response {
    let known = valid_state(&query.state)
        && PENDING
            .lock()
            .unwrap()
            .get(&query.state)
            .is_some_and(|p| p.created.elapsed() < PENDING_TTL);
    if !known {
        return error_page(StatusCode::BAD_REQUEST, "This CLI login link has expired. Run <code>mothership auth</code> again.");
    }

    let secure = crate::web_ui::ghostid_is_secure();
    let jar = jar.add(
        Cookie::build((CLI_COOKIE, query.state.clone()))
            .http_only(true)
            .secure(secure)
            .same_site(SameSite::Lax)
            .path("/")
            .build(),
    );

    if query.provider.is_none() && identify(&state, &jar).await.is_some() {
        return (jar, Redirect::to("/cli/auth/complete")).into_response();
    }
    let target = match query.provider.as_deref() {
        Some("ghostid") => "/auth/ghostid",
        Some("google") => "/auth/start",
        _ => "/login",
    };
    (jar, Redirect::to(target)).into_response()
}

/// GET /cli/auth/complete (web). Issues a one-time code and sends the browser to the CLI's loopback port.
pub async fn browser_complete(State(state): State<AppState>, jar: CookieJar) -> Response {
    let Some(cli_state) = jar.get(CLI_COOKIE).map(|c| c.value().to_string()) else {
        return Redirect::to("/download/authenticated").into_response();
    };
    let port = match PENDING.lock().unwrap().get(&cli_state) {
        Some(pending) if pending.created.elapsed() < PENDING_TTL => pending.port,
        _ => return error_page(StatusCode::BAD_REQUEST, "This CLI login has expired. Run <code>mothership auth</code> again."),
    };

    let Some(identity) = identify(&state, &jar).await else {
        warn!("CLI login: no signed-in browser session found; sending user to sign-in");
        return Redirect::to("/login").into_response();
    };
    if let Some(whitelist) = &state.whitelist {
        if !whitelist.is_user_allowed(&identity.username, &identity.email) {
            warn!("CLI login denied for {}: not in whitelist", identity.username);
            return error_page(StatusCode::FORBIDDEN, "This account is not allowed to use the Mothership CLI.");
        }
    }

    PENDING.lock().unwrap().remove(&cli_state);
    let code = match random_hex() {
        Ok(code) => code,
        Err(status) => return error_page(status, "Could not start the CLI login."),
    };
    {
        let mut codes = CODES.lock().unwrap();
        codes.retain(|_, c| c.created.elapsed() < CODE_TTL);
        codes.insert(
            code.clone(),
            CodeEntry {
                state: cli_state.clone(),
                user_id: identity.user_id,
                username: identity.display_name,
                email: identity.email,
                admin: identity.admin,
                machine_id: identity.machine_id,
                ttl_secs: identity.ttl_secs,
                created: Instant::now(),
            },
        );
    }

    let jar = jar.remove(Cookie::build((CLI_COOKIE, "")).path("/"));
    let target = format!("http://127.0.0.1:{port}/callback?code={code}&state={cli_state}");
    (jar, Redirect::to(&target)).into_response()
}
