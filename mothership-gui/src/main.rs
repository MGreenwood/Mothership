// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::fs;
use tauri::{State, Manager, AppHandle};
use serde::{Deserialize, Serialize};
use mothership_common::{auth::{TokenResponse, OAuthRequest, OAuthResponse, OAuthProvider}, GatewayProject};
use std::sync::{Arc, Mutex};
use tauri_plugin_opener::open_url;
use uuid;
use axum::{extract::Json as AxumJson, response::Json as AxumResponseJson, routing::post, Router};
use tower_http::cors::CorsLayer;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileItem {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: Option<u64>,
    pub modified: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorState {
    pub current_file: Option<String>,
    pub vim_mode: bool,
    pub projects: Vec<GatewayProject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredCredentials {
    pub access_token: String,
    pub user_email: Option<String>,
    pub user_name: Option<String>,
    pub stored_at: String,
}

// Application state
#[derive(Clone)]
pub struct AppState {
    pub editor_state: Arc<Mutex<EditorState>>,
    pub auth_token: Arc<Mutex<Option<String>>>,
    pub server_url: String,
    pub app_handle: Option<Arc<Mutex<Option<AppHandle>>>>,
}

// Helper functions for credential storage
fn get_credentials_file_path(_app: &AppHandle) -> Result<PathBuf, String> {
    // Use the SAME path as CLI for credential consistency
    let app_data_dir = dirs::config_dir()
        .ok_or_else(|| "Could not find config directory".to_string())?
        .join("mothership");
    
    println!("🔍 App data directory (matching CLI): {}", app_data_dir.display());
    
    // Ensure the directory exists
    if !app_data_dir.exists() {
        println!("📁 Creating app data directory: {}", app_data_dir.display());
        fs::create_dir_all(&app_data_dir)
            .map_err(|e| format!("Failed to create app data directory: {}", e))?;
    } else {
        println!("✅ App data directory already exists");
    }
    
    let credentials_path = app_data_dir.join("credentials.json");
    println!("🔍 Credentials file path (matching CLI): {}", credentials_path.display());
    
    Ok(credentials_path)
}

fn save_credentials(app: &AppHandle, credentials: &StoredCredentials) -> Result<(), String> {
    let credentials_path = get_credentials_file_path(app)?;
    
    println!("🔍 Attempting to save credentials to: {}", credentials_path.display());
    println!("📝 Credentials being saved: user={:?}, token_length={}", 
             credentials.user_name, credentials.access_token.len());
    
    let credentials_json = serde_json::to_string_pretty(credentials)
        .map_err(|e| format!("Failed to serialize credentials: {}", e))?;
    
    println!("📄 Serialized credentials (first 100 chars): {}", 
             &credentials_json.chars().take(100).collect::<String>());
    
    fs::write(&credentials_path, credentials_json)
        .map_err(|e| format!("Failed to write credentials file: {}", e))?;
    
    // Verify the file was actually written
    if credentials_path.exists() {
        let file_size = fs::metadata(&credentials_path)
            .map(|m| m.len())
            .unwrap_or(0);
        println!("✅ Credentials saved successfully! File size: {} bytes", file_size);
        
        // Try to immediately read it back to verify
        match fs::read_to_string(&credentials_path) {
            Ok(content) => println!("🔍 Verification read successful, content length: {}", content.len()),
            Err(e) => println!("❌ Verification read failed: {}", e)
        }
    } else {
        println!("❌ Credentials file does not exist after write attempt!");
    }
    
    Ok(())
}

fn load_credentials(app: &AppHandle) -> Result<Option<StoredCredentials>, String> {
    let credentials_path = get_credentials_file_path(app)?;
    
    println!("🔍 Attempting to load credentials from: {}", credentials_path.display());
    println!("📁 File exists: {}", credentials_path.exists());
    
    if !credentials_path.exists() {
        println!("❌ Credentials file does not exist");
        return Ok(None);
    }
    
    let file_metadata = fs::metadata(&credentials_path)
        .map_err(|e| format!("Failed to read file metadata: {}", e))?;
    println!("📊 File metadata: size={} bytes, modified={:?}", 
             file_metadata.len(), file_metadata.modified());
    
    let credentials_content = fs::read_to_string(&credentials_path)
        .map_err(|e| format!("Failed to read credentials file: {}", e))?;
    
    println!("📄 Read credentials file, content length: {}", credentials_content.len());
    println!("📄 Content preview (first 200 chars): {}", 
             &credentials_content.chars().take(200).collect::<String>());
    
    let credentials: StoredCredentials = serde_json::from_str(&credentials_content)
        .map_err(|e| format!("Failed to parse credentials file: {}", e))?;
    
    println!("✅ Credentials loaded successfully: user={:?}, token_length={}", 
             credentials.user_name, credentials.access_token.len());
    
    Ok(Some(credentials))
}

#[derive(Debug, Deserialize)]
struct OAuthCallbackRequest {
    token: String,
    user: String,
    email: String,
}

#[tauri::command]
async fn read_file_content(path: String) -> Result<String, String> {
    fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read file: {}", e))
}

#[tauri::command]
async fn write_file_content(path: String, content: String) -> Result<(), String> {
    fs::write(&path, content)
        .map_err(|e| format!("Failed to write file: {}", e))
}

#[tauri::command]
async fn list_directory(path: String) -> Result<Vec<FileItem>, String> {
    let dir_path = PathBuf::from(&path);
    
    if !dir_path.exists() {
        return Err("Directory does not exist".to_string());
    }

    let mut items = Vec::new();
    
    let entries = fs::read_dir(&dir_path)
        .map_err(|e| format!("Failed to read directory: {}", e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let metadata = entry.metadata()
            .map_err(|e| format!("Failed to read metadata: {}", e))?;
        
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path().to_string_lossy().to_string();
        let is_directory = metadata.is_dir();
        let size = if is_directory { None } else { Some(metadata.len()) };
        let modified = metadata.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0))
            .flatten()
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string());

        items.push(FileItem {
            name,
            path,
            is_directory,
            size,
            modified,
        });
    }

    // Sort directories first, then files
    items.sort_by(|a, b| {
        match (a.is_directory, b.is_directory) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        }
    });

    Ok(items)
}

