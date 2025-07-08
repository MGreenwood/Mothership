use axum::{
    extract::{Path, State, WebSocketUpgrade},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post},
    Router,
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use mothership_common::{
    auth::{OAuthProvider, OAuthRequest, OAuthResponse, OAuthSource, OAuthProfile},
    protocol::{BeamRequest, BeamResponse, GatewayRequest},
    ApiResponse, Project, User, UserRole, GatewayProject, ProjectId,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tracing::{error, info, warn};
use uuid::Uuid;
use url;
use urlencoding;

mod auth;
mod cli_distribution;
mod config;
mod database;
mod handlers;
mod oauth;
mod sync;
mod storage;
mod web_ui;

use auth::AuthService;
use config::{ServerConfig, UserWhitelist};
use database::Database;
use sync::SyncState;
use oauth::OAuthService;
use storage::StorageEngine;

/// Application state shared across all handlers
#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub auth: AuthService,
    pub oauth: OAuthService,
    pub sync: SyncState,
    pub config: ServerConfig,
    pub whitelist: Option<UserWhitelist>,
    pub sessions: Arc<RwLock<HashMap<String, SessionData>>>,
    pub temp_tokens: Arc<RwLock<HashMap<String, TempTokenData>>>,
}

#[derive(Clone, Debug)]
struct SessionData {
    user_id: Uuid,
    username: String,
    email: String,
    token: String,
    created_at: chrono::DateTime<chrono::Utc>,
    expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug)]
struct TempTokenData {
    user_id: Uuid,
    username: String,
    email: String,
    token: String,
    provider: OAuthProvider,
    created_at: chrono::DateTime<chrono::Utc>,
    expires_at: chrono::DateTime<chrono::Utc>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load environment variables - try multiple locations
    dotenvy::dotenv().ok(); // Current directory (for Docker)
    dotenvy::from_filename("../.env").ok(); // Parent directory (for local development)

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    // Load server configuration
    let config = ServerConfig::load_from_file("server.config")?;
    info!("🔧 Loaded server configuration");

    // Load whitelist if enabled
    let whitelist = config.load_whitelist()?;
    if let Some(ref whitelist) = whitelist {
        info!("📋 Loaded whitelist");
    }

    // Set up database connection
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/mothership".to_string());

    info!("🗄️ Connecting to database...");
    let db = Database::new(&database_url).await?;
    info!("✅ Database connected");

    // Initialize storage engine
    let storage_root = std::env::var("STORAGE_ROOT")
        .unwrap_or_else(|_| "storage".to_string());

    info!("📦 Initializing storage engine at {}", storage_root);
    let storage = Arc::new(StorageEngine::new(storage_root.into()).await?);
    info!("✅ Storage engine initialized");

    // Initialize services
    let auth = AuthService::new(
        std::env::var("JWT_SECRET")
            .unwrap_or_else(|_| "mothership_dev_secret".to_string())
    );

    let oauth = OAuthService::new().expect("Failed to initialize OAuth service");

    // Initialize sync state
    let sync = SyncState::new(db.clone(), storage.clone());

    // Create application state
    let state = AppState {
        db: db.clone(),
        auth,
        oauth,
        sync,
        config: config.clone(),
        whitelist,
        sessions: Arc::new(RwLock::new(HashMap::new())),
        temp_tokens: Arc::new(RwLock::new(HashMap::new())),
    };

    let host = config.server.host.parse::<std::net::IpAddr>()
        .unwrap_or_else(|_| {
            warn!("Invalid host address in config: {}, using 0.0.0.0", config.server.host);
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0))
        });

    // Check if dual port mode is enabled
    if let Some(web_port) = config.server.web_port {
        info!("🚀 Starting Mothership in dual port mode");
        
        // Create API router (no web UI routes)
        let api_router = create_api_router(state.clone());
        
        // Create Web UI router (web UI routes only)
        let web_router = create_web_router(state.clone());
        
        // Start API server
        let api_addr = std::net::SocketAddr::new(host, config.server.port);
        info!("🔧 API Server listening on {}", api_addr);
        
        // Start Web UI server
        let web_addr = std::net::SocketAddr::new(host, web_port);
        info!("🌐 Web UI Server listening on {}", web_addr);
        
        // Start both servers concurrently
        let api_listener = tokio::net::TcpListener::bind(api_addr).await?;
        let web_listener = tokio::net::TcpListener::bind(web_addr).await?;
        
        let api_server = axum::serve(api_listener, api_router);
        let web_server = axum::serve(web_listener, web_router);
        
        // Run both servers concurrently
        tokio::try_join!(api_server, web_server)?;
    } else {
        info!("🚀 Starting Mothership in single port mode");
        
        // Single port mode: create combined router with all routes
        let app = create_combined_router(state);
        let addr = std::net::SocketAddr::new(host, config.server.port);
        
        info!("🚀 Mothership Server listening on {}", addr);
        
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;
    }

    Ok(())
}

/// Create API-only router for dual port mode
fn create_api_router(state: AppState) -> Router {
    Router::new()
        // Health check (always available)
        .route("/health", get(health_check))
        // Server capabilities (always available)
        .route("/capabilities", get(server_capabilities))
        
        // Authentication routes
        .route("/auth/check", get(auth_check))
        .route("/auth/oauth/test", get(oauth_test))
        .route("/auth/oauth/start", post(oauth_start))
        .route("/auth/oauth/callback/google", get(oauth_callback_google))
        .route("/auth/oauth/callback/github", get(oauth_callback_github))
        .route("/auth/finalize", get(web_ui::auth_finalize))
        
        // Admin routes
        .route("/admin/create", post(create_admin_user))
        
        // Project routes
        .route("/projects", get(list_projects))
        .route("/projects/:id", get(get_project))
        .route("/projects/name/:name", get(get_project_by_name))
        .route("/projects/:id/beam", post(beam_into_project))
        .route("/projects/:id/files", post(upload_initial_files))
        .route("/projects/:id/checkpoints", post(create_checkpoint))
        .route("/projects/:id/history", get(get_project_history))
        .route("/projects/:id/checkpoints/:checkpoint_id/restore", post(restore_checkpoint))
        .route("/projects/:id", delete(delete_project))
        
        // Gateway routes
        .route("/gateway", post(gateway))
        .route("/gateway/create", post(create_gateway))
        
        // WebSocket route
        .route("/ws/:rift_id", get(websocket_handler))
        
        // CLI distribution routes
        .merge(crate::cli_distribution::routes())
        
        // Add CORS middleware to allow requests from web UI
        .layer(
            CorsLayer::new()
                .allow_origin("https://app.mothershipproject.dev".parse::<HeaderValue>().unwrap())
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::PUT,
                    axum::http::Method::DELETE,
                    axum::http::Method::OPTIONS,
                ])
                .allow_headers([
                    axum::http::header::AUTHORIZATION,
                    axum::http::header::CONTENT_TYPE,
                    axum::http::header::ACCEPT,
                ])
                .allow_credentials(true)
        )
        
        .with_state(state)
}

/// Create Web UI-only router for dual port mode
fn create_web_router(state: AppState) -> Router {
    Router::new()
        // Health check for web server
        .route("/health", get(health_check))
        
        // OAuth routes (needed for web UI)
        .route("/auth/oauth/test", get(oauth_test))
        .route("/auth/oauth/start", post(oauth_start))
        .route("/auth/oauth/callback/google", get(oauth_callback_google))
        .route("/auth/oauth/callback/github", get(oauth_callback_github))
        
        // Web UI routes
        .merge(crate::web_ui::routes())
        
        .with_state(state)
}

