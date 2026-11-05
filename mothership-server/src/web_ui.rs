use axum::{
    extract::{State, Json, Path},
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
use urlencoding;
use std::fs;
use uuid;
use qrcode::QrCode;
use base64::{self, Engine};

/// Web UI routes for authentication and CLI downloads
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/", get(index_page))
    .route("/login", get(login_page))
    .route("/auth/start", get(auth_start))
    .route("/auth/ghostid", get(ghostid_auth_page))
    .route("/auth/ghostid/qr/:challenge_id", get(ghostid_qr_page))
    .route("/auth/ghostid/poll/:challenge_id", get(ghostid_poll_status))
    .route("/auth/ghostid/callback", post(ghostid_auth_callback))
    .route("/auth/ghostid/test", get(ghostid_callback_test))
        .route("/download", get(download_page))
        .route("/download/authenticated", get(authenticated_download_page))
        .route("/auth/callback", post(auth_callback))
        .route("/auth/finalize", get(auth_finalize))
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
) -> Result<Redirect, StatusCode> {
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

    Ok(Redirect::temporary(&auth_url))
}

/// Login page that starts OAuth flow
async fn login_page(State(state): State<crate::AppState>) -> Result<Html<String>, StatusCode> {
    if !state.config.features.oauth_enabled {
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
        ("AUTH_CONTENT", r#"<a href="/auth/start" class="auth-btn">Sign in with OAuth</a>
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
    generate_download_page_html(None, None, None, &state).await
}

/// Authenticated download page (after successful OAuth)
async fn authenticated_download_page(
    jar: CookieJar,
    State(state): State<crate::AppState>,
) -> Result<Html<String>, StatusCode> {
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
        Some(session_data.token), 
        Some(session_data.username), 
        Some(session_data.email), 
        &state
    ).await)
}