#[tauri::command]
async fn get_editor_state(state: State<'_, AppState>) -> Result<EditorState, String> {
    let editor_state = state.editor_state.lock()
        .map_err(|_| "Failed to lock editor state")?;
    Ok(editor_state.clone())
}

#[tauri::command]
async fn set_current_file(
    path: String, 
    state: State<'_, AppState>
) -> Result<(), String> {
    let mut editor_state = state.editor_state.lock()
        .map_err(|_| "Failed to lock editor state")?;
    editor_state.current_file = Some(path);
    Ok(())
}

#[tauri::command]
async fn toggle_vim_mode(state: State<'_, AppState>) -> Result<bool, String> {
    let mut editor_state = state.editor_state.lock()
        .map_err(|_| "Failed to lock editor state")?;
    editor_state.vim_mode = !editor_state.vim_mode;
    Ok(editor_state.vim_mode)
}

#[tauri::command]
async fn authenticate_with_mothership(
    state: State<'_, AppState>
) -> Result<TokenResponse, String> {
    let client = reqwest::Client::new();
    
    // Start device flow
    let device_response = client
        .post(&format!("{}/auth/device", state.server_url))
        .send()
        .await
        .map_err(|e| format!("Failed to start device flow: {}", e))?;

    if !device_response.status().is_success() {
        return Err("Failed to start device flow".to_string());
    }

    let device_data: serde_json::Value = device_response
        .json()
        .await
        .map_err(|e| format!("Failed to parse device response: {}", e))?;

    let verification_uri = device_data["verification_uri"]
        .as_str()
        .ok_or("Missing verification_uri")?;
    let device_code = device_data["device_code"]
        .as_str()
        .ok_or("Missing device_code")?;

    // Open browser for authentication
    open_url(verification_uri, None::<String>)
        .map_err(|e| format!("Failed to open browser: {}", e))?;

    // Poll for token
    let mut attempts = 0;
    loop {
        if attempts > 30 {
            return Err("Authentication timeout".to_string());
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        let token_response = client
            .post(&format!("{}/auth/token", state.server_url))
            .json(&serde_json::json!({
                "device_code": device_code
            }))
            .send()
            .await
            .map_err(|e| format!("Failed to poll for token: {}", e))?;

        if token_response.status().is_success() {
            let token_data: TokenResponse = token_response
                .json()
                .await
                .map_err(|e| format!("Failed to parse token response: {}", e))?;

            // Store token
            let mut auth_token = state.auth_token.lock()
                .map_err(|_| "Failed to lock auth token")?;
            *auth_token = Some(token_data.access_token.clone());

            return Ok(token_data);
        }

        attempts += 1;
    }
}

#[tauri::command]
async fn load_projects(state: State<'_, AppState>) -> Result<Vec<GatewayProject>, String> {
    let auth_token = state.auth_token.lock()
        .map_err(|_| "Failed to lock auth token")?
        .clone()
        .ok_or("Not authenticated")?;

    // First get user info from auth token (same as create_gateway)
    let client = reqwest::Client::new();
    let auth_check_response = client
        .get(&format!("{}/auth/check", state.server_url))
        .bearer_auth(&auth_token)
        .send()
        .await
        .map_err(|e| format!("Failed to check auth: {}", e))?;

    if !auth_check_response.status().is_success() {
        return Err("Authentication failed - please log in again".to_string());
    }

    #[derive(serde::Deserialize)]
    struct AuthCheckResponse {
        authenticated: bool,
        user_id: uuid::Uuid,
        username: String,
        email: String,
        role: mothership_common::UserRole,
        machine_id: String,
    }

    let auth_check_result: mothership_common::protocol::ApiResponse<AuthCheckResponse> = auth_check_response
        .json()
        .await
        .map_err(|e| format!("Failed to parse auth check response: {}", e))?;

    if !auth_check_result.success {
        return Err("Authentication check failed".to_string());
    }

    let user_info = auth_check_result.data.ok_or("No user data received")?;

    if !user_info.authenticated {
        return Err("User not authenticated".to_string());
    }

    // Now load projects for the correct user
    let gateway_request = mothership_common::protocol::GatewayRequest {
        include_inactive: false,
    };

    let response = client
        .post(&format!("{}/gateway", state.server_url))
        .bearer_auth(&auth_token)
        .json(&gateway_request)
        .send()
        .await
        .map_err(|e| format!("Failed to load projects: {}", e))?;

    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(format!("Failed to load projects: {}", error_text));
    }

    let api_response: mothership_common::protocol::ApiResponse<Vec<GatewayProject>> = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse projects response: {}", e))?;

    if !api_response.success {
        return Err(api_response.error.unwrap_or("Unknown error".to_string()));
    }

    let projects = api_response.data.unwrap_or_default();

    println!("✅ Loaded {} projects for user: {}", projects.len(), user_info.username);

    // Update editor state
    {
        let mut editor_state = state.editor_state.lock()
            .map_err(|_| "Failed to lock editor state")?;
        editor_state.projects = projects.clone();
    }

    Ok(projects)
}

