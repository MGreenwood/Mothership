use axum::{
    extract::{State, Json, Path, Query},
    http::StatusCode,
    response::{Html, Response, IntoResponse, Redirect},
    routing::{get, post},
    Router,
};
use mothership_common::auth::{OAuthProvider, OAuthSource};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use tracing::{info, warn, error};
use tower_http::services::ServeDir;
use axum_extra::extract::cookie::Cookie;
use url;
use std::fs;
use uuid;

/// Web UI routes for authentication and CLI downloads
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/", get(index_page))
    .route("/login", get(login_page))
    .route("/auth/start", get(auth_start))
    .route("/auth/ghostid", get(ghostid_auth_page))
    .route("/auth/ghostid/challenge", post(ghostid_start_challenge))
    .route("/auth/ghostid/poll/:challenge_id", get(ghostid_poll))
    .route("/auth/ghostid/me", get(ghostid_me))
    .route("/auth/ghostid/logout", post(ghostid_logout))
        .route("/download", get(download_page))
        .route("/download/authenticated", get(authenticated_download_page))
        .route("/download/cli/:version/:platform/:binary", get(web_download_cli))
        .route("/cli/auth/start", get(crate::cli_auth::browser_start))
        .route("/cli/auth/complete", get(crate::cli_auth::browser_complete))
        .route("/auth/callback", post(auth_callback))
        .route("/auth/finalize", get(auth_finalize))
        .route("/auth/success", get(auth_success_page))
        .route("/auth/error", get(auth_error_page))
        .route("/robots.txt", get(robots_txt))
        // Serve static files (icon.png, etc.)
        .nest_service("/static", ServeDir::new("content"))
}