/// Generate the download page HTML
async fn generate_download_page_html(
    token: Option<String>,
    username: Option<String>,
    email: Option<String>,
    _state: &crate::AppState,
) -> Html<String> {
    let server_url = std::env::var("OAUTH_BASE_URL")
        .or_else(|_| std::env::var("MOTHERSHIP_SERVER_URL"))
        .unwrap_or_else(|_| "http://localhost:7523".to_string());
    
    let is_authenticated = token.is_some();
    
    let version = env!("CARGO_PKG_VERSION");
    
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
        
        {}
        
        <div class="download-methods">
            <div class="method">
                <h3>Your Authentication Token</h3>
                <p>Copy this token for use with the installation commands below:</p>
                <div class="code-block">
                    <button class="copy-btn" onclick="copyToClipboard('auth-token')">Copy</button>
                    <code id="auth-token">{}</code>
                </div>
            </div>
            
            <div class="method">
                <h3>Quick Install (Unix/Linux/macOS)</h3>
                <p>One-liner installation script:</p>
                <div class="code-block">
                    <button class="copy-btn" onclick="copyToClipboard('unix-install')">Copy</button>
                    <code id="unix-install">{}</code>
                </div>
            </div>
            
            <div class="method">
                <h3>Windows Installation</h3>
                <p>PowerShell installation script:</p>
                <div class="code-block">
                    <button class="copy-btn" onclick="copyToClipboard('windows-install')">Copy</button>
                    <code id="windows-install">{}</code>
                </div>
            </div>
        </div>
        
        <div class="platform-downloads">
            <h2>Direct Downloads</h2>
            <p>Download specific binaries for your platform:</p>
            
            <div class="platforms">
                <div class="platform">
                    <h4>Linux x64</h4>
                    <a href="{}/cli/download/{}/x86_64-unknown-linux-gnu/mothership" class="download-btn">CLI</a>
                    <a href="{}/cli/download/{}/x86_64-unknown-linux-gnu/mothership-daemon" class="download-btn">Daemon</a>
                </div>
                
                <div class="platform">
                    <h4>Linux ARM64</h4>
                    <a href="{}/cli/download/{}/aarch64-unknown-linux-gnu/mothership" class="download-btn">CLI</a>
                    <a href="{}/cli/download/{}/aarch64-unknown-linux-gnu/mothership-daemon" class="download-btn">Daemon</a>
                </div>
                
                <div class="platform">
                    <h4>macOS x64</h4>
                    <a href="{}/cli/download/{}/x86_64-apple-darwin/mothership" class="download-btn">CLI</a>
                    <a href="{}/cli/download/{}/x86_64-apple-darwin/mothership-daemon" class="download-btn">Daemon</a>
                </div>
                
                <div class="platform">
                    <h4>macOS ARM64</h4>
                    <a href="{}/cli/download/{}/aarch64-apple-darwin/mothership" class="download-btn">CLI</a>
                    <a href="{}/cli/download/{}/aarch64-apple-darwin/mothership-daemon" class="download-btn">Daemon</a>
                </div>
                
                <div class="platform">
                    <h4>Windows x64</h4>
                    <a href="{}/cli/download/{}/x86_64-pc-windows-msvc/mothership.exe" class="download-btn">CLI</a>
                    <a href="{}/cli/download/{}/x86_64-pc-windows-msvc/mothership-daemon.exe" class="download-btn">Daemon</a>
                </div>
            </div>
        </div>
        
        <div class="note">
            <h3>Next Steps</h3>
            <ol>
                <li>Download and install the CLI using one of the methods above</li>
                <li>Run <code>mothership auth</code> to authenticate with this server</li>
                <li>Use <code>mothership --help</code> to see all available commands</li>
                <li>Run <code>mothership update</code> to check for updates</li>
            </ol>
        </div>
        
        {}
    </div>
    
    <script>
        function copyToClipboard(elementId) {{
            const element = document.getElementById(elementId);
            const text = element.textContent;
            navigator.clipboard.writeText(text).then(() => {{
                const btn = element.parentElement.querySelector('.copy-btn');
                const originalText = btn.textContent;
                btn.textContent = 'Copied!';
                setTimeout(() => {{
                    btn.textContent = originalText;
                }}, 2000);
            }});
        }}
    </script>
</body>
</html>
"#,
        if is_authenticated {
            format!(r#"<div class="user-info">
                <h3>Authenticated as {}</h3>
                <p>Email: {}</p>
                <p>You have access to download all CLI tools.</p>
            </div>"#,
                username.as_deref().unwrap_or("Unknown"),
                email.as_deref().unwrap_or("Unknown")
            )
        } else {
            String::new()
        },
        if is_authenticated { 
            token.as_ref().unwrap()
        } else { 
            "No token available - please authenticate first"
        },
        if is_authenticated { 
            format!("MOTHERSHIP_TOKEN={} curl -sSL {}/cli/install | bash", token.as_ref().unwrap(), server_url) 
        } else { 
            String::new() 
        },
        if is_authenticated { 
            format!("$env:MOTHERSHIP_TOKEN=\"{}\"; irm {}/cli/install/windows | iex", token.as_ref().unwrap(), server_url) 
        } else { 
            String::new() 
        },
        // Platform downloads - exactly 20 pairs for 5 platforms × 2 binaries × 2 args each
        server_url, version,
        server_url, version,
        server_url, version,
        server_url, version,
        server_url, version,
        server_url, version,
        server_url, version,
        server_url, version,
        server_url, version,
        server_url, version,
        if is_authenticated {
            r#"<div class="warning">
                <h3>Secure Token</h3>
                <p>Your authentication token is embedded in the download links above. Keep this page secure and don't share the URLs with others.</p>
            </div>"#
        } else {
            r#"<div class="warning">
                <h3>Authentication Required</h3>
                <p>Direct downloads require authentication. Please use the installation scripts above or authenticate first.</p>
            </div>"#
        }
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
    let success_url = "/download/authenticated";

    Ok((
        CookieJar::new().add(cookie),
        axum::response::Redirect::to(success_url)
    ).into_response())
}