#[tauri::command]
async fn create_checkpoint(
    message: String,
    state: State<'_, AppState>
) -> Result<(), String> {
    let auth_token = state.auth_token.lock()
        .map_err(|_| "Failed to lock auth token")?
        .clone()
        .ok_or("Not authenticated")?;

    let client = reqwest::Client::new();
    let response = client
        .post(&format!("{}/checkpoint", state.server_url))
        .bearer_auth(&auth_token)
        .json(&serde_json::json!({
            "message": message,
            "timestamp": chrono::Utc::now()
        }))
        .send()
        .await
        .map_err(|e| format!("Failed to create checkpoint: {}", e))?;

    if !response.status().is_success() {
        return Err("Failed to create checkpoint".to_string());
    }

    Ok(())
}

#[tauri::command]
async fn start_google_oauth(state: State<'_, AppState>) -> Result<OAuthResponse, String> {
    let client = reqwest::Client::new();
    
    // Create OAuth request
    let oauth_request = OAuthRequest {
        provider: OAuthProvider::Google,
        machine_id: uuid::Uuid::new_v4().to_string(),
        machine_name: "Mothership GUI".to_string(),
        platform: std::env::consts::OS.to_string(),
        hostname: "mothership-gui".to_string(),
    };
    
    let response = client
        .post(&format!("{}/auth/oauth/start", state.server_url))
        .json(&oauth_request)
        .send()
        .await
        .map_err(|e| format!("Failed to start OAuth: {}", e))?;
    
    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(format!("OAuth start failed: {}", error_text));
    }
    
    let oauth_response: mothership_common::protocol::ApiResponse<OAuthResponse> = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse OAuth response: {}", e))?;
    
    if !oauth_response.success {
        return Err(oauth_response.error.unwrap_or_default());
    }
    
    let oauth_data = oauth_response.data.unwrap();
    
    // Open the OAuth URL in browser
    open_url(&oauth_data.auth_url, None::<String>)
        .map_err(|e| format!("Failed to open browser: {}", e))?;
    
    Ok(oauth_data)
}

