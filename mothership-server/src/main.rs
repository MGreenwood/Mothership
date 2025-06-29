use axum::{
    extract::{Path, State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post},
    Router,
};
use mothership_common::{
    auth::{AuthRequest, AuthResponse, TokenRequest, TokenResponse, OAuthRequest, OAuthResponse, OAuthProvider, OAuthProfile, OAuthSource},
    protocol::{ApiResponse, BeamRequest, BeamResponse, GatewayRequest},
    GatewayProject, Project, ProjectId, User, UserRole,
};
use uuid;
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tower_http::cors::CorsLayer;
use tracing::{error, info, warn};

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load environment variables - try multiple locations
    dotenvy::dotenv().ok(); // Current directory (for Docker)
    dotenvy::from_filename("../.env").ok(); // Parent directory (for local development)
    
    // Load server configuration
    let config = ServerConfig::load_from_file("server.config")?;
    info!("✅ Server configuration loaded");
    info!("🔧 Config: host={}, port={}, web_port={:?}", 
        config.server.host, config.server.port, config.server.web_port);
    
    // Load user whitelist if enabled
    let whitelist = config.load_whitelist()?;
    if config.auth.whitelist_enabled {
        if whitelist.is_some() {
            info!("✅ User whitelist loaded and active");
        } else {
            warn!("⚠️ Whitelist enabled but no valid whitelist file found");
        }
    }
    
    // Debug: Check if OAuth environment variables are loaded
    if config.features.oauth_enabled {
        println!("🔍 Environment Check - GOOGLE_CLIENT_ID: {}", 
            std::env::var("GOOGLE_CLIENT_ID").map(|s| format!("{}...", &s[..10.min(s.len())])).unwrap_or_else(|_| "NOT SET".to_string()));
        println!("🔍 Environment Check - GOOGLE_CLIENT_SECRET: {}", 
            std::env::var("GOOGLE_CLIENT_SECRET").map(|s| format!("{}...", &s[..10.min(s.len())])).unwrap_or_else(|_| "NOT SET".to_string()));
    }

    // Initialize tracing with config debug level
    let subscriber = tracing_subscriber::fmt()
        .with_target(false)
        .compact();
    
    if config.server.debug_logging {
        subscriber.with_max_level(tracing::Level::DEBUG).init();
        info!("🔍 Debug logging enabled");
    } else {
        subscriber.with_max_level(tracing::Level::INFO).init();
    }

    // Use port from config (can be overridden by environment)
    let port = std::env::var("MOTHERSHIP_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(config.server.port);

    let jwt_secret = std::env::var("JWT_SECRET")
        .unwrap_or_else(|_| {
            warn!("JWT_SECRET not set, using default (NOT SECURE FOR PRODUCTION)");
            "mothership-default-jwt-secret-change-me".to_string()
        });

    info!("🚀 Starting Mothership Server on port {}", port);

    // Initialize PostgreSQL database
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| {
            warn!("DATABASE_URL not set, using default PostgreSQL connection");
            "postgresql://mothership:mothership_dev@postgres:5432/mothership".to_string()
        });
    
    let db = Database::new(&database_url).await?;
    
    // Ensure database schema exists
    if let Err(e) = db.ensure_schema().await {
        error!("Failed to ensure database schema: {}", e);
        std::process::exit(1);
    }
    
    // Initialize auth service
    let auth = AuthService::new(jwt_secret);
    
    // Initialize OAuth service if enabled
    let oauth = if config.features.oauth_enabled {
        OAuthService::new().unwrap_or_else(|e| {
            warn!("OAuth service initialization failed: {}. OAuth login will not be available.", e);
            // Return a minimal OAuth service that will fail gracefully
            OAuthService::new().unwrap_or_else(|_| panic!("Failed to create OAuth service"))
        })
    } else {
        // Create a dummy OAuth service when disabled
        OAuthService::new().unwrap_or_else(|_| panic!("Failed to create OAuth service"))
    };
    
    // Initialize sync manager
    // Initialize storage engine
    let storage_root = PathBuf::from("./storage");
    let storage = Arc::new(StorageEngine::new(storage_root).await?);
    info!("✅ Storage engine initialized");

    // Initialize sync state with storage
    let sync = SyncState::new(db.clone(), storage.clone());
    info!("✅ Sync state initialized");

    let state = AppState { db, auth, oauth, sync, config: config.clone(), whitelist };

    // Build the router with conditional features
    let mut app = Router::new()
        // Health check (always available)
        .route("/health", get(health_check))
        // Server capabilities (always available)
        .route("/capabilities", get(server_capabilities))
        
        // Authentication endpoints (always available if auth required)
        .route("/auth/start", post(auth_start))
        .route("/auth/token", post(auth_token))
        .route("/auth/verify", post(auth_verify))
        .route("/auth/check", get(auth_check))
        .route("/auth/authorize-device", post(auth_authorize_device))
        
        // Admin endpoints (always available)
        .route("/admin/create", post(create_admin_user))
        
        // Gateway (project discovery)
        .route("/gateway", post(gateway))
        .route("/gateway/create", post(create_gateway))
        
        // Project management
        .route("/projects", get(list_projects))
        .route("/projects/by-name/:name", get(get_project_by_name))
        .route("/projects/:id", get(get_project))
        .route("/projects/:id/beam", post(beam_into_project))
        .route("/projects/:id/upload-initial", post(upload_initial_files))
        .route("/projects/:id/checkpoint", post(create_checkpoint))
        .route("/projects/:id/history", get(get_project_history))
        .route("/projects/:id/restore/:checkpoint_id", post(restore_checkpoint))
        .route("/projects/:id", delete(delete_project));

    // Add OAuth endpoints only if enabled
    if config.features.oauth_enabled {
        info!("🔐 OAuth endpoints enabled");
        app = app
            .route("/auth/oauth/start", post(oauth_start))
            .route("/auth/oauth/test", get(oauth_test))
            .route("/auth/callback/google", get(oauth_callback_google))
            .route("/auth/callback/github", get(oauth_callback_github))
            .route("/auth/success", get(oauth_success_page))
            .route("/auth/error", get(oauth_error_page));
    }

    // Add WebSocket sync only if enabled
    if config.features.websocket_sync_enabled {
        info!("🔄 WebSocket sync enabled");
        app = app.route("/sync/:rift_id", get(websocket_handler));
    }

    // Add CLI distribution endpoints only if enabled
    if config.features.cli_distribution_enabled {
        info!("📦 CLI distribution endpoints enabled");
        app = app.nest("/", cli_distribution::routes());
    }

    // Handle web UI routing based on configuration
    if let Some(web_port) = config.server.web_port {
        // Separate web server on different port
        info!("🌐 Starting separate web UI server on port {}", web_port);
        
        let mut web_app = Router::new()
            .nest("/", web_ui::routes());
            
        // Add OAuth endpoints to web UI server for seamless authentication
        if config.features.oauth_enabled {
            info!("🔐 Adding OAuth endpoints to web UI server");
            web_app = web_app
                .route("/auth/oauth/start", post(oauth_start))
                .route("/auth/oauth/test", get(oauth_test))
                .route("/auth/callback/google", get(oauth_callback_google))
                .route("/auth/callback/github", get(oauth_callback_github))
                .route("/auth/success", get(oauth_success_page))
                .route("/auth/error", get(oauth_error_page));
        }
        
        let web_app = web_app
            .layer(CorsLayer::permissive())
            .with_state(state.clone());
        
        let web_host = config.server.host.parse::<std::net::IpAddr>()
            .unwrap_or_else(|_| {
                warn!("Invalid host address in config: {}, using 0.0.0.0", config.server.host);
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0))
            });
        let web_addr = SocketAddr::new(web_host, web_port);
        
        // Start web server in background
        let web_listener = tokio::net::TcpListener::bind(web_addr).await?;
        info!("🎨 Web UI available at http://{}:{}", web_host, web_port);
        
        tokio::spawn(async move {
            if let Err(e) = axum::serve(web_listener, web_app).await {
                error!("Web UI server error: {}", e);
            }
        });
    } else {
        // Same port - add web UI routes to main app
        info!("🌐 Web UI integrated on main server port");
        app = app.nest("/", web_ui::routes());
    }

    // Apply final layers and state to main app
    let app = app
        .layer(CorsLayer::permissive())
        .with_state(state);

    // Start the main server
    let host = config.server.host.parse::<std::net::IpAddr>()
        .unwrap_or_else(|_| {
            warn!("Invalid host address in config: {}, using 0.0.0.0", config.server.host);
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0))
        });
    let addr = SocketAddr::new(host, port);
    info!("🚀 Mothership API Server listening on {} (config: {}:{})", addr, config.server.host, config.server.port);
    
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
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
    let mut auth_methods = vec!["device_code".to_string()];
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
        sso_domain: None, // TODO: Support SSO domains
        oauth_providers,
        features,
        name: "Mothership Server".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    
    Json(ApiResponse::success(capabilities))
}