/// GhostID wallet authentication page - creates QR challenge and shows QR code
async fn ghostid_auth_page(State(state): State<crate::AppState>) -> Result<Html<String>, StatusCode> {
    // Create QR challenge via internal API
    let challenge_req = crate::qr_auth::QrStartRequest {
        action: Some("download".to_string()),
    };
    
    match crate::qr_auth::start_qr_challenge(State(state), Json(challenge_req)).await {
        Ok(Json(challenge_response)) => {
            let qr_url = format!("/auth/ghostid/qr/{}", challenge_response.challenge_id);
            
            let replacements = [
                ("QR_URL", qr_url.as_str()),
                ("CHALLENGE_ID", challenge_response.challenge_id.as_str()),
            ];
            
            match load_template("ghostid-auth.html", &replacements) {
                Ok(html) => Ok(Html(html)),
                Err(e) => {
                    error!("Failed to load ghostid-auth template: {}", e);
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
        Err(_) => {
            error!("Failed to create QR challenge");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Generate QR code image for GhostID authentication
async fn ghostid_qr_page(
    Path(challenge_id): Path<String>,
) -> impl IntoResponse {
    match crate::qr_auth::get_qr_challenge(&challenge_id).await {
        Some(challenge) => {
            let oauth_base_url = std::env::var("OAUTH_BASE_URL")
                .unwrap_or_else(|_| "https://app.mothership.rest".to_string());
            
            let callback_url = format!("{}/auth/ghostid/callback?challenge_id={}", oauth_base_url, challenge_id);
            
            let mut challenge_json = serde_json::json!({
                "protocolVersion": 1,
                "challengeId": challenge.challenge_id,
                "domain": challenge.domain,
                "action": challenge.action,
                "nonce": challenge.nonce,
                "issuedAt": challenge.created_at.timestamp(),
                "expiresAt": challenge.expires_at.timestamp(),
                "payload": {},
                "permissions": challenge.permissions,
                "tier": challenge.tier,
                "duration_days": challenge.duration_days,
                "callbackUrl": callback_url
            });
            
            // Provider fields let the wallet check this challenge against the DNS record.
            let (provider_public_key, provider_signature) = match crate::ghostid_provider::sign_challenge(&challenge) {
                Some(signed) => signed,
                None => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            };
            challenge_json["providerPublicKey"] = serde_json::Value::String(provider_public_key);
            challenge_json["providerSignature"] = serde_json::Value::String(provider_signature);
            
            // Convert to base64 like the auth-server does
            let json_string = match serde_json::to_string(&challenge_json) {
                Ok(s) => s,
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            };
            let base64_data = base64::engine::general_purpose::STANDARD.encode(json_string.as_bytes());
            
            // Generate QR code as simple text and convert to SVG
            match QrCode::new(&base64_data) {
                Ok(code) => {
                    // Render as string first
                    let qr_string = code.render::<char>()
                        .quiet_zone(false)
                        .module_dimensions(1, 1)
                        .dark_color('█')
                        .light_color(' ')
                        .build();
                    
                    // Convert to SVG manually
                    let lines: Vec<&str> = qr_string.lines().collect();
                    let size = lines.len();
                    let scale = 4; // Scale factor for visibility
                    let svg_size = size * scale;
                    
                    let mut svg = format!(
                        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}">"#,
                        svg_size, svg_size, size, size
                    );
                    
                    for (y, line) in lines.iter().enumerate() {
                        for (x, ch) in line.chars().enumerate() {
                            if ch == '█' {
                                svg.push_str(&format!(
                                    r#"<rect x="{}" y="{}" width="1" height="1" fill="black"/>"#,
                                    x, y
                                ));
                            }
                        }
                    }
                    
                    svg.push_str("</svg>");
                    
                    (
                        [
                            ("content-type", "image/svg+xml"),
                            ("cache-control", "no-cache"),
                        ],
                        svg
                    ).into_response()
                }
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
        None => StatusCode::NOT_FOUND.into_response()
    }
}

/// Poll QR authentication status and create session if verified
async fn ghostid_poll_status(
    Path(challenge_id): Path<String>,
    State(state): State<crate::AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    // Get the challenge status via internal API
    match crate::qr_auth::get_qr_status(Path(challenge_id.clone())).await {
        Ok(Json(status)) => {
            // If verified, create a session like we do for OAuth
            if status.status == "verified" {
                if let Some(user_address) = &status.user_address {
                    // Create session for ghostid wallet user
                    let session_id = uuid::Uuid::new_v4().to_string();
                    let username = format!("ghostid:{}", user_address);
                    let email = format!("{}@ghostid.wallet", user_address);
                    let now = chrono::Utc::now();
                    
                    // Generate a random UUID for the ghostid wallet user
                    let user_uuid = uuid::Uuid::new_v4();
                    
                    let session_data = crate::SessionData {
                        user_id: user_uuid,
                        username: username.clone(),
                        email: email.clone(),
                        token: session_id.clone(), // Use session ID as token for ghostid auth
                        expires_at: now + chrono::Duration::hours(24),
                        created_at: now,
                    };
                    
                    // Store session
                    {
                        let mut sessions = state.sessions.write().await;
                        sessions.insert(session_id.clone(), session_data);
                    }
                    
                    info!("Created ghostid wallet session for user: {} ({})", username, email);
                    
                    // Get the web UI URL for cookie domain
                    let web_ui_url = std::env::var("WEB_UI_BASE_URL")
                        .or_else(|_| std::env::var("OAUTH_BASE_URL"))
                        .unwrap_or_else(|_| "http://localhost:7523".to_string());
                    
                    // Create session cookie - determine secure flag and domain
                    let is_secure = web_ui_url.starts_with("https");
                    let is_localhost = web_ui_url.contains("localhost") || web_ui_url.contains("127.0.0.1");
                    
                    let mut cookie_builder = Cookie::build(("mothership_session", session_id.clone()))
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
                                    info!("Setting session cookie domain to: {}", base_domain);
                                    cookie_builder = cookie_builder.domain(base_domain);
                                }
                            }
                        }
                    }
                    
                    let cookie = cookie_builder.build();
                    
              info!("GhostID session cookie created - secure: {}, localhost: {}, domain: {:?}", 
                  is_secure, is_localhost, cookie.domain());
                    
                    // Return success status with cookie
                    Ok((
                        CookieJar::new().add(cookie),
                        Json(serde_json::json!({
                            "status": "verified",
                            "user_address": user_address,
                            "session_id": session_id
                        }))
                    ).into_response())
                } else {
                    Ok(Json(serde_json::json!({"status": "failed", "error": "missing_address"})).into_response())
                }
            } else {
                // Return the current status (pending, failed, etc.)
                Ok(Json(serde_json::json!(status)).into_response())
            }
        }
        Err(status_code) => Err(status_code)
    }
}

/// Verifies the wallet's ed25519 signature over the canonical challenge message
/// (same construction as the GhostID auth-server).
fn verify_wallet_signature(
    challenge: &crate::qr_auth::QrChallenge,
    public_key_b64: &str,
    token_id: &str,
    signature_b64: &str,
) -> bool {
    let engine = base64::engine::general_purpose::STANDARD;
    let (Ok(public_key), Ok(signature)) = (engine.decode(public_key_b64), engine.decode(signature_b64)) else {
        return false;
    };
    let message = challenge.canonical_auth_message(public_key_b64, token_id);
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &public_key)
        .verify(message.as_bytes(), &signature)
        .is_ok()
}

/// Handle callback from GhostID Chain after mobile authentication
async fn ghostid_auth_callback(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
    Json(callback_data): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    info!("GhostID AUTH CALLBACK ENDPOINT HIT!");
    info!("Received GhostID Chain callback: {:?}", callback_data);
    info!("Query parameters: {:?}", params);
    
    // Extract challengeId, publicKey, signature, and tokenId from mobile app callback data
    info!("Extracting fields from callback data...");
    let challenge_id_from_payload = callback_data.get("challengeId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            error!("Missing 'challengeId' field in callback data");
            StatusCode::BAD_REQUEST
        })?;
    
    let public_key = callback_data.get("publicKey")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            error!("Missing 'publicKey' field in callback data");
            StatusCode::BAD_REQUEST
        })?;
    
    let signature = callback_data.get("signature")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            error!("Missing 'signature' field in callback data");
            StatusCode::BAD_REQUEST
        })?;
    
    let token_id = callback_data.get("tokenId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            error!("Missing 'tokenId' field in callback data");
            StatusCode::BAD_REQUEST
        })?;
    
    info!("Successfully extracted fields - challengeId: {}, tokenId: {}, publicKey: {}...", 
          challenge_id_from_payload, token_id, &public_key[..std::cmp::min(8, public_key.len())]);
    
    // Extract challenge_id from query parameters (should match challengeId from payload)
    info!("Extracting challenge_id from query parameters...");
    let challenge_id = params.get("challenge_id")
        .ok_or_else(|| {
            error!("Missing 'challenge_id' query parameter");
            StatusCode::BAD_REQUEST
        })?;
    
    // Verify challenge IDs match
    info!("Verifying challenge IDs match...");
    if challenge_id != challenge_id_from_payload {
        error!("Challenge ID mismatch: URL param '{}' != payload '{}'", challenge_id, challenge_id_from_payload);
        return Err(StatusCode::BAD_REQUEST);
    }
    info!("Challenge IDs match: {}", challenge_id);
    
    info!("Processing callback for challenge_id: {}, token_id: {}, public_key: {}...", 
          challenge_id, token_id, &public_key[..std::cmp::min(8, public_key.len())]);
    
    // Verify the challenge exists
    info!("Looking up challenge in database...");
    let challenge = crate::qr_auth::get_qr_challenge(challenge_id).await;
    
    if let Some(challenge) = challenge {
        info!("Challenge found for domain: {}", challenge.domain);

        if chrono::Utc::now() >= challenge.expires_at {
            error!("Challenge expired: {}", challenge_id);
            return Err(StatusCode::BAD_REQUEST);
        }

        if !verify_wallet_signature(&challenge, public_key, token_id, signature) {
            error!("Invalid wallet signature for challenge {}", challenge_id);
            return Err(StatusCode::UNAUTHORIZED);
        }

        // Re-check the provider DNS record so a removed or rotated key stops logins.
        if let Err(reason) = crate::ghostid_provider::verify_published_key().await {
            error!("GhostID login rejected for challenge {}: {}", challenge_id, reason);
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }

        if !crate::qr_auth::mark_qr_challenge_used(challenge_id).await {
            error!("Challenge already used: {}", challenge_id);
            return Err(StatusCode::CONFLICT);
        }
        
    // For mobile wallet authentication, we don't need to verify domain since 
    // the mobile app already validated the auth token on-chain
    info!("Creating user address from public key...");
        
    // Create a user address from the public key for identification
    let user_address = format!("ghostid_wallet_{}", public_key);
    info!("Generated user address for public key {}...", &public_key[..std::cmp::min(8, public_key.len())]);
        
        // Update the QR result to verified
        info!("Updating QR challenge status to verified...");
        crate::qr_auth::update_qr_result(challenge_id.to_string(), crate::qr_auth::QrResult {
            status: "verified".to_string(),
            user_address: Some(user_address),
            verified_at: Some(chrono::Utc::now()),
        }).await;
        
        info!("Updated QR challenge {} status to verified", challenge_id);
        
        let response = serde_json::json!({
            "success": true,
            "message": "Callback processed successfully",
            "challenge_id": challenge_id,
            "token_id": token_id
        });
        info!("Sending success response: {:?}", response);
        return Ok(Json(response));
    } else {
        error!("Challenge not found: {}", challenge_id);
    }
    
    error!("Failed to process callback: invalid challenge");
    Err(StatusCode::BAD_REQUEST)
}