#[tauri::command]
async fn save_auth_token(
    token: String, 
    state: State<'_, AppState>,
    app: AppHandle
) -> Result<(), String> {
    println!("🔐 save_auth_token called with token length: {}", token.len());
    
    {
        let mut auth_token = state.auth_token.lock()
            .map_err(|_| "Failed to lock auth token")?;
        *auth_token = Some(token.clone());
        println!("✅ Token saved to app state");
    }
    
    // Also save to persistent storage
    let credentials = StoredCredentials {
        access_token: token,
        user_email: None,
        user_name: None,
        stored_at: chrono::Utc::now().to_rfc3339(),
    };
    
    println!("💾 Attempting to save credentials to persistent storage");
    save_credentials(&app, &credentials)?;
    println!("✅ save_auth_token completed successfully");
    
    Ok(())
}

#[tauri::command]
async fn check_auth_status(state: State<'_, AppState>) -> Result<bool, String> {
    let auth_token = state.auth_token.lock()
        .map_err(|_| "Failed to lock auth token")?;
    Ok(auth_token.is_some())
}

#[tauri::command]
async fn logout(state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    // Clear auth token
    {
        let mut auth_token = state.auth_token.lock()
            .map_err(|_| "Failed to lock auth token")?;
        *auth_token = None;
    } // Drop the mutex guard here
    
    // Clear stored credentials
    clear_stored_credentials(app).await?;
    
    // Clear editor state
    {
        let mut editor_state = state.editor_state.lock()
            .map_err(|_| "Failed to lock editor state")?;
        editor_state.projects = Vec::new();
    } // Drop the mutex guard here too
    
    Ok(())
}

#[tauri::command]
async fn authenticate_with_username_password(
    _email: String,
    _password: String,
    _state: State<'_, AppState>
) -> Result<TokenResponse, String> {
    // This is a placeholder - in a real implementation you'd hash the password
    // and verify against a database
    Err("Username/password authentication not yet implemented".to_string())
}