/// Serve robots.txt
async fn robots_txt() -> Result<Response<String>, StatusCode> {
    match std::fs::read_to_string("content/robots.txt") {
        Ok(content) => Ok(Response::builder()
            .header("content-type", "text/plain")
            .body(content)
            .unwrap()),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

/// Landing page after a successful sign-in.
async fn auth_success_page(Query(params): Query<std::collections::HashMap<String, String>>) -> Response {
    let user = params.get("user").map(String::as_str).unwrap_or("");
    let email = params.get("email").map(String::as_str).unwrap_or("");
    render_auth_page("auth-success.html", &[("USER", user), ("EMAIL", email)])
}

/// Landing page for a failed sign-in. Explains access denials and shows the reason.
async fn auth_error_page(Query(params): Query<std::collections::HashMap<String, String>>) -> Response {
    let message = params.get("message").map(String::as_str).unwrap_or("Unknown error");
    let (heading, detail) = if message.contains("not authorized") {
        (
            "Access not granted",
            "Your sign-in worked, but this account isn't on the server's access list. Ask a server administrator to add your GhostID address or email, then try again.",
        )
    } else {
        (
            "Sign-in failed",
            "Something went wrong while signing you in. Go back and try again.",
        )
    };
    render_auth_page("auth-error.html", &[("HEADING", heading), ("DETAIL", detail), ("MESSAGE", message)])
}

/// Renders a `content/` template for an auth result page, HTML-escaping every value.
fn render_auth_page(template: &str, replacements: &[(&str, &str)]) -> Response {
    let escaped: Vec<(&str, String)> = replacements
        .iter()
        .map(|(key, value)| (*key, html_escape(value)))
        .collect();
    let borrowed: Vec<(&str, &str)> = escaped.iter().map(|(key, value)| (*key, value.as_str())).collect();
    match load_template(template, &borrowed) {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            error!("Failed to load {} template: {}", template, e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

#[derive(Deserialize)]
struct DownloadPageQuery {
    token: Option<String>,
    user: Option<String>,
    email: Option<String>,
}

/// Load and process an HTML template with placeholder replacement
fn load_template(template_name: &str, replacements: &[(&str, &str)]) -> Result<String, std::io::Error> {
    let template_path = format!("content/{}", template_name);
    let mut content = fs::read_to_string(&template_path)?;
    
    for (placeholder, replacement) in replacements {
        content = content.replace(&format!("{{{{{}}}}}", placeholder), replacement);
    }
    
    Ok(content)
}

/// Main index page
async fn index_page(State(state): State<crate::AppState>) -> Result<Html<String>, StatusCode> {
    let auth_required = state.config.cli_distribution.require_auth_for_downloads || state.whitelist.is_some();
    
    let auth_status = if auth_required {
        "Authentication Required - CLI downloads require authentication through your authorized account."
    } else {
        "Public Access - CLI downloads are publicly available. Authentication is required for server usage."
    };
    
    let download_button = if auth_required {
        r#"<a href="/login" class="btn btn-primary">Sign In to Download CLI</a>"#
    } else {
        r#"<a href="/download" class="btn btn-primary">Download CLI</a>"#
    };
    
    let replacements = [
        ("AUTH_STATUS", auth_status),
        ("DOWNLOAD_BUTTON", download_button),
    ];
    
    match load_template("index.html", &replacements) {
        Ok(html) => Ok(Html(html)),
        Err(e) => {
            error!("Failed to load index template: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Start OAuth authentication from the web UI
async fn auth_start(
    State(state): State<crate::AppState>,
    jar: CookieJar,
) -> Result<(CookieJar, Redirect), StatusCode> {
    let web_ui_url = std::env::var("WEB_UI_BASE_URL")
        .unwrap_or_else(|_| "https://app.mothership.rest".to_string());

    let callback_url = format!("{}/auth/finalize", web_ui_url);

    let (auth_url, _csrf_state) = state.oauth
        .get_authorization_url(
            OAuthProvider::Google,
            OAuthSource::Web,
            Some(callback_url),
        )
        .await
        .map_err(|e| {
            error!("Failed to start web OAuth: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // A Google login must not be overridden by an older GhostID session (see `identify`).
    Ok((clear_ghostid_cookies(jar), Redirect::temporary(&auth_url)))
}

/// Login page that starts OAuth or GhostID flow
async fn login_page(State(state): State<crate::AppState>) -> Result<Html<String>, StatusCode> {
    if !state.config.features.oauth_enabled && state.ghostid.is_none() {
        let replacements = [
            ("PAGE_TITLE", "Authentication Disabled"),
            ("PAGE_MESSAGE", "OAuth authentication is not configured on this server"),
            ("AUTH_CONTENT", r#"<div style="background: rgba(255, 255, 255, 0.1); padding: 2rem; border-radius: 10px; margin: 2rem 0;">
                <h3>Configuration Required</h3>
                <p>The server administrator needs to configure OAuth settings to enable authentication features.</p>
            </div>"#),
        ];
        
        match load_template("login.html", &replacements) {
            Ok(html) => return Ok(Html(html)),
            Err(e) => {
                error!("Failed to load login template: {}", e);
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    }
    
    let replacements = [
        ("PAGE_TITLE", "Sign In"),
        ("PAGE_MESSAGE", "Choose your preferred authentication method"),
        ("AUTH_CONTENT", r#"<a href="/auth/start" class="auth-btn">Sign in with Google</a>
        <a href="/auth/ghostid" class="auth-btn" style="background: linear-gradient(45deg, #4CAF50, #45a049);">� Sign in with GhostID Wallet</a>"#),
    ];
    
    match load_template("login.html", &replacements) {
        Ok(html) => Ok(Html(html)),
        Err(e) => {
            error!("Failed to load login template: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Public download page (when auth not required)
async fn download_page(State(state): State<crate::AppState>) -> Html<String> {
    let auth_required = state.config.cli_distribution.require_auth_for_downloads || state.whitelist.is_some();
    
    if auth_required {
        match load_template("auth-required.html", &[]) {
            Ok(html) => return Html(html),
            Err(e) => {
                error!("Failed to load auth-required template: {}", e);
                return Html("<!DOCTYPE html><html><head><title>Auth Required</title></head><body><h1>Authentication Required</h1><p><a href='/login'>Please sign in</a></p></body></html>".to_string());
            }
        }
    }

    // Public download page
    generate_download_page_html(None, None, &state).await
}

/// Authenticated download page (after successful OAuth or GhostID login)
async fn authenticated_download_page(
    jar: CookieJar,
    State(state): State<crate::AppState>,
) -> Result<Html<String>, StatusCode> {
    if let (Some(service), Some(ghostid_token)) = (state.ghostid.as_ref(), ghostid_cookie_value(&jar)) {
        match service.current_user(Some(&ghostid_token)).await {
            Ok(ghost_user) => {
                let email = format!("{}@ghostid.wallet", ghost_user.address);
                let user_uuid = uuid::Uuid::parse_str(&ghost_user.user_id)
                    .unwrap_or_else(|_| uuid::Uuid::new_v4());
                let _local_user_id = match state.db.get_user_by_username(&ghost_user.address).await {
                    Ok(Some(existing)) => existing.id,
                    Ok(None) => state
                        .db
                        .create_user_with_id(
                            user_uuid,
                            ghost_user.address.clone(),
                            email.clone(),
                            mothership_common::UserRole::User,
                        )
                        .await
                        .map(|u| u.id)
                        .unwrap_or(user_uuid),
                    Err(_) => user_uuid,
                };
                // The bearer token is never shown on the GhostID page; CLI access needs a separate flow.
                return Ok(generate_download_page_html(
                    Some(ghost_user.address),
                    Some(email),
                    &state,
                )
                .await);
            }
            Err(err) => return Err(err.status),
        }
    }

    // Get session from secure cookie
    let session_id = match jar.get("mothership_session") {
        Some(cookie) => {
            info!("Session cookie found");
            cookie.value().to_string()
        }
        None => {
            warn!("Authenticated download page accessed without session cookie");
            return Ok(Html(format!(r#"
<!DOCTYPE html>
<html>
<head><title>Session Expired</title></head>
<body>
    <h1>Session Expired</h1>
    <p>Your session has expired. Please <a href="/login">sign in again</a>.</p>
</body>
</html>
            "#)));
        }
    };
    
    // Retrieve session data
    let session_data = {
        let sessions = state.sessions.read().await;
        let session_count = sessions.len();
        info!("Total active sessions: {}", session_count);
        sessions.get(&session_id).cloned()
    };
    
    let session_data = match session_data {
        Some(data) => {
            let now = chrono::Utc::now();
            info!("Session found - expires at: {}, current time: {}", data.expires_at, now);
            
            // Check if session is expired
            if now > data.expires_at {
                warn!("Expired session used for download page: {} (expired at {}, current: {})", session_id, data.expires_at, now);
                // Clean up expired session
                {
                    let mut sessions = state.sessions.write().await;
                    sessions.remove(&session_id);
                }
                return Ok(Html(format!(r#"
<!DOCTYPE html>
<html>
<head><title>Session Expired</title></head>
<body>
    <h1>Session Expired</h1>
    <p>Your session has expired. Please <a href="/login">sign in again</a>.</p>
</body>
</html>
                "#)));
            }
            data
        }
        None => {
            warn!("Invalid session ID used for download page: {}", session_id);
            return Ok(Html(format!(r#"
<!DOCTYPE html>
<html>
<head><title>Invalid Session</title></head>
<body>
    <h1>Invalid Session</h1>
    <p>Your session is invalid. Please <a href="/login">sign in again</a>.</p>
</body>
</html>
            "#)));
        }
    };
    
    info!("Authenticated download page accessed by user: {} ({})", session_data.username, session_data.email);
    
    Ok(generate_download_page_html(
        Some(session_data.username), 
        Some(session_data.email), 
        &state
    ).await)
}

/// Generate the download page HTML. Only the Windows CLI is downloadable for now; other platforms are disabled.
async fn generate_download_page_html(
    username: Option<String>,
    email: Option<String>,
    _state: &crate::AppState,
) -> Html<String> {
    let is_authenticated = username.is_some();
    let latest_version = crate::cli_distribution::latest_cli_version().await;

    let user_block = if is_authenticated {
        format!(r#"<div class="user-info">
                <h3>Authenticated as {}</h3>
                <p>Email: {}</p>
                <p>You have access to download all CLI tools.</p>
                <button class="download-btn" type="button" onclick="fetch('/auth/ghostid/logout', {{method:'POST', credentials:'same-origin'}}).finally(function(){{ window.location.replace('/login'); }})">Sign out</button>
            </div>"#,
            username.as_deref().unwrap_or("Unknown"),
            email.as_deref().unwrap_or("Unknown"))
    } else {
        String::new()
    };

    let disabled_buttons = |label: &str| {
        format!(r#"<span class="download-btn disabled" aria-disabled="true" title="{label}">CLI</span>
                    <span class="download-btn disabled" aria-disabled="true" title="{label}">Daemon</span>"#)
    };
    let windows_buttons = match (&latest_version, is_authenticated) {
        (Some(v), true) => format!(
            r#"<a href="/download/cli/{v}/x86_64-pc-windows-msvc/mothership.exe" class="download-btn">CLI</a>
                    <a href="/download/cli/{v}/x86_64-pc-windows-msvc/mothership-daemon.exe" class="download-btn">Daemon</a>"#
        ),
        (Some(_), false) => disabled_buttons("Sign in to download"),
        (None, _) => disabled_buttons("No Windows build is available"),
    };
    let platform_cards = format!(
        r#"<div class="platform">
                    <h4>Windows x64</h4>
                    {windows_buttons}
                </div>
                <div class="platform disabled">
                    <h4>Linux x64</h4>
                    {linux_x64}
                </div>
                <div class="platform disabled">
                    <h4>Linux ARM64</h4>
                    {linux_arm}
                </div>
                <div class="platform disabled">
                    <h4>macOS x64</h4>
                    {mac_x64}
                </div>
                <div class="platform disabled">
                    <h4>macOS ARM64</h4>
                    {mac_arm}
                </div>"#,
        linux_x64 = disabled_buttons("Coming soon"),
        linux_arm = disabled_buttons("Coming soon"),
        mac_x64 = disabled_buttons("Coming soon"),
        mac_arm = disabled_buttons("Coming soon"),
    );
    
    let html = format!(r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Download CLI - Mothership</title>
    <link rel="icon" type="image/png" href="/static/icon.png">
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            line-height: 1.6;
            margin: 0;
            padding: 2rem;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            color: white;
        }}
        
        .container {{
            max-width: 900px;
            margin: 0 auto;
            background: rgba(255, 255, 255, 0.1);
            padding: 3rem;
            border-radius: 20px;
            backdrop-filter: blur(10px);
        }}
        
        h1 {{
            font-size: 2.5rem;
            text-align: center;
            margin-bottom: 1rem;
        }}
        
        .user-info {{
            background: rgba(72, 187, 120, 0.2);
            border: 2px solid rgba(72, 187, 120, 0.5);
            padding: 1rem;
            border-radius: 10px;
            margin: 2rem 0;
            text-align: center;
        }}
        
        .download-methods {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
            gap: 2rem;
            margin: 3rem 0;
        }}
        
        .method {{
            background: rgba(255, 255, 255, 0.1);
            padding: 2rem;
            border-radius: 15px;
        }}
        
        .method h3 {{
            margin-bottom: 1rem;
            color: #48bb78;
        }}
        
        .code-block {{
            background: rgba(0, 0, 0, 0.4);
            padding: 1rem;
            border-radius: 10px;
            font-family: 'Monaco', 'Courier New', monospace;
            font-size: 0.9rem;
            overflow-x: auto;
            margin: 1rem 0;
            position: relative;
        }}
        
        .copy-btn {{
            position: absolute;
            top: 10px;
            right: 10px;
            background: rgba(72, 187, 120, 0.8);
            border: none;
            color: white;
            padding: 0.5rem;
            border-radius: 5px;
            cursor: pointer;
            font-size: 0.8rem;
        }}
        
        .copy-btn:hover {{
            background: rgba(72, 187, 120, 1);
        }}
        
        .platform-downloads {{
            margin: 3rem 0;
        }}
        
        .platforms {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 1rem;
            margin: 2rem 0;
        }}
        
        .platform {{
            background: rgba(255, 255, 255, 0.1);
            padding: 1.5rem;
            border-radius: 10px;
            text-align: center;
        }}
        
        .download-btn {{
            display: inline-block;
            padding: 0.8rem 1.5rem;
            background: rgba(72, 187, 120, 0.8);
            color: white;
            text-decoration: none;
            border-radius: 8px;
            font-weight: bold;
            margin: 0.5rem;
            transition: all 0.3s ease;
        }}
        
        .download-btn:hover {{
            background: rgba(72, 187, 120, 1);
            transform: translateY(-2px);
        }}
        
        .warning {{
            background: rgba(245, 101, 101, 0.2);
            border: 2px solid rgba(245, 101, 101, 0.5);
            padding: 1rem;
            border-radius: 10px;
            margin: 2rem 0;
        }}
        
        .platform.disabled {{
            opacity: 0.4;
        }}
        
        .download-btn.disabled {{
            background: #777;
            cursor: not-allowed;
            pointer-events: none;
        }}
        
        .note {{
            background: rgba(66, 153, 225, 0.2);
            border: 2px solid rgba(66, 153, 225, 0.5);
            padding: 1rem;
            border-radius: 10px;
            margin: 2rem 0;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>Download Mothership CLI</h1>
        
        {user_block}
        
        <div class="platform-downloads">
            <h2>Direct Downloads</h2>
            <p>Windows is the only platform available right now. Other platforms are coming soon.</p>
            
            <div class="platforms">
                {platform_cards}
            </div>
        </div>
        
        <div class="note">
            <h3>Next Steps</h3>
            <ol>
                <li>Download and install the CLI</li>
                <li>Run <code>mothership auth</code> to authenticate with this server</li>
                <li>Use <code>mothership --help</code> to see all available commands</li>
                <li>Run <code>mothership update</code> to check for updates</li>
            </ol>
        </div>
    </div>
</body>
</html>
"#,
        user_block = user_block,
        platform_cards = platform_cards,
    );

    Html(html)
}

/// Handle server-to-server authentication callback
async fn auth_callback(
    State(state): State<crate::AppState>,
    Json(callback_data): Json<mothership_common::auth::ServerAuthCallback>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    info!("Received auth callback for user: {} ({})", callback_data.username, callback_data.email);
    
    // Validate the token with the API server
    let client = reqwest::Client::new();
    let api_server_url = std::env::var("MOTHERSHIP_API_URL")
        .unwrap_or_else(|_| "https://api.mothership.rest".to_string());
    
    let validation_response = client
        .get(&format!("{}/auth/check", api_server_url))
        .bearer_auth(&callback_data.token)
        .send()
        .await
        .map_err(|e| {
            error!("Failed to validate token with API server: {}", e);
            StatusCode::BAD_REQUEST
        })?;
    
    if !validation_response.status().is_success() {
        error!("Token validation failed: HTTP {}", validation_response.status());
        return Err(StatusCode::UNAUTHORIZED);
    }
    
    // Create local session
    let session_id = uuid::Uuid::new_v4().to_string();
    let session_data = crate::SessionData {
        user_id: callback_data.user_id,
        username: callback_data.username.clone(),
        email: callback_data.email.clone(),
        token: callback_data.token,
        created_at: chrono::Utc::now(),
        expires_at: callback_data.expires_at,
    };
    
    // Store session
    {
        let mut sessions = state.sessions.write().await;
        sessions.insert(session_id.clone(), session_data);
    }
    
    info!("Created local session for user: {} ({})", callback_data.username, callback_data.email);
    
    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Authentication successful"
    })))
}

/// Handle browser authentication finalization with temporary code
pub async fn auth_finalize(
    State(state): State<crate::AppState>,
    query: axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Response, StatusCode> {
    let _callback_url = query.get("callback_url")
        .map(|url| url.to_string())
        .unwrap_or_else(|| "https://app.mothership.rest".to_string());

    let code = query.get("code")
        .ok_or_else(|| {
            error!("Auth finalize missing 'code' parameter");
            StatusCode::BAD_REQUEST
        })?
        .clone();
    
    info!("Processing auth finalize");
    
    // Retrieve and validate temporary token
    let temp_token_data = {
        let mut temp_tokens = state.temp_tokens.write().await;
        temp_tokens.remove(&code)
    };
    
    let temp_token_data = match temp_token_data {
        Some(data) => {
            // Check if token is expired
            if chrono::Utc::now() > data.expires_at {
                error!("Temporary token expired: {}", code);
                return Ok(axum::response::Redirect::to("/auth/error?message=Authentication code expired. Please try again.").into_response());
            }
            data
        }
        None => {
            error!("Invalid or missing temporary token for code: {}", code);
            return Ok(axum::response::Redirect::to("/auth/error?message=Invalid authentication code. Please try again.").into_response());
        }
    };
    
    info!("Validated temporary token for user: {} ({})", temp_token_data.username, temp_token_data.email);
    
    // Create session
    let session_id = uuid::Uuid::new_v4().to_string();
    let token = temp_token_data.token.clone();
    let session_data = crate::SessionData {
        user_id: temp_token_data.user_id,
        username: temp_token_data.username.clone(),
        email: temp_token_data.email.clone(),
        token,
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(24),
    };
    
    // Store session
    {
        let mut sessions = state.sessions.write().await;
        sessions.insert(session_id.clone(), session_data);
    }
    
    info!("Created session for user: {} ({})", temp_token_data.username, temp_token_data.email);
    
    // Get the web UI URL
    let web_ui_url = std::env::var("WEB_UI_BASE_URL")
        .or_else(|_| std::env::var("OAUTH_BASE_URL"))
        .unwrap_or_else(|_| "http://localhost:7523".to_string());
    
    // Create session cookie - determine secure flag and domain
    let is_secure = web_ui_url.starts_with("https");
    let is_localhost = web_ui_url.contains("localhost") || web_ui_url.contains("127.0.0.1");
    
    let mut cookie_builder = Cookie::build(("mothership_session", session_id))
        .http_only(true)
        .secure(is_secure)
        .same_site(axum_extra::extract::cookie::SameSite::Lax)
        .path("/");
    
    // Set domain for non-localhost URLs
    if !is_localhost {
        // Extract base domain from web_ui_url
        if let Ok(url) = url::Url::parse(&web_ui_url) {
            if let Some(domain) = url.domain() {
                // Get the base domain (e.g., "mothership.rest" from "app.mothership.rest")
                let parts: Vec<&str> = domain.split('.').collect();
                if parts.len() >= 2 {
                    let base_domain = format!(".{}.{}", parts[parts.len() - 2], parts[parts.len() - 1]);
                    info!("Setting cookie domain to: {}", base_domain);
                    cookie_builder = cookie_builder.domain(base_domain);
                }
            }
        }
    }
    
    let cookie = cookie_builder.build();
    
    info!("Session cookie created - secure: {}, localhost: {}, domain: {:?}", 
          is_secure, is_localhost, cookie.domain());
    
    // Redirect to the download page; it reads the session from the cookie, so no
    // credentials are placed in the URL.
    let success_url = "/cli/auth/complete";

    Ok((
        clear_ghostid_cookies(CookieJar::new().add(cookie)),
        axum::response::Redirect::to(success_url)
    ).into_response())
}

/// Browser download of a CLI binary. Accepts a live GhostID session or an OAuth session cookie.
async fn web_download_cli(
    State(state): State<crate::AppState>,
    jar: CookieJar,
    Path((version, platform, binary)): Path<(String, String, String)>,
) -> Result<Response, StatusCode> {
    let (username, email) = authorized_browser_user(&state, &jar).await?;
    if let Some(whitelist) = &state.whitelist {
        if !whitelist.is_user_allowed(&username, &email) {
            warn!("Browser CLI download denied for {}", username);
            return Err(StatusCode::FORBIDDEN);
        }
    }
    crate::cli_distribution::serve_binary(&version, &platform, &binary).await
}

/// Resolves the signed-in user from the GhostID session (re-verified each call) or the OAuth session.
async fn authorized_browser_user(state: &crate::AppState, jar: &CookieJar) -> Result<(String, String), StatusCode> {
    if let (Some(service), Some(token)) = (state.ghostid.as_ref(), ghostid_cookie_value(jar)) {
        let user = service.current_user(Some(&token)).await.map_err(|e| e.status)?;
        let email = format!("{}@ghostid.wallet", user.address);
        return Ok((user.address, email));
    }
    let session_id = jar
        .get("mothership_session")
        .map(|c| c.value().to_string())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let session = state
        .sessions
        .read()
        .await
        .get(&session_id)
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if chrono::Utc::now() > session.expires_at {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok((session.username, session.email))
}

/// Session cookie for GhostID login. Uses `__Host-ghostid` over HTTPS and `ghostid` over local HTTP.
fn ghostid_cookie(token: String, secure: bool) -> Cookie<'static> {
    let name = if secure {
        crate::ghostid_login::SESSION_COOKIE
    } else {
        "ghostid"
    };
    Cookie::build((name, token))
        .http_only(true)
        .secure(secure)
        .same_site(axum_extra::extract::cookie::SameSite::Lax)
        .path("/")
        .build()
}

pub(crate) fn ghostid_is_secure() -> bool {
    std::env::var("PUBLIC_BASE_URL")
        .or_else(|_| std::env::var("WEB_UI_BASE_URL"))
        .or_else(|_| std::env::var("OAUTH_BASE_URL"))
        .map(|u| u.starts_with("https://"))
        .unwrap_or(false)
}

pub(crate) fn ghostid_cookie_value(jar: &CookieJar) -> Option<String> {
    jar.get(crate::ghostid_login::SESSION_COOKIE)
        .or_else(|| jar.get("ghostid"))
        .map(|c| c.value().to_string())
}

fn ghostid_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({"error": "GhostID login is not configured"})),
    )
        .into_response()
}

fn ghostid_error(err: crate::ghostid_login::GhostIdError) -> Response {
    (err.status, Json(serde_json::json!({"error": err.message}))).into_response()
}

const GHOSTID_LOGIN_PAGE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Sign in with GhostID</title>
<style>
body { font-family: sans-serif; background: #111; color: #eee; text-align: center; padding: 2rem; }
img { width: 260px; height: 260px; background: #fff; padding: 12px; border-radius: 8px; }
a { color: #8f8; }
</style>
</head>
<body>
<h1>Sign in with GhostID</h1>
<p>Scan this code with the GhostID wallet app and approve the login.</p>
<img id="qr" alt="GhostID login QR code" hidden>
<p id="status">Preparing login...</p>
<p><a href="/login">Back to sign-in options</a></p>
<script>
(async function () {
  const statusEl = document.getElementById('status');
  const qr = document.getElementById('qr');
  let timer = null;
  function fail(msg) { clearInterval(timer); statusEl.textContent = msg; }
  try {
    const res = await fetch('/auth/ghostid/challenge', { method: 'POST', credentials: 'same-origin' });
    if (!res.ok) { fail('Could not start GhostID login (' + res.status + '). Reload to try again.'); return; }
    const data = await res.json();
    qr.src = data.qrUrl;
    qr.hidden = false;
    statusEl.textContent = 'Waiting for approval in the wallet app...';
    const expiresAtMs = data.expiresAt * 1000;
    timer = setInterval(async function () {
      if (Date.now() >= expiresAtMs) { fail('This QR code expired. Reload to try again.'); return; }
      try {
        const r = await fetch('/auth/ghostid/poll/' + encodeURIComponent(data.challengeId), { credentials: 'same-origin' });
        const body = await r.json().catch(function () { return {}; });
        if (!r.ok) { fail(body.error || 'GhostID login failed. Reload to try again.'); return; }
        if (body.status === 'verified') {
          clearInterval(timer);
          window.location.replace('/cli/auth/complete');
        }
      } catch (e) {
        fail('Network error while waiting for approval. Reload to try again.');
      }
    }, 2000);
  } catch (e) {
    fail('Could not start GhostID login. Reload to try again.');
  }
})();
</script>
</body>
</html>"#;

pub(crate) async fn ghostid_auth_page() -> Html<&'static str> {
    Html(GHOSTID_LOGIN_PAGE)
}

pub(crate) async fn ghostid_start_challenge(State(state): State<crate::AppState>, jar: CookieJar) -> Response {
    let Some(service) = state.ghostid.as_ref() else {
        return ghostid_unavailable();
    };
    let secure = ghostid_is_secure();
    match service.start_login(ghostid_cookie_value(&jar).as_deref()).await {
        Ok(started) => {
            let jar = match started.new_session_token {
                Some(token) => jar.add(ghostid_cookie(token, secure)),
                None => jar,
            };
            (
                jar,
                Json(serde_json::json!({
                    "challengeId": started.challenge_id,
                    "qrUrl": started.qr_url,
                    "expiresAt": started.expires_at,
                })),
            )
                .into_response()
        }
        Err(err) => ghostid_error(err),
    }
}

pub(crate) async fn ghostid_poll(
    State(state): State<crate::AppState>,
    jar: CookieJar,
    Path(challenge_id): Path<String>,
) -> Response {
    let Some(service) = state.ghostid.as_ref() else {
        return ghostid_unavailable();
    };
    let secure = ghostid_is_secure();
    match service.poll_login(ghostid_cookie_value(&jar).as_deref(), &challenge_id).await {
        Ok(crate::ghostid_login::PollOutcome::Pending) => {
            Json(serde_json::json!({"status": "pending"})).into_response()
        }
        Ok(crate::ghostid_login::PollOutcome::Verified { session_token, user }) => {
            info!("GhostID login verified for user {}", user.user_id);
            (
                clear_oauth_cookie(jar).add(ghostid_cookie(session_token, secure)),
                Json(serde_json::json!({"status": "verified", "user": user})),
            )
                .into_response()
        }
        Err(err) => ghostid_error(err),
    }
}

pub(crate) async fn ghostid_me(State(state): State<crate::AppState>, jar: CookieJar) -> Response {
    let Some(service) = state.ghostid.as_ref() else {
        return ghostid_unavailable();
    };
    match service.current_user(ghostid_cookie_value(&jar).as_deref()).await {
        Ok(user) => Json(serde_json::json!({"user": user})).into_response(),
        Err(err) => ghostid_error(err),
    }
}

pub(crate) async fn ghostid_logout(State(state): State<crate::AppState>, jar: CookieJar) -> Response {
    let Some(service) = state.ghostid.as_ref() else {
        return ghostid_unavailable();
    };
    if let Err(err) = service.logout(ghostid_cookie_value(&jar).as_deref()).await {
        return ghostid_error(err);
    }
    let jar = clear_ghostid_cookies(jar);
    (jar, Json(serde_json::json!({"status": "signed_out"}))).into_response()
}

/// Expires the GhostID session cookies. `__Host-` cookies must be sent with Secure to be cleared.
fn clear_ghostid_cookies(jar: CookieJar) -> CookieJar {
    jar.remove(Cookie::build((crate::ghostid_login::SESSION_COOKIE, "")).path("/").secure(true))
        .remove(Cookie::build(("ghostid", "")).path("/"))
}

/// Expires the OAuth session cookie so a GhostID login is not overridden by an older Google session.
fn clear_oauth_cookie(jar: CookieJar) -> CookieJar {
    jar.remove(Cookie::build(("mothership_session", "")).path("/"))
}