/// Test endpoint to verify the callback route is accessible
async fn ghostid_callback_test() -> Json<serde_json::Value> {
    info!("GhostID callback test endpoint hit!");
    Json(serde_json::json!({
        "status": "ok",
        "message": "GhostID callback endpoint is accessible",
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}

#[cfg(test)]
mod ghostid_signature_tests {
    use super::verify_wallet_signature;
    use crate::qr_auth::QrChallenge;
    use base64::Engine;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    fn sample_challenge() -> QrChallenge {
        let now = chrono::Utc::now();
        QrChallenge {
            challenge_id: "chal-1".to_string(),
            domain: "mothership.cli".to_string(),
            action: "login".to_string(),
            nonce: "nonce-1".to_string(),
            created_at: now,
            expires_at: now + chrono::Duration::minutes(5),
            permissions: vec!["login".to_string(), "access".to_string()],
            tier: "early access".to_string(),
            duration_days: 365,
        }
    }

    #[test]
    fn accepts_valid_signature_and_rejects_tampering() {
        let engine = base64::engine::general_purpose::STANDARD;
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("keygen");
        let keypair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("parse key");

        let challenge = sample_challenge();
        let public_key = engine.encode(keypair.public_key().as_ref());
        let token_id = "token-42";
        let message = challenge.canonical_auth_message(&public_key, token_id);
        let signature = engine.encode(keypair.sign(message.as_bytes()).as_ref());

        assert!(verify_wallet_signature(&challenge, &public_key, token_id, &signature));
        assert!(!verify_wallet_signature(&challenge, &public_key, "other-token", &signature));

        let mut other = sample_challenge();
        other.nonce = "nonce-2".to_string();
        assert!(!verify_wallet_signature(&other, &public_key, token_id, &signature));

        assert!(!verify_wallet_signature(&challenge, &public_key, token_id, "not-base64!"));
    }
}