#[tauri::command]
async fn handle_oauth_callback(
    token: String,
    user: String,
    email: String,
    state: State<'_, AppState>,
    app: AppHandle
) -> Result<(), String> {
    println!("✅ OAuth callback received for user: {}", user);
    println!("🔐 Token length: {}, Email: {}", token.len(), email);
    
    // Save the token to state
    {
        let mut auth_token = state.auth_token.lock()
            .map_err(|_| "Failed to lock auth token")?;
        *auth_token = Some(token.clone());
        println!("✅ OAuth token saved to app state");
    }
    
    // Save credentials to file for persistence
    let credentials = StoredCredentials {
        access_token: token,
        user_email: Some(email),
        user_name: Some(user),
        stored_at: chrono::Utc::now().to_rfc3339(),
    };
    
    println!("💾 Attempting to save OAuth credentials to persistent storage");
    save_credentials(&app, &credentials)?;
    println!("🎉 OAuth callback completed successfully!");
    
    Ok(())
}

#[tauri::command]
async fn validate_token(
    token: String,
    state: State<'_, AppState>
) -> Result<bool, String> {
    let client = reqwest::Client::new();
    
    // Try to make an authenticated request to validate the token
    let response = client
        .get(&format!("{}/auth/check", state.server_url))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("Failed to validate token: {}", e))?;
    
    Ok(response.status().is_success())
}

#[tauri::command]
async fn auto_login(
    state: State<'_, AppState>,
    app: AppHandle
) -> Result<bool, String> {
    println!("🔍 === AUTO-LOGIN PROCESS STARTING ===");
    
    // Try to load stored credentials
    println!("📂 Step 1: Loading stored credentials...");
    let credentials = match load_credentials(&app)? {
        Some(creds) => {
            println!("✅ Found stored credentials!");
            creds
        },
        None => {
            println!("❌ No stored credentials found");
            println!("🔍 === AUTO-LOGIN PROCESS ENDED (NO CREDENTIALS) ===");
            return Ok(false);
        }
    };
    
    println!("📝 Step 2: Validating credentials...");
    println!("👤 User: {:?}", credentials.user_name);
    println!("📧 Email: {:?}", credentials.user_email);
    println!("📅 Stored at: {}", credentials.stored_at);
    println!("🔐 Token length: {}", credentials.access_token.len());
    
    // Validate the stored token
    println!("🔍 Step 3: Validating token with server...");
    let is_valid = validate_token(credentials.access_token.clone(), state.clone()).await?;
    
    if !is_valid {
        println!("❌ Stored token is invalid, removing credentials");
        clear_stored_credentials(app).await?;
        println!("🔍 === AUTO-LOGIN PROCESS ENDED (INVALID TOKEN) ===");
        return Ok(false);
    }
    
    println!("✅ Token is valid!");
    
    // Token is valid, restore it to the app state
    println!("💾 Step 4: Restoring token to app state...");
    {
        let mut auth_token = state.auth_token.lock()
            .map_err(|_| "Failed to lock auth token")?;
        *auth_token = Some(credentials.access_token);
        println!("✅ Token restored to app state");
    }
    
    println!("🎉 === AUTO-LOGIN PROCESS COMPLETED SUCCESSFULLY ===");
    Ok(true)
}