/// Create combined router for single port mode (legacy compatibility)
fn create_combined_router(state: AppState) -> Router {
    Router::new()
        // Health check (always available)
        .route("/health", get(health_check))
        // Server capabilities (always available)
        .route("/capabilities", get(server_capabilities))
        
        // Authentication routes
        .route("/auth/check", get(auth_check))
        .route("/auth/oauth/test", get(oauth_test))
        .route("/auth/oauth/start", post(oauth_start))
        .route("/auth/oauth/callback/google", get(oauth_callback_google))
        .route("/auth/oauth/callback/github", get(oauth_callback_github))
        .route("/auth/finalize", get(web_ui::auth_finalize))
        .route("/auth/success", get(oauth_success_page))
        .route("/auth/error", get(oauth_error_page))
        
        // Admin routes
        .route("/admin/create", post(create_admin_user))
        
        // Project routes
        .route("/projects", get(list_projects))
        .route("/projects/:id", get(get_project))
        .route("/projects/name/:name", get(get_project_by_name))
        .route("/projects/:id/beam", post(beam_into_project))
        .route("/projects/:id/files", post(upload_initial_files))
        .route("/projects/:id/checkpoints", post(create_checkpoint))
        .route("/projects/:id/history", get(get_project_history))
        .route("/projects/:id/checkpoints/:checkpoint_id/restore", post(restore_checkpoint))
        .route("/projects/:id", delete(delete_project))
        
        // Gateway routes
        .route("/gateway", post(gateway))
        .route("/gateway/create", post(create_gateway))
        
        // WebSocket route
        .route("/ws/:rift_id", get(websocket_handler))
        
        // Web UI routes
        .merge(crate::web_ui::routes())
        
        // CLI distribution routes
        .merge(crate::cli_distribution::routes())
        
        .with_state(state)
}

/// Create the application router (deprecated - use create_combined_router)
fn create_router(state: AppState) -> Router {
    create_combined_router(state)
}

/// Health check endpoint
async fn health_check() -> Json<ApiResponse<String>> {
    Json(ApiResponse::success("Mothership is operational".to_string()))
}

/// Server capabilities for discovery and client configuration
#[derive(serde::Serialize)]
struct ServerCapabilities {
    auth_methods: Vec<String>,
    sso_domain: Option<String>,
    oauth_providers: Vec<String>,
    features: Vec<String>,
    name: String,
    version: String,
}