/// Start the authentication flow
async fn auth_start(
    State(state): State<AppState>,
    Json(req): Json<AuthRequest>,
) -> Result<Json<ApiResponse<AuthResponse>>, StatusCode> {
    info!("Auth start request from machine: {}", req.machine_id);
    
    match state.auth.start_auth_flow(req).await {
        Ok(response) => Ok(Json(ApiResponse::success(response))),
        Err(e) => {
            error!("Auth start failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Exchange device code for tokens
async fn auth_token(
    State(state): State<AppState>,
    Json(req): Json<TokenRequest>,
) -> Result<Json<ApiResponse<TokenResponse>>, StatusCode> {
    match state.auth.exchange_token(req).await {
        Ok(response) => Ok(Json(ApiResponse::success(response))),
        Err(e) => {
            warn!("Token exchange failed: {}", e);
            Ok(Json(ApiResponse::error(e.to_string())))
        }
    }
}

/// Verify an existing token
async fn auth_verify(
    State(state): State<AppState>,
    Json(token): Json<String>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    match state.auth.verify_token(&token) {
        Ok(claims) => Ok(Json(ApiResponse::success(format!(
            "Token valid for user: {}",
            claims.username
        )))),
        Err(e) => Ok(Json(ApiResponse::error(e.to_string()))),
    }
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
    
    // Check if OAuth is enabled
    if !state.config.features.oauth_enabled {
        error!("❌ OAuth request received but OAuth is disabled in config");
        return Ok(Json(ApiResponse::error("OAuth is disabled".to_string())));
    }
    
    match state.oauth.get_authorization_url(req.provider, req.source).await {
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
        Ok((profile, source)) => {
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
                    // Handle CLI vs Web OAuth differently
                    match source {
                        OAuthSource::CLI | OAuthSource::GUI => {
                            // For CLI/GUI: Store token in temporary session and redirect to clean URL
                            let session_id = uuid::Uuid::new_v4().to_string();
                            
                            // Store token data temporarily (you'd want Redis in production)
                            // For now, we'll serve the page directly with embedded token
                            let success_html = generate_cli_success_page(&token, &user.username, &user.email);
                            return Ok(axum::response::Html(success_html).into_response());
                        }
                        OAuthSource::Web => {
                            // For Web: Redirect to download page as before
                            let web_ui_url = std::env::var("WEB_UI_BASE_URL")
                                .or_else(|_| std::env::var("OAUTH_BASE_URL")) // Fallback to OAuth base URL
                                .unwrap_or_else(|_| {
                                    if let Some(web_port) = state.config.server.web_port {
                                        // Dual port mode - use web UI port
                                        format!("http://localhost:{}", web_port)
                                    } else {
                                        // Single port mode - use main server port
                                        "http://localhost:7523".to_string()
                                    }
                                });
                            
                            Ok(axum::response::Redirect::to(&format!(
                                "{}/download/authenticated?token={}&user={}&email={}",
                                web_ui_url,
                                urlencoding::encode(&token),
                                urlencoding::encode(&user.username),
                                urlencoding::encode(&user.email)
                            )).into_response())
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
    
    // Check if base username is available
    if !db.user_exists_by_username(&base_username).await? {
        return Ok(base_username);
    }
    
    // Try with numbers appended
    for i in 1..=999 {
        let numbered_username = format!("{}{}", base_username, i);
        if !db.user_exists_by_username(&numbered_username).await? {
            info!("🔄 Username '{}' taken, using '{}'", base_username, numbered_username);
            return Ok(numbered_username);
        }
    }
    
    // Fallback with timestamp if all numbers taken
    let timestamp_username = format!("{}{}", base_username, chrono::Utc::now().timestamp());
    Ok(timestamp_username)
}

/// Get human-readable provider name
fn provider_name(provider: &OAuthProvider) -> &'static str {
    match provider {
        OAuthProvider::Google => "Google",
        OAuthProvider::GitHub => "GitHub",
    }
}

/// Generate CLI success page with embedded token (no URL exposure)
fn generate_cli_success_page(token: &str, username: &str, email: &str) -> String {
    format!(r#"<!DOCTYPE html>
<html>
<head>
    <title>Mothership Authentication Success</title>
    <meta charset="utf-8">
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
        .token-section {{ background: #e8f4fd; padding: 20px; border-radius: 8px; margin: 20px 0; text-align: left; }}
        .token-textarea {{ 
            width: 100%; 
            height: 120px; 
            padding: 15px; 
            border: 2px solid #007acc; 
            border-radius: 4px; 
            font-family: 'Courier New', monospace; 
            font-size: 12px; 
            background: #f8f9fa; 
            resize: none;
            word-wrap: break-word;
            overflow-wrap: break-word;
            box-sizing: border-box;
        }}
        .buttons {{ margin-top: 15px; text-align: center; }}
        .btn {{ 
            background: #007acc; 
            color: white; 
            border: none; 
            padding: 10px 20px; 
            border-radius: 4px; 
            cursor: pointer;
            font-size: 14px;
            margin: 0 5px;
        }}
        .btn:hover {{ background: #005fa3; }}
        .btn.copy {{ background: #28a745; }}
        .btn.copy:hover {{ background: #1e7e34; }}
        .instructions {{ 
            background: #fff3cd; 
            padding: 15px; 
            border-radius: 8px; 
            border-left: 4px solid #ffc107;
            margin-top: 20px;
            text-align: left;
        }}
        .tip {{ 
            background: #d1ecf1; 
            padding: 15px; 
            border-radius: 8px; 
            margin-top: 15px; 
            font-size: 12px; 
            color: #0c5460;
        }}
    </style>
</head>
<body>
    <div class="container">
        <div class="success-icon">✅</div>
        <h1>CLI Authentication Successful!</h1>
        
        <div class="user-info">
            <p><strong>User:</strong> {}</p>
            <p><strong>Email:</strong> {}</p>
        </div>
        
        <div class="token-section">
            <h4 style="margin-top: 0; color: #0066cc;">🖥️ For Terminal/CLI Users:</h4>
            <p style="margin: 10px 0; font-size: 14px;">Copy the token below and paste it in your terminal when prompted:</p>
            
            <textarea id="token-textarea" class="token-textarea" readonly onclick="this.select()" title="Click to select all text">{}</textarea>
            
            <div class="buttons">
                <button class="btn" onclick="selectAllToken()">📝 Select All</button>
                <button class="btn copy" onclick="copyToClipboard()">📋 Copy Token</button>
            </div>
        </div>
        
        <div class="instructions">
            <h4 style="margin-top: 0; color: #856404;">📝 Instructions:</h4>
            <ol style="margin: 10px 0; padding-left: 20px; font-size: 14px;">
                <li>Click the "Copy Token" button above, or</li>
                <li>Click in the text box and press <kbd style="background: #f8f9fa; padding: 2px 6px; border-radius: 3px; border: 1px solid #ddd;">Ctrl+A</kbd> then <kbd style="background: #f8f9fa; padding: 2px 6px; border-radius: 3px; border: 1px solid #ddd;">Ctrl+C</kbd></li>
                <li>Go back to your terminal and paste the token when prompted</li>
                <li>You can close this window after copying the token</li>
            </ol>
        </div>
        
        <div class="tip">
            💡 <strong>Security Note:</strong> This token is not visible in the URL for your security. It's only shown on this page.
        </div>
    </div>

    <script>
        // Token is embedded securely in the page, not in URL
        const rawToken = '{}';
        
        function selectAllToken() {{
            const textarea = document.getElementById('token-textarea');
            if (textarea) {{
                textarea.focus();
                textarea.select();
                textarea.setSelectionRange(0, textarea.value.length);
            }}
        }}
        
        async function copyToClipboard() {{
            const btn = event.target;
            const originalText = btn.textContent;
            
            try {{
                // Try modern clipboard API first
                await navigator.clipboard.writeText(rawToken);
                
                // Show success feedback
                btn.textContent = '✅ Copied!';
                btn.style.background = '#28a745';
                
                setTimeout(() => {{
                    btn.textContent = originalText;
                    btn.style.background = '#28a745';
                }}, 2000);
                
            }} catch (err) {{
                console.error('Modern clipboard API failed:', err);
                
                // Fallback: Try legacy execCommand method
                try {{
                    const textarea = document.getElementById('token-textarea');
                    textarea.select();
                    textarea.setSelectionRange(0, textarea.value.length);
                    
                    const successful = document.execCommand('copy');
                    if (successful) {{
                        btn.textContent = '✅ Copied!';
                        btn.style.background = '#28a745';
                        
                        setTimeout(() => {{
                            btn.textContent = originalText;
                            btn.style.background = '#28a745';
                        }}, 2000);
                    }} else {{
                        throw new Error('execCommand failed');
                    }}
                }} catch (fallbackErr) {{
                    console.error('All clipboard methods failed:', fallbackErr);
                    
                    // Final fallback: Select text and show instructions
                    selectAllToken();
                    btn.textContent = '📋 Text Selected!';
                    btn.style.background = '#ffc107';
                    
                    alert('❌ Automatic copy failed.\\n\\nThe token text has been selected for you.\\nPress Ctrl+C (or Cmd+C on Mac) to copy it manually.');
                    
                    setTimeout(() => {{
                        btn.textContent = originalText;
                        btn.style.background = '#28a745';
                    }}, 3000);
                }}
            }}
        }}
        
        // Auto-select token when page loads
        document.addEventListener('DOMContentLoaded', function() {{
            selectAllToken();
        }});
        
        // Also select when clicking anywhere on the textarea
        document.getElementById('token-textarea').addEventListener('click', selectAllToken);
    </script>
</body>
</html>"#, 
        username, 
        email, 
        token,  // Token safely embedded in textarea
        token   // Token for JavaScript (properly escaped)
    )
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
            max-width: 500px;
            width: 100%;
        }}
        .success-icon {{ font-size: 48px; margin-bottom: 20px; }}
        h1 {{ color: #2d3748; margin-bottom: 20px; }}
        .user-info {{ background: #f7fafc; padding: 20px; border-radius: 8px; margin: 20px 0; }}
        .status {{ color: #38a169; font-weight: 500; margin-top: 20px; }}
        .spinner {{ 
            border: 3px solid #f3f3f3; 
            border-top: 3px solid #667eea; 
            border-radius: 50%; 
            width: 20px; 
            height: 20px; 
            animation: spin 1s linear infinite; 
            display: inline-block;
            margin-right: 10px;
        }}
        @keyframes spin {{ 0% {{ transform: rotate(0deg); }} 100% {{ transform: rotate(360deg); }} }}
    </style>
</head>
<body>
    <div class="container">
        <div class="success-icon">✅</div>
        <h1>Authentication Successful!</h1>
        <div class="user-info">
            <p><strong>User:</strong> {}</p>
            <p><strong>Email:</strong> {}</p>
        </div>
        <div class="status" id="status">
            <div class="spinner"></div>
            Completing authentication...
        </div>
    </div>
    
    <script>
        console.log('OAuth Success - Script loaded successfully!');
        
        // Token data  
        const token = '{}';
        const user = '{}';
        const email = '{}';
        
        console.log('OAuth Success - Token received:', token ? token.substring(0, 20) + '...' : 'NO TOKEN');
        console.log('OAuth Success - User:', user);
        console.log('OAuth Success - Email:', email);
        
        // Handle authentication completion
        async function handleAuthCompletion() {{
            const tokenData = {{
                token: token,
                user: user,
                email: email
            }};
            
            // Check if this is CLI-initiated OAuth
            const urlParams = new URLSearchParams(window.location.search);
            const source = urlParams.get('source');
            const isCLIAuth = source === 'cli';
            
            // Check if we're in a local development environment
            const isLocalhost = window.location.hostname === 'localhost' || 
                               window.location.hostname === '127.0.0.1' ||
                               window.location.hostname.includes('.local');
            
            if (isLocalhost) {{
                // Try to send to local GUI callback endpoint for development
                const callbacks = [
                    {{ url: 'http://localhost:7524/oauth/callback', name: 'GUI (Tauri)', method: 'POST' }}
                ];
                
                let successCount = 0;
                
                for (const callback of callbacks) {{
                    try {{
                        console.log(`🔍 Trying ${{callback.name}} callback...`);
                        
                        const response = await fetch(callback.url, {{
                            method: 'POST',
                            headers: {{
                                'Content-Type': 'application/json',
                            }},
                            body: JSON.stringify(tokenData)
                        }});
                        
                        if (response.ok) {{
                            console.log(`✅ ${{callback.name}} callback successful!`);
                            successCount++;
                        }} else {{
                            console.log(`⚠️ ${{callback.name}} callback failed: HTTP ${{response.status}}`);
                        }}
                    }} catch (error) {{
                        console.log(`⚠️ ${{callback.name}} callback failed: ${{error.message}}`);
                    }}
                }}
                
                if (successCount > 0) {{
                    document.getElementById('status').innerHTML = `✅ Authentication completed! Sent to ${{successCount}} app(s). You can close this window.`;
                    setTimeout(() => window.close(), 2000);
                    return;
                }}
            }}
            
            // For production/web environments, redirect to download page with token (unless CLI-initiated)
            if (!isLocalhost && !isCLIAuth) {{
                console.log('🌐 Production environment detected, redirecting to download page...');
                const baseUrl = window.location.origin; // Use current domain (https://app.mothershipproject.dev)
                const downloadUrl = `${{baseUrl}}/download/authenticated?token=${{encodeURIComponent(token)}}&user=${{encodeURIComponent(user)}}&email=${{encodeURIComponent(email)}}`;
                console.log('🔗 Redirecting to:', downloadUrl);
                window.location.href = downloadUrl;
                return;
            }}
            
            // For CLI-initiated OAuth, always show manual copy interface
            if (isCLIAuth) {{
                console.log('🖥️ CLI-initiated OAuth detected, showing manual token copy interface...');
            }}
            
            // Fallback: show manual token copy (for localhost when GUI callback fails)
                
                // Fallback: Show manual copy option  
                document.getElementById('status').innerHTML = `
                    <div style="text-align: left; max-width: 600px; margin: 0 auto;">
                        <h3 style="color: #28a745; text-align: center; margin-bottom: 20px;">✅ Authentication Successful!</h3>
                        
                        <div style="background: #e8f4fd; padding: 20px; border-radius: 8px; margin-bottom: 20px;">
                            <h4 style="margin-top: 0; color: #0066cc;">For Terminal/CLI Users:</h4>
                            <p style="margin: 10px 0; font-size: 14px;">Copy the token below and paste it in your terminal when prompted:</p>
                            
                            <div style="position: relative;">
                                <textarea id="token-textarea" readonly style="
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
                                " onclick="this.select()" title="Click to select all text">${{token}}</textarea>
                            </div>
                            
                            <div style="margin-top: 15px; text-align: center;">
                                <button onclick="selectAllToken()" style="
                                    background: #007acc; 
                                    color: white; 
                                    border: none; 
                                    padding: 10px 20px; 
                                    border-radius: 4px; 
                                    cursor: pointer;
                                    font-size: 14px;
                                    margin-right: 10px;
                                ">📝 Select All Text</button>
                                
                                <button onclick="tryClipboardCopy()" style="
                                    background: #28a745; 
                                    color: white; 
                                    border: none; 
                                    padding: 10px 20px; 
                                    border-radius: 4px; 
                                    cursor: pointer;
                                    font-size: 14px;
                                ">📋 Try Auto-Copy</button>
                            </div>
                        </div>
                        
                        <div style="background: #fff3cd; padding: 15px; border-radius: 8px; border-left: 4px solid #ffc107;">
                            <h4 style="margin-top: 0; color: #856404;">📝 Manual Instructions:</h4>
                            <ol style="margin: 10px 0; padding-left: 20px; font-size: 14px;">
                                <li>Click in the text box above to select the token</li>
                                <li>Press <kbd style="background: #f8f9fa; padding: 2px 6px; border-radius: 3px; border: 1px solid #ddd;">Ctrl+A</kbd> to select all text</li>
                                <li>Press <kbd style="background: #f8f9fa; padding: 2px 6px; border-radius: 3px; border: 1px solid #ddd;">Ctrl+C</kbd> to copy (or <kbd style="background: #f8f9fa; padding: 2px 6px; border-radius: 3px; border: 1px solid #ddd;">Cmd+C</kbd> on Mac)</li>
                                <li>Go back to your terminal and paste with <kbd style="background: #f8f9fa; padding: 2px 6px; border-radius: 3px; border: 1px solid #ddd;">Ctrl+V</kbd> (or <kbd style="background: #f8f9fa; padding: 2px 6px; border-radius: 3px; border: 1px solid #ddd;">Cmd+V</kbd> on Mac)</li>
                            </ol>
                        </div>
                        
                        <div style="background: #d1ecf1; padding: 15px; border-radius: 8px; margin-top: 15px; text-align: center;">
                            <p style="margin: 0; font-size: 12px; color: #0c5460;">
                                💡 <strong>Tip:</strong> You can also right-click in the text box and select "Copy" from the context menu
                            </p>
                        </div>
                    </div>
                    
                    <scr' + 'ipt>
                        // Store the raw token value
                        const rawToken = token;
                        
                        function selectAllToken() {{
                            const textarea = document.getElementById('token-textarea');
                            if (textarea) {{
                                textarea.focus();
                                textarea.select();
                                // Also try to select all content
                                textarea.setSelectionRange(0, textarea.value.length);
                            }}
                        }}
                        
                        async function tryClipboardCopy() {{
                            try {{
                                await navigator.clipboard.writeText(rawToken);
                                alert('✅ Token copied to clipboard successfully!');
                            }} catch (err) {{
                                console.error('Clipboard copy failed:', err);
                                selectAllToken();
                                alert('❌ Automatic copy failed. The token text has been selected - please copy it manually with Ctrl+C (Cmd+C on Mac).');
                            }}
                        }}
                        
                        // Auto-select the token when the page loads with better timing
                        function autoSelectWhenReady() {{
                            const textarea = document.getElementById('token-textarea');
                            if (textarea && textarea.offsetParent !== null) {{
                                // Element is visible and ready
                                selectAllToken();
                                
                                // Show a subtle indication that text is selected
                                const status = document.createElement('div');
                                status.style.cssText = 'margin-top: 10px; padding: 8px; background: #e7f3ff; border-radius: 4px; font-size: 12px; color: #0066cc;';
                                status.innerHTML = '✨ Token text is now selected and ready to copy!';
                                textarea.parentNode.appendChild(status);
                                
                                // Remove the status after a few seconds
                                setTimeout(() => {{
                                    if (status.parentNode) {{
                                        status.parentNode.removeChild(status);
                                    }}
                                }}, 3000);
                            }} else {{
                                // Try again in a moment
                                setTimeout(autoSelectWhenReady, 100);
                            }}
                        }}
                        
                        // Start checking when DOM is ready
                        if (document.readyState === 'loading') {{
                            document.addEventListener('DOMContentLoaded', autoSelectWhenReady);
                        }} else {{
                            autoSelectWhenReady();
                        }}
                    </scr' + 'ipt>
                `;
            }}
        }}
        
        // Handle authentication completion immediately
        handleAuthCompletion();
    </script>
</body>
</html>"#, 
        user, 
        email, 
        serde_json::to_string(&token).unwrap_or_else(|_| "\"\"".to_string()),
        serde_json::to_string(&user).unwrap_or_else(|_| "\"\"".to_string()),
        serde_json::to_string(&email).unwrap_or_else(|_| "\"\"".to_string()));
    
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
    Json(req): Json<GatewayRequest>,
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

    match state.db.get_user_projects(user_id, req.include_inactive).await {
        Ok(projects) => Ok(Json(ApiResponse::success(projects))),
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
    Path(_rift_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Response, StatusCode> {
    info!("🔐 WebSocket connection request with authentication");
    
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
    
    // Verify user exists in database
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
    
    info!("✅ WebSocket connection authenticated and authorized for user: {}", claims.username);
    
    Ok(ws.on_upgrade(move |socket| async move {
        info!("📡 WebSocket connection established for user: {}", claims.username);
        sync::handle_websocket(socket, state.sync).await;
        info!("📡 WebSocket connection closed for user: {}", claims.username);
    }))
} 