#[tauri::command]
async fn clear_stored_credentials(app: AppHandle) -> Result<(), String> {
    let credentials_path = get_credentials_file_path(&app)?;
    
    if credentials_path.exists() {
        fs::remove_file(&credentials_path)
            .map_err(|e| format!("Failed to remove credentials file: {}", e))?;
        println!("🗑️ Stored credentials cleared");
    }
    
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct CreateGatewayRequest {
    name: String,
    description: String,
    project_path: String,
}

#[tauri::command]
async fn create_gateway(
    request: CreateGatewayRequest,
    state: State<'_, AppState>
) -> Result<mothership_common::Project, String> {
    let auth_token = state.auth_token.lock()
        .map_err(|_| "Failed to lock auth token")?
        .clone()
        .ok_or("Not authenticated")?;

    // First get user info from auth token
    let client = reqwest::Client::new();
    let auth_check_response = client
        .get(&format!("{}/auth/check", state.server_url))
        .bearer_auth(&auth_token)
        .send()
        .await
        .map_err(|e| format!("Failed to check auth: {}", e))?;

    if !auth_check_response.status().is_success() {
        return Err("Authentication failed - please log in again".to_string());
    }

    #[derive(serde::Deserialize)]
    struct AuthCheckResponse {
        authenticated: bool,
        user_id: uuid::Uuid,
        username: String,
        email: String,
        role: mothership_common::UserRole,
        machine_id: String,
    }

    let auth_check_result: mothership_common::protocol::ApiResponse<AuthCheckResponse> = auth_check_response
        .json()
        .await
        .map_err(|e| format!("Failed to parse auth check response: {}", e))?;

    if !auth_check_result.success {
        return Err("Authentication check failed".to_string());
    }

    let user_info = auth_check_result.data.ok_or("No user data received")?;

    if !user_info.authenticated {
        return Err("User not authenticated".to_string());
    }

    // Now create the gateway
    #[derive(serde::Serialize)]
    struct ServerCreateGatewayRequest {
        name: String,
        description: String,
        project_path: std::path::PathBuf,
    }

    let gateway_request = ServerCreateGatewayRequest {
        name: request.name,
        description: request.description,
        project_path: std::path::PathBuf::from(request.project_path),
    };

    let response = client
        .post(&format!("{}/gateway/create", state.server_url))
        .bearer_auth(&auth_token)
        .json(&gateway_request)
        .send()
        .await
        .map_err(|e| format!("Failed to create gateway: {}", e))?;

    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(format!("Failed to create gateway: {}", error_text));
    }

    let api_response: mothership_common::protocol::ApiResponse<mothership_common::Project> = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse gateway creation response: {}", e))?;

    if !api_response.success {
        return Err(api_response.error.unwrap_or("Unknown error".to_string()));
    }

    let project = api_response.data.ok_or("No project data received")?;

    println!("✅ Gateway created successfully: {} ({}) for user: {}", 
        project.name, project.id, user_info.username);
    
    Ok(project)
}

#[tauri::command]
async fn open_directory_dialog(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    
    let file_path = app.dialog()
        .file()
        .set_title("Select Gateway Directory")
        .blocking_pick_folder();
    
    match file_path {
        Some(path) => {
            // Convert FilePath to string
            let path_str = path.to_string();
            Ok(Some(path_str))
        },
        None => Ok(None)
    }
}

#[tauri::command]
async fn debug_credentials_file(app: AppHandle) -> Result<String, String> {
    let credentials_path = get_credentials_file_path(&app)?;
    
    let mut debug_info = format!("🔍 === CREDENTIALS FILE DEBUG ===\n");
    debug_info.push_str(&format!("📁 Path: {}\n", credentials_path.display()));
    debug_info.push_str(&format!("📂 Exists: {}\n", credentials_path.exists()));
    
    if credentials_path.exists() {
        match fs::metadata(&credentials_path) {
            Ok(metadata) => {
                debug_info.push_str(&format!("📊 Size: {} bytes\n", metadata.len()));
                debug_info.push_str(&format!("📅 Modified: {:?}\n", metadata.modified()));
            }
            Err(e) => debug_info.push_str(&format!("❌ Metadata error: {}\n", e))
        }
        
        match fs::read_to_string(&credentials_path) {
            Ok(content) => {
                debug_info.push_str(&format!("📄 Content length: {} chars\n", content.len()));
                debug_info.push_str(&format!("📄 Content preview:\n{}\n", 
                    &content.chars().take(500).collect::<String>()));
            }
            Err(e) => debug_info.push_str(&format!("❌ Read error: {}\n", e))
        }
    }
    
    debug_info.push_str("🔍 === END DEBUG ===");
    println!("{}", debug_info);
    Ok(debug_info)
}