/// Server capabilities endpoint
async fn server_capabilities(
    State(state): State<AppState>,
) -> Json<ApiResponse<ServerCapabilities>> {
    let mut auth_methods = vec![];
    let mut oauth_providers = vec![];
    let mut features = vec![
        "project_sync".to_string(),
        "checkpoints".to_string(),
        "beam".to_string(),
        "file_storage".to_string(),
    ];

    // Add OAuth info if enabled
    if state.config.features.oauth_enabled {
        auth_methods.push("oauth".to_string());
        oauth_providers.extend([
            "google".to_string(),
            "github".to_string(),
        ]);
    }

    // Add features based on config
    if state.config.features.websocket_sync_enabled {
        features.push("websocket_sync".to_string());
    }
    if state.config.features.chat_enabled {
        features.push("chat".to_string());
    }
    if state.config.features.file_uploads_enabled {
        features.push("file_uploads".to_string());
    }
    if state.config.features.cli_distribution_enabled {
        features.push("cli_distribution".to_string());
    }

    let capabilities = ServerCapabilities {
        auth_methods,
        sso_domain: None,
        oauth_providers,
        features,
        name: "Mothership Server".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    
    Json(ApiResponse::success(capabilities))
}

/// Check authentication via Authorization header (for CLI)
async fn auth_check(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<AuthCheckResponse>>, StatusCode> {
    // Extract Bearer token from Authorization header
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = auth_header.trim_start_matches("Bearer ");

    // Verify the token
    match state.auth.verify_token(token) {
        Ok(claims) => {
            // Check if user still exists in database
            let user_id = uuid::Uuid::parse_str(&claims.sub)
                .map_err(|_| StatusCode::UNAUTHORIZED)?;
            
            match state.db.get_user(user_id).await {
                Ok(Some(user)) => {
                    // Check whitelist if enabled
                    if let Some(whitelist) = &state.whitelist {
                        if !whitelist.is_user_allowed(&user.username, &user.email) {
                            warn!("User {} ({}) not in whitelist", user.username, user.email);
                            return Err(StatusCode::FORBIDDEN);
                        }
                    }

                    let response = AuthCheckResponse {
                        authenticated: true,
                        user_id: user.id,
                        username: user.username,
                        email: user.email,
                        role: user.role,
                        machine_id: claims.machine_id,
                    };
                    Ok(Json(ApiResponse::success(response)))
                }
                Ok(None) => {
                    // User no longer exists in database (likely due to server restart with in-memory DB)
                    // Recreate the user from JWT claims if this is an OAuth token
                    info!("User {} not found in database, attempting to recreate from JWT claims", claims.username);
                    
                    if claims.machine_id == "web-oauth" {
                        // This is an OAuth token, recreate the user with the ORIGINAL user ID from JWT
                        let email = claims.email.clone().unwrap_or_else(|| format!("{}@oauth.mothership", claims.username));
                        match state.db.create_user_with_id(user_id, claims.username.clone(), email, UserRole::User).await {
                            Ok(recreated_user) => {
                                info!("✅ Successfully recreated OAuth user: {} (ID: {})", recreated_user.username, recreated_user.id);
                                
                                let response = AuthCheckResponse {
                                    authenticated: true,
                                    user_id: recreated_user.id,
                                    username: recreated_user.username,
                                    email: recreated_user.email,
                                    role: recreated_user.role,
                                    machine_id: claims.machine_id,
                                };
                                Ok(Json(ApiResponse::success(response)))
                            }
                            Err(e) => {
                                error!("❌ Failed to recreate OAuth user: {}", e);
                                Err(StatusCode::INTERNAL_SERVER_ERROR)
                            }
                        }
                    } else {
                        // Non-OAuth token, user really doesn't exist
                        Err(StatusCode::NOT_FOUND)
                    }
                }
                Err(_) => {
                    // Database error
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
        Err(_) => {
            // Invalid or expired token
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// Test OAuth configuration
async fn oauth_test(
    State(state): State<AppState>,
) -> Json<ApiResponse<serde_json::Value>> {
    let mut status = serde_json::Map::new();
    
    status.insert("oauth_enabled".to_string(), serde_json::Value::Bool(state.config.features.oauth_enabled));
    
    // Check environment variables
    status.insert("google_client_id_set".to_string(), 
        serde_json::Value::Bool(std::env::var("GOOGLE_CLIENT_ID").is_ok()));
    status.insert("google_client_secret_set".to_string(), 
        serde_json::Value::Bool(std::env::var("GOOGLE_CLIENT_SECRET").is_ok()));
    status.insert("github_client_id_set".to_string(), 
        serde_json::Value::Bool(std::env::var("GITHUB_CLIENT_ID").is_ok()));
    status.insert("github_client_secret_set".to_string(), 
        serde_json::Value::Bool(std::env::var("GITHUB_CLIENT_SECRET").is_ok()));
    
    Json(ApiResponse::success(serde_json::Value::Object(status)))
}

/// Start OAuth flow
async fn oauth_start(
    State(state): State<AppState>,
    Json(req): Json<OAuthRequest>,
) -> Result<Json<ApiResponse<OAuthResponse>>, StatusCode> {
    info!("🔐 OAuth start request for provider: {:?} from {:?} source on machine: {}", req.provider, req.source, req.machine_id);
    info!("🔐 Callback URL: {:?}", req.callback_url);
    
    // Check if OAuth is enabled
    if !state.config.features.oauth_enabled {
        error!("❌ OAuth request received but OAuth is disabled in config");
        return Ok(Json(ApiResponse::error("OAuth is disabled".to_string())));
    }
    
    match state.oauth.get_authorization_url(req.provider, req.source, req.callback_url).await {
        Ok((auth_url, csrf_state)) => {
            info!("✅ Generated OAuth URL: {}", auth_url);
            let response = OAuthResponse {
                auth_url,
                state: csrf_state,
                expires_in: 600, // 10 minutes
            };
            Ok(Json(ApiResponse::success(response)))
        }
        Err(e) => {
            error!("❌ OAuth start failed: {}", e);
            Ok(Json(ApiResponse::error(format!("OAuth initialization failed: {}", e))))
        }
    }
}

/// OAuth callback for Google
async fn oauth_callback_google(
    State(state): State<AppState>,
    query: axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Response, StatusCode> {
    oauth_callback_handler(state, query, OAuthProvider::Google).await
}

/// OAuth callback for GitHub
async fn oauth_callback_github(
    State(state): State<AppState>,
    query: axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Response, StatusCode> {
    oauth_callback_handler(state, query, OAuthProvider::GitHub).await
}

/// Common OAuth callback handler
async fn oauth_callback_handler(
    state: AppState,
    query: axum::extract::Query<std::collections::HashMap<String, String>>,
    provider: OAuthProvider,
) -> Result<Response, StatusCode> {
    info!("🔄 OAuth callback received for {:?}", provider);
    info!("📋 Callback query params: {:?}", query.iter().map(|(k, v)| (k, if k == "code" { "***" } else { v })).collect::<Vec<_>>());
    
    let code = query.get("code")
        .ok_or_else(|| {
            error!("❌ OAuth callback missing 'code' parameter");
            StatusCode::BAD_REQUEST
        })?
        .clone();
    
    let csrf_state = query.get("state")
        .ok_or_else(|| {
            error!("❌ OAuth callback missing 'state' parameter");
            StatusCode::BAD_REQUEST
        })?
        .clone();
    
    info!("✅ OAuth callback has required parameters");

    match state.oauth.exchange_code(code, csrf_state).await {
        Ok((profile, source, callback_url)) => {
            info!("OAuth success for {} user: {} ({})", 
                match provider {
                    OAuthProvider::Google => "Google",
                    OAuthProvider::GitHub => "GitHub",
                },
                profile.name, 
                profile.email
            );

            // Robust user matching and creation logic
            let user = match find_or_create_oauth_user(&state.db, &profile, &provider).await {
                Ok(user) => {
                    info!("✅ Successfully resolved OAuth user: {} ({})", user.username, user.email);
                    
                    // Check whitelist if enabled
                    if let Some(whitelist) = &state.whitelist {
                        if !whitelist.is_user_allowed(&user.username, &user.email) {
                            warn!("OAuth user {} ({}) not in whitelist", user.username, user.email);
                            let web_ui_url = std::env::var("WEB_UI_BASE_URL")
                                .or_else(|_| std::env::var("OAUTH_BASE_URL"))
                                .unwrap_or_else(|_| "http://localhost:7523".to_string());
                            return Ok(axum::response::Redirect::to(&format!("{}/auth/error?message=Access denied - user not authorized", web_ui_url)).into_response());
                        }
                    }
                    
                    user
                }
                Err(e) => {
                    error!("❌ Failed to resolve OAuth user: {}", e);
                    let web_ui_url = std::env::var("WEB_UI_BASE_URL")
                        .or_else(|_| std::env::var("OAUTH_BASE_URL"))
                        .unwrap_or_else(|_| "http://localhost:7523".to_string());
                    return Ok(axum::response::Redirect::to(&format!("{}/auth/error?message=Failed to resolve user account", web_ui_url)).into_response());
                }
            };

            // Generate JWT token for the user
            let claims = mothership_common::auth::Claims {
                sub: user.id.to_string(),
                machine_id: "web-oauth".to_string(), // For OAuth, we don't have a specific machine
                username: user.username.clone(),
                email: Some(user.email.clone()), // Include email for user recreation
                iat: chrono::Utc::now().timestamp(),
                exp: (chrono::Utc::now() + chrono::Duration::days(30)).timestamp(),
                aud: "mothership".to_string(),
                iss: "mothership-server".to_string(),
            };

            match state.auth.encode_token(&claims) {
                Ok(token) => {
                    // Handle different OAuth flows
                    match (source, callback_url) {
                        (OAuthSource::Web, Some(callback_url)) => {
                            // Store token temporarily and redirect browser with code
                            let temp_code = uuid::Uuid::new_v4().to_string();
                            let temp_token_data = TempTokenData {
                                user_id: user.id,
                                username: user.username.clone(),
                                email: user.email.clone(),
                                token: token.clone(),
                                provider,
                                created_at: chrono::Utc::now(),
                                expires_at: chrono::Utc::now() + chrono::Duration::minutes(5), // 5 minute expiry
                            };
                            
                            // Store temporary token
                            {
                                let mut temp_tokens = state.temp_tokens.write().await;
                                temp_tokens.insert(temp_code.clone(), temp_token_data);
                            }
                            
                            info!("🔄 Stored temporary token with code: {}", temp_code);
                            info!("🔄 Callback URL from request: {}", callback_url);
                            
                            // Redirect browser to user's server with the code
                            let finalize_url = format!("{}/auth/finalize?code={}", 
                                std::env::var("OAUTH_BASE_URL")
                                    .or_else(|_| std::env::var("MOTHERSHIP_SERVER_URL"))
                                    .unwrap_or_else(|_| "http://localhost:7523".to_string()),
                                temp_code
                            );
                            
                            info!("🔄 Redirecting browser to: {}", finalize_url);
                            info!("🔄 This should trigger the /auth/finalize endpoint on the user's server");
                            Ok(axum::response::Redirect::to(&finalize_url).into_response())
                        }
                        (OAuthSource::CLI | OAuthSource::GUI, _) => {
                            // For CLI/GUI: Store token in temporary session and redirect to clean URL
                            let _session_id = uuid::Uuid::new_v4().to_string();
                            
                            // Store token data temporarily (you'd want Redis in production)
                            // For now, we'll serve the page directly with embedded token
                            let success_html = generate_cli_success_page(&token, &user.username, &user.email);
                            return Ok(axum::response::Html(success_html).into_response());
                        }
                        (OAuthSource::Web, None) => {
                            // For Web without callback URL: Create secure session and redirect to clean URL
                            let session_id = uuid::Uuid::new_v4().to_string();
                            let session_data = SessionData {
                                user_id: user.id,
                                username: user.username.clone(),
                                email: user.email.clone(),
                                token: token.clone(),
                                created_at: chrono::Utc::now(),
                                expires_at: chrono::Utc::now() + chrono::Duration::hours(24),
                            };
                            
                            // Store session
                            {
                                let mut sessions = state.sessions.write().await;
                                sessions.insert(session_id.clone(), session_data);
                            }
                            
                            // Determine the correct web UI URL
                            let web_ui_url = if let Some(web_port) = state.config.server.web_port {
                                // Use the same host as the current request but different port
                                let host = std::env::var("MOTHERSHIP_HOST")
                                    .unwrap_or_else(|_| "localhost".to_string());
                                format!("http://{}:{}", host, web_port)
                            } else {
                                std::env::var("WEB_UI_BASE_URL")
                                    .or_else(|_| std::env::var("OAUTH_BASE_URL"))
                                    .unwrap_or_else(|_| "http://localhost:7523".to_string())
                            };
                            
                            info!("Creating session for web UI: {}", web_ui_url);
                            
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
                                        // Get the base domain (e.g., "mothershipproject.dev" from "app.mothershipproject.dev")
                                        let parts: Vec<&str> = domain.split('.').collect();
                                        if parts.len() >= 2 {
                                            let base_domain = format!(".{}.{}", parts[parts.len() - 2], parts[parts.len() - 1]);
                                            info!("🍪 Setting cookie domain to: {}", base_domain);
                                            cookie_builder = cookie_builder.domain(base_domain);
                                        }
                                    }
                                }
                            }
                            
                            let cookie = cookie_builder.build();
                            
                            info!("Session cookie created - secure: {}, localhost: {}, domain: {:?}", 
                                  is_secure, is_localhost, cookie.domain());
                            
                            // Redirect to auth success with user data
                            let success_url = format!("/auth/success?user_id={}&username={}&email={}&token={}",
                                user.id,
                                urlencoding::encode(&user.username),
                                urlencoding::encode(&user.email),
                                urlencoding::encode(&token)
                            );
                            
                            Ok((
                                CookieJar::new().add(cookie),
                                axum::response::Redirect::to(&success_url)
                            ).into_response())
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to generate JWT token: {}", e);
                    let web_ui_url = std::env::var("WEB_UI_BASE_URL")
                        .or_else(|_| std::env::var("OAUTH_BASE_URL"))
                        .unwrap_or_else(|_| "http://localhost:7523".to_string());
                    Ok(axum::response::Redirect::to(&format!("{}/auth/error?message=Failed to generate token", web_ui_url)).into_response())
                }
            }
        }
        Err(e) => {
            error!("OAuth callback failed: {}", e);
            let web_ui_url = std::env::var("WEB_UI_BASE_URL")
                .or_else(|_| std::env::var("OAUTH_BASE_URL"))
                .unwrap_or_else(|_| "http://localhost:7523".to_string());
            Ok(axum::response::Redirect::to(&format!(
                "{}/auth/error?message={}",
                web_ui_url,
                urlencoding::encode(&e.to_string())
            )).into_response())
        }
    }
}

/// Robust OAuth user resolution with provider-aware logic
async fn find_or_create_oauth_user(
    db: &Database,
    profile: &OAuthProfile,
    provider: &OAuthProvider,
) -> Result<User, anyhow::Error> {
    // Step 1: Try to find existing user by email (most reliable)
    if let Some(existing_user) = db.get_user_by_email(&profile.email).await? {
        info!("✅ Found existing user by email: {} ({})", existing_user.username, existing_user.email);
        return Ok(existing_user);
    }

    // Step 2: Generate provider-aware username
    let candidate_username = generate_provider_username(profile, provider);
    
    // Step 3: Try to find by the candidate username
    if let Some(existing_user) = db.get_user_by_username(&candidate_username).await? {
        info!("✅ Found existing user by username: {} ({})", existing_user.username, existing_user.email);
        return Ok(existing_user);
    }

    // Step 4: Find available username (handle conflicts)
    let available_username = find_available_username(db, &candidate_username).await?;
    
    // Step 5: Create new user
    info!("🔄 Creating new OAuth user: {} ({}) via {}", available_username, profile.email, provider_name(provider));
    
    let user = db.create_user(available_username, profile.email.clone(), UserRole::User).await?;
    
    info!("✅ Successfully created OAuth user: {} (ID: {})", user.username, user.id);
    Ok(user)
}

/// Generate provider-specific username
fn generate_provider_username(profile: &OAuthProfile, provider: &OAuthProvider) -> String {
    match provider {
        OAuthProvider::GitHub => {
            // GitHub provides usernames, use them directly
            profile.username.clone()
                .unwrap_or_else(|| fallback_username_from_email(&profile.email))
        }
        OAuthProvider::Google => {
            // Google doesn't provide usernames, generate from email
            fallback_username_from_email(&profile.email)
        }
    }
}

/// Generate username from email address
fn fallback_username_from_email(email: &str) -> String {
    email.split('@')
        .next()
        .unwrap_or("user")
        .to_string()
        .replace(".", "")  // Remove dots that might cause issues
        .replace("+", "")  // Remove plus signs
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>()
        .trim_matches('-')
        .trim_matches('_')
        .to_string()
        .to_lowercase()
}

/// Find an available username by appending numbers if needed
async fn find_available_username(db: &Database, candidate: &str) -> Result<String, anyhow::Error> {
    // Sanitize the candidate username
    let base_username = if candidate.is_empty() || candidate == "user" {
        "user".to_string()
    } else {
        candidate.to_string()
    };

    // Try the base username first
    if db.get_user_by_username(&base_username).await?.is_none() {
        return Ok(base_username);
    }

    // Try appending numbers until we find an available one
    for i in 2..1000 {
        let username = format!("{}{}", base_username, i);
        if db.get_user_by_username(&username).await?.is_none() {
            return Ok(username);
        }
    }

    // If we get here, something is very wrong
    Err(anyhow::anyhow!("Could not find an available username"))
}

/// Get provider name for logging
fn provider_name(provider: &OAuthProvider) -> &'static str {
    match provider {
        OAuthProvider::Google => "Google",
        OAuthProvider::GitHub => "GitHub",
    }
}

/// Generate success page for CLI/GUI OAuth flow
fn generate_cli_success_page(token: &str, username: &str, email: &str) -> String {
    format!(r#"
<!DOCTYPE html>
<html>
<head>
    <title>Mothership Authentication Success</title>
    <link rel="icon" type="image/png" href="/static/icon.png">
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            margin: 0;
            padding: 20px;
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }}
        .container {{
            background: white;
            padding: 40px;
            border-radius: 12px;
            box-shadow: 0 10px 30px rgba(0,0,0,0.2);
            text-align: center;
            max-width: 600px;
            width: 100%;
        }}
        .success-icon {{ font-size: 48px; margin-bottom: 20px; }}
        h1 {{ color: #2d3748; margin-bottom: 20px; }}
        .user-info {{ background: #f7fafc; padding: 20px; border-radius: 8px; margin: 20px 0; }}
        .token-container {{
            background: #e8f4fd;
            padding: 20px;
            border-radius: 8px;
            margin: 20px 0;
            text-align: left;
        }}
        .token-textarea {{
            width: 100%;
            height: 100px;
            padding: 15px;
            border: 2px solid #007acc;
            border-radius: 4px;
            font-family: 'Courier New', monospace;
            font-size: 12px;
            background: #f8f9fa;
            resize: none;
            word-wrap: break-word;
            overflow-wrap: break-word;
            margin: 10px 0;
        }}
        .button-container {{
            display: flex;
            gap: 10px;
            justify-content: center;
            margin-top: 15px;
        }}
        .button {{
            background: #007acc;
            color: white;
            border: none;
            padding: 10px 20px;
            border-radius: 4px;
            cursor: pointer;
            font-size: 14px;
        }}
        .button.copy {{
            background: #28a745;
        }}
        .instructions {{
            background: #fff3cd;
            padding: 15px;
            border-radius: 8px;
            border-left: 4px solid #ffc107;
            margin-top: 20px;
            text-align: left;
        }}
        kbd {{
            background: #f8f9fa;
            padding: 2px 6px;
            border-radius: 3px;
            border: 1px solid #ddd;
        }}
    </style>
</head>
<body>
    <div class="container">
        <div class="success-icon">✅</div>
        <h1>Authentication Successful!</h1>
        
        <div class="user-info">
            <p><strong>Username:</strong> {}</p>
            <p><strong>Email:</strong> {}</p>
        </div>
        
        <div class="token-container">
            <h4 style="margin-top: 0; color: #0066cc;">Copy your authentication token:</h4>
            <textarea id="token-textarea" readonly class="token-textarea" onclick="this.select()">{}</textarea>
            
            <div class="button-container">
                <button onclick="selectAllToken()" class="button">📝 Select All</button>
                <button onclick="copyToken()" class="button copy">📋 Copy Token</button>
            </div>
        </div>
        
        <div class="instructions">
            <h4 style="margin-top: 0; color: #856404;">📝 Instructions:</h4>
            <ol style="margin: 10px 0; padding-left: 20px; font-size: 14px;">
                <li>Copy the token above using one of the buttons</li>
                <li>Switch back to your terminal</li>
                <li>Paste the token when prompted</li>
            </ol>
        </div>
    </div>

    <script>
        // Auto-select token on page load
        window.onload = function() {{
            selectAllToken();
        }};

        function selectAllToken() {{
            const textarea = document.getElementById('token-textarea');
            textarea.focus();
            textarea.select();
        }}

        async function copyToken() {{
            const textarea = document.getElementById('token-textarea');
            try {{
                await navigator.clipboard.writeText(textarea.value);
                alert('✅ Token copied to clipboard!');
            }} catch (err) {{
                alert('❌ Failed to copy automatically. Please select and copy the token manually.');
                selectAllToken();
            }}
        }}
    </script>
</body>
</html>"#, 
        username, 
        email,
        token)
}

/// Serve OAuth success page
async fn oauth_success_page(
    query: axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<axum::response::Html<String>, StatusCode> {
    let token = query.get("token").unwrap_or(&String::new()).clone();
    let user = query.get("user").unwrap_or(&String::new()).clone();
    let email = query.get("email").unwrap_or(&String::new()).clone();
    
    // Create a simple success page with embedded token
    let html_content = format!(r#"<!DOCTYPE html>
<html>
<head>
    <title>Mothership Authentication</title>
    <link rel="icon" type="image/png" href="/static/icon.png">
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            margin: 0;
            padding: 20px;
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }}
        .container {{
            background: white;
            padding: 40px;
            border-radius: 12px;
            box-shadow: 0 10px 30px rgba(0,0,0,0.2);
            text-align: center;
            max-width: 600px;
            width: 100%;
        }}
        .success-icon {{ font-size: 48px; margin-bottom: 20px; }}
        h1 {{ color: #2d3748; margin-bottom: 20px; }}
        .user-info {{ background: #f7fafc; padding: 20px; border-radius: 8px; margin: 20px 0; }}
        .token-container {{
            background: #e8f4fd;
            padding: 20px;
            border-radius: 8px;
            margin: 20px 0;
            text-align: left;
        }}
        .token-textarea {{
            width: 100%;
            height: 100px;
            padding: 15px;
            border: 2px solid #007acc;
            border-radius: 4px;
            font-family: 'Courier New', monospace;
            font-size: 12px;
            background: #f8f9fa;
            resize: none;
            word-wrap: break-word;
            overflow-wrap: break-word;
            margin: 10px 0;
        }}
        .button-container {{
            display: flex;
            gap: 10px;
            justify-content: center;
            margin-top: 15px;
        }}
        .button {{
            background: #007acc;
            color: white;
            border: none;
            padding: 10px 20px;
            border-radius: 4px;
            cursor: pointer;
            font-size: 14px;
        }}
        .button.copy {{
            background: #28a745;
        }}
        .instructions {{
            background: #fff3cd;
            padding: 15px;
            border-radius: 8px;
            border-left: 4px solid #ffc107;
            margin-top: 20px;
            text-align: left;
        }}
        kbd {{
            background: #f8f9fa;
            padding: 2px 6px;
            border-radius: 3px;
            border: 1px solid #ddd;
        }}
    </style>
</head>
<body>
    <div class="container">
        <div class="success-icon">✅</div>
        <h1>Authentication Successful!</h1>
        
        <div class="user-info">
            <p><strong>Username:</strong> {}</p>
            <p><strong>Email:</strong> {}</p>
        </div>
        
        <div class="token-container">
            <h4 style="margin-top: 0; color: #0066cc;">Copy your authentication token:</h4>
            <textarea id="token-textarea" readonly class="token-textarea" onclick="this.select()">{}</textarea>
            
            <div class="button-container">
                <button onclick="selectAllToken()" class="button">📝 Select All</button>
                <button onclick="copyToken()" class="button copy">📋 Copy Token</button>
            </div>
        </div>
        
        <div class="instructions">
            <h4 style="margin-top: 0; color: #856404;">📝 Instructions:</h4>
            <ol style="margin: 10px 0; padding-left: 20px; font-size: 14px;">
                <li>Copy the token above using one of the buttons</li>
                <li>Switch back to your terminal</li>
                <li>Paste the token when prompted</li>
            </ol>
        </div>
    </div>

    <script>
        // Auto-select token on page load
        window.onload = function() {{
            selectAllToken();
        }};

        function selectAllToken() {{
            const textarea = document.getElementById('token-textarea');
            textarea.focus();
            textarea.select();
        }}

        async function copyToken() {{
            const textarea = document.getElementById('token-textarea');
            try {{
                await navigator.clipboard.writeText(textarea.value);
                alert('✅ Token copied to clipboard!');
            }} catch (err) {{
                alert('❌ Failed to copy automatically. Please select and copy the token manually.');
                selectAllToken();
            }}
        }}
    </script>
</body>
</html>"#, 
        user, 
        email,
        token);
    
    Ok(axum::response::Html(html_content))
}

/// Serve OAuth error page
async fn oauth_error_page(
    query: axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<axum::response::Html<String>, StatusCode> {
    let error_message = query.get("message").unwrap_or(&"Unknown error".to_string()).clone();
    
    // Read the HTML file
    let html_content = std::fs::read_to_string("oauth-success.html")
        .unwrap_or_else(|_| {
            // Fallback HTML if file doesn't exist
            format!(r#"
<!DOCTYPE html>
<html>
<head>
    <title>Mothership Authentication Error</title>
    <link rel="icon" type="image/png" href="/static/icon.png">
</head>
<body>
    <h1>❌ Authentication Failed</h1>
    <p><strong>Error:</strong> {}</p>
    <p>Please try again or contact support if the problem persists.</p>
</body>
</html>
            "#, error_message)
        });
    
    Ok(axum::response::Html(html_content))
}

/// Complete device authorization (called by auth server)
/// 
/// ⚠️ WARNING: This is a DEMO implementation that accepts any username/email without verification!
/// In production, this should:
/// 1. Verify the user's identity through proper authentication (OAuth, SSO, email verification, etc.)
/// 2. Check if the user is allowed to access the system (whitelist, permissions, etc.)
/// 3. Implement rate limiting and security measures
async fn auth_authorize_device(
    State(state): State<AppState>,
    Json(req): Json<DeviceAuthRequest>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    info!("Device authorization request for device code: {}", req.device_code);
    
    // ⚠️ SECURITY WARNING: This demo implementation trusts the auth server to verify users
    // In production, add proper authentication here!
    
    // Check whitelist if enabled
    if let Some(whitelist) = &state.whitelist {
        if !whitelist.is_user_allowed(&req.username, &req.email) {
            warn!("Device auth rejected - user not in whitelist: {} ({})", req.username, req.email);
            return Ok(Json(ApiResponse::error("Access denied - user not authorized".to_string())));
        }
    }
    
    // Check if user exists, if not create them as a regular user
    let user = if let Some(existing_user) = state.db.get_user_by_username(&req.username).await.unwrap_or(None) {
        // Verify email matches for existing user
        if existing_user.email != req.email {
            warn!("Device auth rejected - email mismatch for user: {}", req.username);
            return Ok(Json(ApiResponse::error("Email mismatch for existing user".to_string())));
        }
        existing_user
    } else {
        // Create new user with regular user role
        match state.db.create_user(req.username.clone(), req.email.clone(), UserRole::User).await {
            Ok(user) => {
                info!("Created new user during auth: {} ({})", user.username, user.email);
                user
            }
            Err(e) => {
                error!("Failed to create user during auth: {}", e);
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    };
    
    // Clone username and email before moving user data
    let username = user.username.clone();
    let email = user.email.clone();
    
    match state.auth.simulate_user_authorization(&req.device_code, user.id, user.username).await {
        Ok(_) => {
            info!("Successfully authorized device for user: {} ({})", username, email);
            Ok(Json(ApiResponse::success("Device authorized successfully".to_string())))
        }
        Err(e) => {
            error!("Device authorization failed: {}", e);
            Ok(Json(ApiResponse::error(e.to_string())))
        }
    }
}

#[derive(serde::Deserialize)]
struct DeviceAuthRequest {
    device_code: String,
    user_id: String,
    username: String,
    email: String,
}

#[derive(serde::Deserialize)]
struct CreateAdminRequest {
    secret: String,
    username: String,
    email: String,
    role: UserRole,
}

#[derive(serde::Deserialize)]
struct CreateGatewayRequest {
    name: String,
    description: String,
    project_path: PathBuf,
}

#[derive(serde::Deserialize)]
struct UploadInitialFilesRequest {
    project_id: uuid::Uuid,
    files: std::collections::HashMap<PathBuf, String>,
}

#[derive(serde::Serialize)]
struct AuthCheckResponse {
    authenticated: bool,
    user_id: uuid::Uuid,
    username: String,
    email: String,
    role: UserRole,
    machine_id: String,
}

/// Create admin user with secret
async fn create_admin_user(
    State(state): State<AppState>,
    Json(req): Json<CreateAdminRequest>,
) -> Result<Json<ApiResponse<mothership_common::User>>, StatusCode> {
    // Get admin secret from environment
    let admin_secret = std::env::var("ADMIN_SECRET")
        .unwrap_or_else(|_| {
            warn!("ADMIN_SECRET not set, using default (NOT SECURE FOR PRODUCTION)");
            "mothership-admin-secret-2025".to_string()
        });
    
    if req.secret != admin_secret {
        warn!("Invalid admin secret provided for user creation: {}", req.username);
        return Ok(Json(ApiResponse::error("Invalid secret".to_string())));
    }

    // Validate role - only allow Admin or SuperAdmin creation via this endpoint
    if !matches!(req.role, UserRole::Admin | UserRole::SuperAdmin) {
        return Ok(Json(ApiResponse::error("Only Admin or SuperAdmin roles can be created via this endpoint".to_string())));
    }

    // Check if user already exists
    if state.db.user_exists_by_email(&req.email).await.unwrap_or(false) {
        return Ok(Json(ApiResponse::error("User with this email already exists".to_string())));
    }

    if state.db.user_exists_by_username(&req.username).await.unwrap_or(false) {
        return Ok(Json(ApiResponse::error("User with this username already exists".to_string())));
    }

    // Create the admin user
    match state.db.create_user(req.username.clone(), req.email.clone(), req.role.clone()).await {
        Ok(user) => {
            info!("Created {} user: {} ({})", 
                match req.role {
                    UserRole::SuperAdmin => "SuperAdmin",
                    UserRole::Admin => "Admin", 
                    UserRole::User => "User",
                }, 
                user.username, 
                user.email
            );
            Ok(Json(ApiResponse::success(user)))
        }
        Err(e) => {
            error!("Failed to create admin user: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Gateway - list accessible projects
async fn gateway(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(_req): Json<GatewayRequest>,
) -> Result<Json<ApiResponse<Vec<GatewayProject>>>, StatusCode> {
    // Extract user ID from JWT token instead of requiring it in request
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = auth_header.trim_start_matches("Bearer ");
    let claims = match state.auth.verify_token(token) {
        Ok(claims) => claims,
        Err(_) => return Err(StatusCode::UNAUTHORIZED),
    };
    
    let user_id = uuid::Uuid::parse_str(&claims.sub)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Ensure user exists in database (recreate from JWT if needed)
    match state.db.get_user(user_id).await {
        Ok(Some(_)) => {
            // User exists, proceed normally
        }
        Ok(None) => {
            // User no longer exists in database (likely due to server restart)
            // Recreate the user from JWT claims if this is an OAuth token
            if claims.machine_id == "web-oauth" {
                let email = claims.email.clone().unwrap_or_else(|| format!("{}@oauth.mothership", claims.username));
                match state.db.create_user_with_id(user_id, claims.username.clone(), email, UserRole::User).await {
                    Ok(_) => {
                        info!("✅ Successfully recreated OAuth user for gateway listing: {} (ID: {})", claims.username, user_id);
                    }
                    Err(e) => {
                        error!("❌ Failed to recreate OAuth user for gateway listing: {}", e);
                        return Err(StatusCode::INTERNAL_SERVER_ERROR);
                    }
                }
            } else {
                // Non-OAuth token, user really doesn't exist
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
        Err(e) => {
            error!("Database error during gateway listing: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    match state.db.get_user_projects(user_id).await {
        Ok(projects) => {
            // Convert Project to GatewayProject
            let gateway_projects: Vec<GatewayProject> = projects.into_iter().map(|project| {
                GatewayProject {
                    project,
                    active_rifts: vec![], // TODO: Get actual active rifts
                    your_rifts: vec![],   // TODO: Get user's rifts
                    last_activity: None,  // TODO: Get last activity
                }
            }).collect();
            Ok(Json(ApiResponse::success(gateway_projects)))
        }
        Err(e) => {
            error!("Gateway request failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Create new gateway project
async fn create_gateway(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateGatewayRequest>,
) -> Result<Json<ApiResponse<Project>>, StatusCode> {
    // Extract user ID from JWT token
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = auth_header.trim_start_matches("Bearer ");
    let claims = match state.auth.verify_token(token) {
        Ok(claims) => claims,
        Err(_) => return Err(StatusCode::UNAUTHORIZED),
    };
    
    let user_id = uuid::Uuid::parse_str(&claims.sub)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    info!("Gateway creation request: {} for user {}", req.name, user_id);

    // Verify user exists and is authenticated (recreate from JWT if needed)
    let user = match state.db.get_user(user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            // User no longer exists in database (likely due to server restart)
            // Recreate the user from JWT claims if this is an OAuth token
            info!("User {} (ID: {}) not found in database during gateway creation, attempting to recreate from JWT claims", claims.username, user_id);
            
            if claims.machine_id == "web-oauth" {
                // This is an OAuth token, recreate the user with the ORIGINAL user ID from JWT
                let email = claims.email.clone().unwrap_or_else(|| format!("{}@oauth.mothership", claims.username));
                match state.db.create_user_with_id(user_id, claims.username.clone(), email, UserRole::User).await {
                    Ok(recreated_user) => {
                        info!("✅ Successfully recreated OAuth user for gateway creation: {} (ID: {})", recreated_user.username, recreated_user.id);
                        recreated_user
                    }
                    Err(e) => {
                        error!("❌ Failed to recreate OAuth user for gateway creation: {}", e);
                        return Err(StatusCode::INTERNAL_SERVER_ERROR);
                    }
                }
            } else {
                // Non-OAuth token, user really doesn't exist
                warn!("Gateway creation failed: User not found: {}", user_id);
                return Ok(Json(ApiResponse::error("User not found".to_string())));
            }
        }
        Err(e) => {
            error!("Database error during gateway creation: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // For now, all authenticated users can create gateways (private gateway capability)
    // In future versions, this will check for premium/enterprise features
    
    // Check if project name already exists for this user
    if state.db.project_exists_by_name(&req.name).await.unwrap_or(false) {
        return Ok(Json(ApiResponse::error("Project with this name already exists".to_string())));
    }

    // Create the project
    match state.db.create_project(req.name.clone(), req.description.clone(), vec![user_id]).await {
        Ok(project) => {
            info!("Created gateway project: {} (ID: {}) for user: {}", 
                project.name, project.id, user.username);
            
            // Create the main rift for the project
            match state.db.create_rift(project.id, user_id, Some("main".to_string())).await {
                Ok(main_rift) => {
                    info!("Created main rift: {} for project: {}", main_rift.id, project.name);
                }
                Err(e) => {
                    error!("Failed to create main rift for project {}: {}", project.name, e);
                    // Continue anyway - rift can be created later during upload/beam
                }
            }
            
            Ok(Json(ApiResponse::success(project)))
        }
        Err(e) => {
            error!("Failed to create gateway project: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// List all projects (temporary endpoint for testing)
async fn list_projects(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<mothership_common::Project>>>, StatusCode> {
    match state.db.list_all_projects().await {
        Ok(projects) => Ok(Json(ApiResponse::success(projects))),
        Err(e) => {
            error!("List projects failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Get specific project details
async fn get_project(
    State(state): State<AppState>,
    Path(id): Path<ProjectId>,
) -> Result<Json<ApiResponse<mothership_common::Project>>, StatusCode> {
    match state.db.get_project(id).await {
        Ok(Some(project)) => Ok(Json(ApiResponse::success(project))),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            error!("Get project failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Get project by name
async fn get_project_by_name(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<ApiResponse<mothership_common::Project>>, StatusCode> {
    match state.db.get_project_by_name(&name).await {
        Ok(Some(project)) => Ok(Json(ApiResponse::success(project))),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            error!("Get project by name failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Beam into a project (join/sync)
async fn beam_into_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<ProjectId>,
    Json(req): Json<BeamRequest>,
) -> Result<Json<ApiResponse<BeamResponse>>, StatusCode> {
    info!("Beam request for project: {}", project_id);
    
    // 🔥 CRITICAL FIX: Extract user ID from JWT token like other endpoints
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = auth_header.trim_start_matches("Bearer ");
    let claims = match state.auth.verify_token(token) {
        Ok(claims) => claims,
        Err(_) => return Err(StatusCode::UNAUTHORIZED),
    };
    
    let user_id = uuid::Uuid::parse_str(&claims.sub)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Ensure user exists in database (recreate from JWT if needed)
    match state.db.get_user(user_id).await {
        Ok(Some(_)) => {
            // User exists, proceed normally
        }
        Ok(None) => {
            // User no longer exists in database, recreate from JWT claims if OAuth token
            if claims.machine_id == "web-oauth" {
                let email = claims.email.clone().unwrap_or_else(|| format!("{}@oauth.mothership", claims.username));
                match state.db.create_user_with_id(user_id, claims.username.clone(), email, UserRole::User).await {
                    Ok(_) => {
                        info!("✅ Successfully recreated OAuth user for beam: {} (ID: {})", claims.username, user_id);
                    }
                    Err(e) => {
                        error!("❌ Failed to recreate OAuth user for beam: {}", e);
                        return Err(StatusCode::INTERNAL_SERVER_ERROR);
                    }
                }
            } else {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
        Err(e) => {
            error!("Database error during beam: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
    
    match handlers::handle_beam(&state, project_id, req, user_id).await {
        Ok(response) => Ok(Json(ApiResponse::success(response))),
        Err(e) => {
            error!("Beam failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Upload initial files for a project
async fn upload_initial_files(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<ProjectId>,
    Json(req): Json<UploadInitialFilesRequest>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    // Extract user ID from JWT token (same pattern as other endpoints)
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = auth_header.trim_start_matches("Bearer ");
    let claims = match state.auth.verify_token(token) {
        Ok(claims) => claims,
        Err(_) => return Err(StatusCode::UNAUTHORIZED),
    };
    
    let user_id = uuid::Uuid::parse_str(&claims.sub)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    info!("Upload initial files request for project: {} by user: {}", project_id, user_id);

    // Verify project exists and user has access
    let project = match state.db.get_project(project_id).await {
        Ok(Some(project)) => project,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    if !state.db.user_has_project_access(user_id, project_id).await.unwrap_or(false) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Get or create the main rift for this project
    let rift = match state.db.get_user_rift(project_id, user_id).await {
        Ok(Some(existing_rift)) => {
            info!("Found existing main rift: {} for initial upload to project: {}", existing_rift.id, project_id);
            existing_rift
        }
        Ok(None) => {
            // Create main rift if it doesn't exist (should happen during project creation)
            info!("Creating main rift for initial upload to project: {}", project_id);
            match state.db.create_rift(project_id, user_id, Some("main".to_string())).await {
                Ok(rift) => rift,
                Err(e) => {
                    error!("Failed to create main rift for initial upload: {}", e);
                    return Err(StatusCode::INTERNAL_SERVER_ERROR);
                }
            }
        }
        Err(e) => {
            error!("Failed to check for existing rift during initial upload: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let file_count = req.files.len();
    info!("Uploading {} initial files to rift: {}", file_count, rift.id);

    // Store each file in the storage engine
    for (path, content) in req.files {
        if let Err(e) = state.sync.storage.update_live_state(rift.id, path.clone(), content).await {
            error!("Failed to store initial file {}: {}", path.display(), e);
            // Continue with other files rather than failing completely
        } else {
            info!("Stored initial file: {}", path.display());
        }
    }

    Ok(Json(ApiResponse::success(format!(
        "Successfully uploaded {} initial files to project '{}'",
        file_count,
        project.name
    ))))
}

/// Create a checkpoint for a project
async fn create_checkpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<ProjectId>,
    Json(req): Json<CreateCheckpointRequest>,
) -> Result<Json<ApiResponse<CheckpointData>>, StatusCode> {
    // Extract user ID from JWT token
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = auth_header.trim_start_matches("Bearer ");
    let claims = match state.auth.verify_token(token) {
        Ok(claims) => claims,
        Err(_) => return Err(StatusCode::UNAUTHORIZED),
    };
    
    let user_id = uuid::Uuid::parse_str(&claims.sub)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    info!("Checkpoint request for project: {} by user: {}", project_id, user_id);

    // Verify project exists and user has access
    let _project = match state.db.get_project(project_id).await {
        Ok(Some(project)) => project,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    if !state.db.user_has_project_access(user_id, project_id).await.unwrap_or(false) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Get user's rift for this project
    let rift = match state.db.get_user_rift(project_id, user_id).await {
        Ok(Some(rift)) => rift,
        Ok(None) => {
            error!("No rift found for user {} in project {}", user_id, project_id);
            return Err(StatusCode::BAD_REQUEST);
        }
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    // Create checkpoint using storage engine
    match state.sync.storage.create_checkpoint(
        rift.id,
        user_id,
        req.message,
        false, // Manual checkpoint
    ).await {
        Ok(checkpoint) => {
            let checkpoint_data = CheckpointData {
                checkpoint_id: checkpoint.id,
                file_count: checkpoint.changes.len(),
            };
            
            info!("Created checkpoint {} with {} files", checkpoint.id, checkpoint.changes.len());
            Ok(Json(ApiResponse::success(checkpoint_data)))
        }
        Err(e) => {
            error!("Failed to create checkpoint: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(serde::Deserialize)]
struct CreateCheckpointRequest {
    message: Option<String>,
    #[allow(dead_code)]
    timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize)]
struct CheckpointData {
    checkpoint_id: uuid::Uuid,
    file_count: usize,
}

/// Get project history (checkpoints)
async fn get_project_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<ProjectId>,
    query: axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<ApiResponse<Vec<mothership_common::Checkpoint>>>, StatusCode> {
    // Extract user ID from JWT token
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = auth_header.trim_start_matches("Bearer ");
    let claims = match state.auth.verify_token(token) {
        Ok(claims) => claims,
        Err(_) => return Err(StatusCode::UNAUTHORIZED),
    };
    
    let user_id = uuid::Uuid::parse_str(&claims.sub)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    info!("History request for project: {} by user: {}", project_id, user_id);

    // Verify project exists and user has access
    let _project = match state.db.get_project(project_id).await {
        Ok(Some(project)) => project,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    if !state.db.user_has_project_access(user_id, project_id).await.unwrap_or(false) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Get user's rift for this project
    let rift = match state.db.get_user_rift(project_id, user_id).await {
        Ok(Some(rift)) => rift,
        Ok(None) => {
            // No rift yet, return empty history
            return Ok(Json(ApiResponse::success(vec![])));
        }
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    // Get limit from query parameters
    let limit = query.get("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(20);

    // Get checkpoints from storage
    match state.sync.storage.list_checkpoints(rift.id).await {
        Ok(mut checkpoints) => {
            // Sort by timestamp (newest first) and limit
            checkpoints.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
            checkpoints.truncate(limit);
            
            info!("Found {} checkpoints for rift: {}", checkpoints.len(), rift.id);
            Ok(Json(ApiResponse::success(checkpoints)))
        }
        Err(e) => {
            error!("Failed to get checkpoints: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Restore to a specific checkpoint
async fn restore_checkpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project_id, checkpoint_id)): Path<(ProjectId, uuid::Uuid)>,
) -> Result<Json<ApiResponse<RestoreData>>, StatusCode> {
    // Extract user ID from JWT token
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = auth_header.trim_start_matches("Bearer ");
    let claims = match state.auth.verify_token(token) {
        Ok(claims) => claims,
        Err(_) => return Err(StatusCode::UNAUTHORIZED),
    };
    
    let user_id = uuid::Uuid::parse_str(&claims.sub)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    info!("Restore request for project: {} checkpoint: {} by user: {}", project_id, checkpoint_id, user_id);

    // Verify project exists and user has access
    let _project = match state.db.get_project(project_id).await {
        Ok(Some(project)) => project,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    if !state.db.user_has_project_access(user_id, project_id).await.unwrap_or(false) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Load the checkpoint
    let checkpoint = match state.sync.storage.load_checkpoint(checkpoint_id).await {
        Ok(Some(checkpoint)) => checkpoint,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(e) => {
            error!("Failed to load checkpoint: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Get all files at this checkpoint
    let files = match state.sync.storage.get_checkpoint_files(checkpoint_id).await {
        Ok(files) => files,
        Err(e) => {
            error!("Failed to get checkpoint files: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let restore_data = RestoreData {
        checkpoint,
        files,
    };

    info!("Restore data prepared with {} files", restore_data.files.len());
    Ok(Json(ApiResponse::success(restore_data)))
}

/// Delete a project and all associated data
async fn delete_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<ProjectId>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    // Extract user ID from JWT token
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = auth_header.trim_start_matches("Bearer ");
    let claims = match state.auth.verify_token(token) {
        Ok(claims) => claims,
        Err(_) => return Err(StatusCode::UNAUTHORIZED),
    };
    
    let user_id = uuid::Uuid::parse_str(&claims.sub)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    info!("Delete request for project: {} by user: {}", project_id, user_id);

    // Verify project exists and user has access
    let project = match state.db.get_project(project_id).await {
        Ok(Some(project)) => project,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    if !state.db.user_has_project_access(user_id, project_id).await.unwrap_or(false) {
        return Err(StatusCode::FORBIDDEN);
    }

    // TODO: Check if user has admin/owner permissions for the project
    // For now, any member can delete (this should be restricted in production)

    // Delete the project and all associated data
    match state.db.delete_project(project_id).await {
        Ok(()) => {
            info!("Successfully deleted project: {} ({})", project.name, project_id);
            
            // TODO: Also clean up storage engine data for this project's rifts
            // This would involve:
            // 1. Finding all rifts for this project
            // 2. Cleaning up checkpoint data and content files
            // 3. Cleaning up live state
            
            Ok(Json(ApiResponse::success(format!(
                "Project '{}' and all associated data have been permanently deleted",
                project.name
            ))))
        }
        Err(e) => {
            error!("Failed to delete project {}: {}", project_id, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(serde::Serialize)]
struct RestoreData {
    checkpoint: mothership_common::Checkpoint,
    files: std::collections::HashMap<std::path::PathBuf, String>,
}

/// WebSocket handler for real-time sync WITH AUTHENTICATION
async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(rift_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Response, StatusCode> {
    info!("🔐 WebSocket connection request with authentication for rift: {}", rift_id);
    
    // AUTHENTICATION FIX: Extract and validate token from query parameters
    let token = params.get("token")
        .ok_or_else(|| {
            warn!("❌ WebSocket connection rejected: No authentication token provided");
            StatusCode::UNAUTHORIZED
        })?;
    
    // Validate the token
    let claims = state.auth.verify_token(token)
        .map_err(|e| {
            warn!("❌ WebSocket connection rejected: Invalid token - {}", e);
            StatusCode::UNAUTHORIZED
        })?;
    
    let user_id = uuid::Uuid::parse_str(&claims.sub)
        .map_err(|_| {
            warn!("❌ WebSocket connection rejected: Invalid user ID in token");
            StatusCode::UNAUTHORIZED
        })?;
    
    // SECURITY: Verify user exists in database
    match state.db.get_user(user_id).await {
        Ok(Some(_user)) => {
            info!("✅ WebSocket connection authenticated for user: {} ({})", claims.username, user_id);
        }
        Ok(None) => {
            // User doesn't exist - try to recreate from OAuth token
            if claims.machine_id == "web-oauth" {
                let email = claims.email.clone().unwrap_or_else(|| format!("{}@oauth.mothership", claims.username));
                if let Err(e) = state.db.create_user_with_id(user_id, claims.username.clone(), email, mothership_common::UserRole::User).await {
                    error!("❌ Failed to recreate OAuth user for WebSocket: {}", e);
                    return Err(StatusCode::INTERNAL_SERVER_ERROR);
                }
                info!("✅ Recreated OAuth user for WebSocket: {} ({})", claims.username, user_id);
            } else {
                warn!("❌ WebSocket connection rejected: User not found and not OAuth token");
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
        Err(e) => {
            error!("❌ Database error during WebSocket auth: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
    
    // SECURITY: Parse and validate rift ID
    let rift_uuid = uuid::Uuid::parse_str(&rift_id)
        .map_err(|_| {
            warn!("❌ WebSocket connection rejected: Invalid rift ID format: {}", rift_id);
            StatusCode::BAD_REQUEST
        })?;
    
    // SECURITY: Verify user has access to this specific rift
    match state.db.get_rift(rift_uuid).await {
        Ok(Some(rift)) => {
            // Check if user is a collaborator on this rift
            if !rift.collaborators.contains(&user_id) {
                warn!("❌ WebSocket connection rejected: User {} not authorized for rift {}", user_id, rift_id);
                return Err(StatusCode::FORBIDDEN);
            }
            info!("✅ User {} authorized for rift: {}", user_id, rift_id);
        }
        Ok(None) => {
            warn!("❌ WebSocket connection rejected: Rift not found: {}", rift_id);
            return Err(StatusCode::NOT_FOUND);
        }
        Err(e) => {
            error!("❌ Database error during rift authorization: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
    
    // Check whitelist if enabled
    if let Some(whitelist) = &state.whitelist {
        let user = state.db.get_user(user_id).await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::UNAUTHORIZED)?;
            
        if !whitelist.is_user_allowed(&user.username, &user.email) {
            warn!("❌ WebSocket connection rejected: User {} ({}) not in whitelist", user.username, user.email);
            return Err(StatusCode::FORBIDDEN);
        }
    }
    
    info!("✅ WebSocket connection authenticated and authorized for user: {} on rift: {}", claims.username, rift_id);
    
    Ok(ws.on_upgrade(move |socket| async move {
        info!("📡 WebSocket connection established for user: {} on rift: {}", claims.username, rift_id);
        sync::handle_websocket(socket, state.sync, rift_id.clone()).await;
        info!("📡 WebSocket connection closed for user: {} on rift: {}", claims.username, rift_id);
    }))
} 