fn main() {
    let app_state = AppState {
        editor_state: Arc::new(Mutex::new(EditorState {
            current_file: None,
            vim_mode: true, // Default to vim mode
            projects: Vec::new(),
        })),
        auth_token: Arc::new(Mutex::new(None)),
        server_url: "http://localhost:7523".to_string(),
        app_handle: None,
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            read_file_content,
            write_file_content,
            list_directory,
            get_editor_state,
            set_current_file,
            toggle_vim_mode,
            authenticate_with_mothership,
            load_projects,
            create_checkpoint,
            start_google_oauth,
            save_auth_token,
            check_auth_status,
            logout,
            authenticate_with_username_password,
            handle_oauth_callback,
            create_gateway,
            validate_token,
            auto_login,
            clear_stored_credentials,
            open_directory_dialog,
            debug_credentials_file
        ])
        .setup(|app| {
            // Store the app handle in the state for persistent operations
            let app_handle = app.handle().clone();
            let app_state = app.state::<AppState>();
            
            // Update the app state with the app handle
            let updated_state = AppState {
                editor_state: app_state.editor_state.clone(),
                auth_token: app_state.auth_token.clone(),
                server_url: app_state.server_url.clone(),
                app_handle: Some(Arc::new(Mutex::new(Some(app_handle.clone())))),
            };
            
            // Start OAuth callback server after Tauri is initialized
            tauri::async_runtime::spawn(async move {
                start_oauth_callback_server(updated_state).await;
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

async fn start_oauth_callback_server(app_state: AppState) {
    let state = app_state.clone();
    
    let app = Router::new()
        .route("/oauth/callback", post(move |AxumJson(payload): AxumJson<OAuthCallbackRequest>| {
            let state = state.clone();
            async move {
                println!("✅ OAuth callback received for user: {} ({})", payload.user, payload.email);
                println!("🔑 Token length: {} characters", payload.token.len());
                
                // Save the token to app state
                if let Ok(mut auth_token) = state.auth_token.lock() {
                    *auth_token = Some(payload.token.clone());
                    println!("✅ OAuth token saved to app state");
                } else {
                    eprintln!("❌ Failed to lock auth token");
                    return AxumResponseJson(serde_json::json!({"success": false, "error": "Failed to save token"}));
                }
                
                // Also save to persistent storage
                if let Some(app_handle_arc) = &state.app_handle {
                    if let Ok(app_handle_mutex) = app_handle_arc.lock() {
                        if let Some(app_handle) = app_handle_mutex.as_ref() {
                            let credentials = StoredCredentials {
                                access_token: payload.token,
                                user_email: Some(payload.email),
                                user_name: Some(payload.user),
                                stored_at: chrono::Utc::now().to_rfc3339(),
                            };
                            
                            println!("💾 Attempting to save OAuth credentials to persistent storage");
                            match save_credentials(app_handle, &credentials) {
                                Ok(()) => {
                                    println!("🎉 OAuth credentials saved to persistent storage!");
                                }
                                Err(e) => {
                                    eprintln!("❌ Failed to save credentials to persistent storage: {}", e);
                                }
                            }
                        } else {
                            eprintln!("❌ AppHandle not available for persistent storage");
                        }
                    } else {
                        eprintln!("❌ Failed to lock AppHandle for persistent storage");
                    }
                } else {
                    eprintln!("❌ No AppHandle available for persistent storage");
                }
                
                println!("🎉 OAuth callback processing completed!");
                AxumResponseJson(serde_json::json!({"success": true, "message": "Token and credentials saved successfully"}))
            }
        }))
        .layer(CorsLayer::permissive());

    match tokio::net::TcpListener::bind("127.0.0.1:7524").await {
        Ok(listener) => {
            println!("🔗 OAuth callback server listening on http://127.0.0.1:7524");
            if let Err(e) = axum::serve(listener, app).await {
                eprintln!("❌ OAuth callback server error: {}", e);
            }
        }
        Err(e) => {
            eprintln!("❌ Failed to bind OAuth callback server: {}", e);
        }
    }
} 