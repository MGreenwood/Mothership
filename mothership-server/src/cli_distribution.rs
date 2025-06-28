use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tracing::{info, warn, error};

/// CLI distribution endpoints for self-hosted binary updates
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/cli/install", get(serve_install_script))
        .route("/cli/install/:platform", get(serve_install_script_platform))
        .route("/cli/versions", get(list_versions))
        .route("/cli/latest", get(get_latest_version))
        .route("/cli/download/:version/:platform/:binary", get(download_binary))
        .route("/cli/update-check", get(check_for_updates))
}

#[derive(Debug, Serialize)]
struct VersionInfo {
    version: String,
    platforms: Vec<String>,
    release_date: chrono::DateTime<chrono::Utc>,
    changes: Vec<String>,
}

#[derive(Debug, Serialize)]
struct UpdateCheckResponse {
    current_version: String,
    latest_version: String,
    update_available: bool,
    download_url: Option<String>,
    changes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateCheckQuery {
    current_version: Option<String>,
    platform: Option<String>,
    binary: Option<String>,
}

/// Serve the installation script with server URL pre-configured
async fn serve_install_script(
    State(state): State<crate::AppState>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    // Require authentication if whitelist is enabled (private deployment)
    if state.whitelist.is_some() {
        let _user = verify_authenticated_user(&state, &headers).await?;
        info!("📋 Serving install script to authenticated user");
    } else {
        info!("📋 Serving public install script");
    }
    let server_url = get_server_url(&state).await;
    
    let script = format!(r#"#!/bin/bash
set -e

# Mothership CLI Installation Script (Self-Hosted)
# Server: {server_url}

BOLD='\033[1m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${{BOLD}}🚀 Mothership CLI Installation${{NC}}"
echo -e "${{BLUE}}📡 Server: {server_url}${{NC}}"
echo ""

# Detect platform
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case $ARCH in
    x86_64) ARCH="x86_64" ;;
    aarch64|arm64) ARCH="aarch64" ;;
    *) echo -e "${{RED}}❌ Unsupported architecture: $ARCH${{NC}}"; exit 1 ;;
esac

case $OS in
    linux) OS="unknown-linux-gnu" ;;
    darwin) OS="apple-darwin" ;;
    *) echo -e "${{RED}}❌ Unsupported OS: $OS${{NC}}"; exit 1 ;;
esac

PLATFORM="${{ARCH}}-${{OS}}"
echo -e "${{BLUE}}📋 Detected platform: $PLATFORM${{NC}}"

# Get latest version
echo -e "${{YELLOW}}🔍 Checking latest version...${{NC}}"
LATEST_VERSION=$(curl -s "{server_url}/cli/latest" | grep -o '"version":"[^"]*"' | cut -d'"' -f4)

if [ -z "$LATEST_VERSION" ]; then
    echo -e "${{RED}}❌ Failed to get latest version${{NC}}"
    exit 1
fi

echo -e "${{GREEN}}📦 Latest version: $LATEST_VERSION${{NC}}"

# Download and install CLI
echo -e "${{YELLOW}}⬇️  Downloading mothership CLI...${{NC}}"
curl -L -o /tmp/mothership "{server_url}/cli/download/$LATEST_VERSION/$PLATFORM/mothership"
chmod +x /tmp/mothership
sudo mv /tmp/mothership /usr/local/bin/

# Download and install daemon
echo -e "${{YELLOW}}⬇️  Downloading mothership daemon...${{NC}}"
curl -L -o /tmp/mothership-daemon "{server_url}/cli/download/$LATEST_VERSION/$PLATFORM/mothership-daemon"
chmod +x /tmp/mothership-daemon
sudo mv /tmp/mothership-daemon /usr/local/bin/

# Set server URL for CLI
mkdir -p ~/.config/mothership
echo "server_url={server_url}" > ~/.config/mothership/config.toml

echo ""
echo -e "${{GREEN}}✅ Installation complete!${{NC}}"
echo ""
echo -e "${{YELLOW}}Quick start:${{NC}}"
echo -e "  ${{BLUE}}mothership auth${{NC}}           # Authenticate"
echo -e "  ${{BLUE}}cd your-project${{NC}}"
echo -e "  ${{BLUE}}mothership deploy${{NC}}        # Deploy project"
echo -e "  ${{BLUE}}mothership beam project${{NC}}  # Start collaboration"
echo ""
echo -e "${{YELLOW}}Stay updated:${{NC}}"
echo -e "  ${{BLUE}}mothership update${{NC}}        # Update to latest version"
echo ""
"#);

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/plain")
        .body(script.into())
        .unwrap())
}

/// Serve platform-specific installation script
async fn serve_install_script_platform(
    Path(platform): Path<String>,
    State(state): State<crate::AppState>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    // Require authentication if whitelist is enabled (private deployment)
    if state.whitelist.is_some() {
        let _user = verify_authenticated_user(&state, &headers).await?;
        info!("📋 Serving platform-specific install script to authenticated user");
    } else {
        info!("📋 Serving public platform-specific install script");
    }
    let server_url = get_server_url(&state).await;
    
    let auth_required = state.config.cli_distribution.require_auth_for_downloads || state.whitelist.is_some();
    
    let script = match platform.as_str() {
        "windows" => generate_windows_install_script(&server_url, auth_required),
        "unix" | "linux" | "macos" => generate_unix_install_script(&server_url),
        _ => return Err(StatusCode::BAD_REQUEST),
    };

    let content_type = if platform == "windows" {
        "text/plain; charset=utf-8"
    } else {
        "text/plain"
    };

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(script.into())
        .unwrap())
}

/// List all available versions
async fn list_versions(
    State(state): State<crate::AppState>,
    headers: HeaderMap,
) -> Result<axum::Json<Vec<VersionInfo>>, StatusCode> {
    // Always require authentication for version info (sensitive data)
    let (user_id, username, _) = verify_authenticated_user(&state, &headers).await?;
    info!("📋 Listing versions for user: {} ({})", username, user_id);
    
    let versions = get_available_versions().await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    Ok(axum::Json(versions))
}

/// Get the latest version info
async fn get_latest_version(
    State(state): State<crate::AppState>,
    headers: HeaderMap,
) -> Result<axum::Json<VersionInfo>, StatusCode> {
    // Always require authentication for version info (sensitive data)
    let (user_id, username, _) = verify_authenticated_user(&state, &headers).await?;
    info!("📋 Getting latest version for user: {} ({})", username, user_id);
    
    let versions = get_available_versions().await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    let latest = versions.into_iter()
        .max_by(|a, b| a.version.cmp(&b.version))
        .ok_or(StatusCode::NOT_FOUND)?;
    
    Ok(axum::Json(latest))
}

/// Download a specific binary
async fn download_binary(
    State(state): State<crate::AppState>,
    headers: HeaderMap,
    Path((version, platform, binary)): Path<(String, String, String)>,
) -> Result<Response, StatusCode> {
    // Verify authentication and whitelist
    let (user_id, username, _) = verify_authenticated_user(&state, &headers).await?;
    // Validate inputs
    if !is_valid_version(&version) || !is_valid_platform(&platform) || !is_valid_binary(&binary) {
        return Err(StatusCode::BAD_REQUEST);
    }
    
    let binary_path = format!("cli-binaries/{}/{}/{}", version, platform, binary);
    
    match fs::read(&binary_path).await {
        Ok(data) => {
            info!("📦 Serving binary: {} ({}) to user: {} ({})", binary, platform, username, user_id);
            
            Ok(Response::builder()
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", binary))
                .body(data.into())
                .unwrap())
        }
        Err(_) => {
            warn!("❌ Binary not found: {} (requested by user: {})", binary_path, username);
            Err(StatusCode::NOT_FOUND)
        }
    }
}

/// Check for CLI updates
async fn check_for_updates(
    Query(query): Query<UpdateCheckQuery>,
    State(state): State<crate::AppState>,
    headers: HeaderMap,
) -> Result<axum::Json<UpdateCheckResponse>, StatusCode> {
    // Always require authentication for update checks (reveals version info)
    let (user_id, username, _) = verify_authenticated_user(&state, &headers).await?;
    info!("🔄 Checking updates for user: {} ({})", username, user_id);
    let current_version = query.current_version.unwrap_or_default();
    let platform = query.platform.unwrap_or_default();
    let binary = query.binary.unwrap_or_else(|| "mothership".to_string());
    
    let latest = get_latest_version(State(state.clone()), headers.clone()).await?;
    let latest_version = latest.0.version.clone();
    let update_available = version_compare(&current_version, &latest_version);
    
    let download_url = if update_available && !platform.is_empty() {
        let server_url = get_server_url(&state).await;
        Some(format!("{}/cli/download/{}/{}/{}", server_url, latest_version, platform, binary))
    } else {
        None
    };
    
    Ok(axum::Json(UpdateCheckResponse {
        current_version,
        latest_version,
        update_available,
        download_url,
        changes: latest.0.changes,
    }))
}

// Helper functions

/// Verify authentication token and check whitelist
async fn verify_authenticated_user(
    state: &crate::AppState,
    headers: &HeaderMap,
) -> Result<(uuid::Uuid, String, String), StatusCode> {
    // Always require auth if whitelist is enabled, regardless of config
    if state.whitelist.is_some() && !state.config.cli_distribution.require_auth_for_downloads {
        warn!("🔒 Whitelist enabled but CLI auth disabled - this is a security risk!");
    }
    
    // Skip authentication only if both whitelist is disabled AND auth is disabled
    if state.whitelist.is_none() && !state.config.cli_distribution.require_auth_for_downloads {
        info!("🔓 CLI access allowed without authentication (no whitelist, auth disabled)");
        // Return a dummy user for logging purposes
        return Ok((
            uuid::Uuid::nil(),
            "anonymous".to_string(),
            "anonymous@local".to_string(),
        ));
    }

    // Extract Bearer token from Authorization header
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            warn!("❌ CLI download attempted without authorization header");
            StatusCode::UNAUTHORIZED
        })?;

    if !auth_header.starts_with("Bearer ") {
        warn!("❌ CLI download attempted with invalid authorization format");
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = auth_header.trim_start_matches("Bearer ");

    // Verify the token
    let claims = state.auth.verify_token(token)
        .map_err(|e| {
            warn!("❌ CLI download attempted with invalid token: {}", e);
            StatusCode::UNAUTHORIZED
        })?;

    // Get user from database
    let user_id = uuid::Uuid::parse_str(&claims.sub)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let user = state.db.get_user(user_id).await
        .map_err(|e| {
            error!("Database error during CLI auth: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or_else(|| {
            warn!("❌ CLI download attempted by non-existent user: {}", user_id);
            StatusCode::UNAUTHORIZED
        })?;

    // Check whitelist if enabled
    if let Some(whitelist) = &state.whitelist {
        if !whitelist.is_user_allowed(&user.username, &user.email) {
            warn!("❌ CLI download denied - user {} ({}) not in whitelist", user.username, user.email);
            return Err(StatusCode::FORBIDDEN);
        }
        info!("✅ CLI download authorized for whitelisted user: {} ({})", user.username, user.email);
    } else {
        info!("✅ CLI download authorized for user: {} ({})", user.username, user.email);
    }

    Ok((user.id, user.username, user.email))
}

async fn get_server_url(_state: &crate::AppState) -> String {
    // Get from config or use default
    std::env::var("MOTHERSHIP_SERVER_URL")
        .unwrap_or_else(|_| "http://localhost:7523".to_string())
}

async fn get_available_versions() -> Result<Vec<VersionInfo>> {
    // Read from cli-binaries directory or database
    // For now, return current version
    Ok(vec![VersionInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        platforms: vec![
            "x86_64-unknown-linux-gnu".to_string(),
            "aarch64-unknown-linux-gnu".to_string(),
            "x86_64-apple-darwin".to_string(),
            "aarch64-apple-darwin".to_string(),
            "x86_64-pc-windows-msvc".to_string(),
        ],
        release_date: chrono::Utc::now(),
        changes: vec![
            "🔥 Fixed file watcher async/sync boundary issue".to_string(),
            "✅ Real-time collaboration working".to_string(),
            "🚀 Self-hosted CLI distribution".to_string(),
        ],
    }])
}

fn is_valid_version(version: &str) -> bool {
    version.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '-')
}

fn is_valid_platform(platform: &str) -> bool {
    matches!(platform, 
        "x86_64-unknown-linux-gnu" | 
        "aarch64-unknown-linux-gnu" |
        "x86_64-apple-darwin" |
        "aarch64-apple-darwin" |
        "x86_64-pc-windows-msvc"
    )
}

fn is_valid_binary(binary: &str) -> bool {
    matches!(binary, "mothership" | "mothership-daemon" | "mothership.exe" | "mothership-daemon.exe")
}

fn version_compare(current: &str, latest: &str) -> bool {
    // Simple version comparison - in production would use semver
    current != latest
}

fn generate_windows_install_script(server_url: &str, auth_required: bool) -> String {
    if auth_required {
        format!(r#"# Mothership CLI Installation Script for Windows (Self-Hosted)
# Server: {server_url}

$ErrorActionPreference = "Stop"

Write-Host "🚀 Mothership CLI Installation" -ForegroundColor Cyan
Write-Host "📡 Server: {server_url}" -ForegroundColor Blue
Write-Host ""

Write-Host "🔐 This server requires authentication for CLI downloads." -ForegroundColor Yellow
Write-Host ""
Write-Host "To install the Mothership CLI:" -ForegroundColor Blue
Write-Host "1. Visit: {server_url}/login" -ForegroundColor Blue
Write-Host "2. Complete authentication in your browser" -ForegroundColor Blue  
Write-Host "3. Copy your authentication token" -ForegroundColor Blue
Write-Host "4. Run this script with your token:" -ForegroundColor Blue
Write-Host ""
Write-Host '$env:MOTHERSHIP_TOKEN="your_token_here"; irm {server_url}/cli/install/windows | iex' -ForegroundColor Green
Write-Host ""

# Check if token is provided
if (-not $env:MOTHERSHIP_TOKEN) {{
    Write-Host "❌ No authentication token provided" -ForegroundColor Red
    Write-Host "Please set MOTHERSHIP_TOKEN environment variable and try again" -ForegroundColor Yellow
    exit 1
}}

Write-Host "✅ Using provided authentication token" -ForegroundColor Green
$Headers = @{{ "Authorization" = "Bearer $env:MOTHERSHIP_TOKEN" }}

# Detect architecture
$Arch = if ([Environment]::Is64BitOperatingSystem) {{ "x86_64" }} else {{ "i686" }}
$Platform = "$Arch-pc-windows-msvc"

Write-Host "📋 Detected platform: $Platform" -ForegroundColor Blue

# Get latest version
Write-Host "🔍 Checking latest version..." -ForegroundColor Yellow
try {{
    $LatestInfo = Invoke-RestMethod -Uri "{server_url}/cli/latest" -Headers $Headers
    $LatestVersion = $LatestInfo.version
    Write-Host "📦 Latest version: $LatestVersion" -ForegroundColor Green
}} catch {{
    Write-Host "❌ Failed to get latest version (check your token)" -ForegroundColor Red
    exit 1
}}

# Create installation directory
$InstallDir = "$env:LOCALAPPDATA\Mothership"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

# Download CLI
Write-Host "⬇️  Downloading mothership CLI..." -ForegroundColor Yellow
$CliUrl = "{server_url}/cli/download/$LatestVersion/$Platform/mothership.exe"
Invoke-WebRequest -Uri $CliUrl -Headers $Headers -OutFile "$InstallDir\mothership.exe"

# Download daemon
Write-Host "⬇️  Downloading mothership daemon..." -ForegroundColor Yellow
$DaemonUrl = "{server_url}/cli/download/$LatestVersion/$Platform/mothership-daemon.exe"
Invoke-WebRequest -Uri $DaemonUrl -Headers $Headers -OutFile "$InstallDir\mothership-daemon.exe"

# Add to PATH
$CurrentPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($CurrentPath -notlike "*$InstallDir*") {{
    [Environment]::SetEnvironmentVariable("PATH", "$CurrentPath;$InstallDir", "User")
    $env:PATH += ";$InstallDir"
}}

# Create config
$ConfigDir = "$env:USERPROFILE\.config\mothership"
New-Item -ItemType Directory -Force -Path $ConfigDir | Out-Null
"server_url={server_url}" | Out-File -FilePath "$ConfigDir\config.toml" -Encoding UTF8

Write-Host ""
Write-Host "✅ Installation complete!" -ForegroundColor Green
Write-Host ""
Write-Host "Quick start:" -ForegroundColor Yellow
Write-Host "  mothership auth           # Authenticate" -ForegroundColor Blue
Write-Host "  cd your-project" -ForegroundColor Blue
Write-Host "  mothership deploy        # Deploy project" -ForegroundColor Blue
Write-Host "  mothership beam project  # Start collaboration" -ForegroundColor Blue
Write-Host ""
Write-Host "Stay updated:" -ForegroundColor Yellow
Write-Host "  mothership update        # Update to latest version" -ForegroundColor Blue
Write-Host ""
"#)
    } else {
        format!(r#"# Mothership CLI Installation Script for Windows (Self-Hosted)
# Server: {server_url}

$ErrorActionPreference = "Stop"

Write-Host "🚀 Mothership CLI Installation" -ForegroundColor Cyan
Write-Host "📡 Server: {server_url}" -ForegroundColor Blue
Write-Host ""

# Detect architecture
$Arch = if ([Environment]::Is64BitOperatingSystem) {{ "x86_64" }} else {{ "i686" }}
$Platform = "$Arch-pc-windows-msvc"

Write-Host "📋 Detected platform: $Platform" -ForegroundColor Blue

# Get latest version
Write-Host "🔍 Checking latest version..." -ForegroundColor Yellow
$LatestInfo = Invoke-RestMethod -Uri "{server_url}/cli/latest"
$LatestVersion = $LatestInfo.version

Write-Host "📦 Latest version: $LatestVersion" -ForegroundColor Green

# Create installation directory
$InstallDir = "$env:LOCALAPPDATA\Mothership"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

# Download CLI
Write-Host "⬇️  Downloading mothership CLI..." -ForegroundColor Yellow
$CliUrl = "{server_url}/cli/download/$LatestVersion/$Platform/mothership.exe"
Invoke-WebRequest -Uri $CliUrl -OutFile "$InstallDir\mothership.exe"

# Download daemon
Write-Host "⬇️  Downloading mothership daemon..." -ForegroundColor Yellow
$DaemonUrl = "{server_url}/cli/download/$LatestVersion/$Platform/mothership-daemon.exe"
Invoke-WebRequest -Uri $DaemonUrl -OutFile "$InstallDir\mothership-daemon.exe"

# Add to PATH
$CurrentPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($CurrentPath -notlike "*$InstallDir*") {{
    [Environment]::SetEnvironmentVariable("PATH", "$CurrentPath;$InstallDir", "User")
    $env:PATH += ";$InstallDir"
}}

# Create config
$ConfigDir = "$env:USERPROFILE\.config\mothership"
New-Item -ItemType Directory -Force -Path $ConfigDir | Out-Null
"server_url={server_url}" | Out-File -FilePath "$ConfigDir\config.toml" -Encoding UTF8

Write-Host ""
Write-Host "✅ Installation complete!" -ForegroundColor Green
Write-Host ""
Write-Host "Quick start:" -ForegroundColor Yellow
Write-Host "  mothership auth           # Authenticate" -ForegroundColor Blue
Write-Host "  cd your-project" -ForegroundColor Blue
Write-Host "  mothership deploy        # Deploy project" -ForegroundColor Blue
Write-Host "  mothership beam project  # Start collaboration" -ForegroundColor Blue
Write-Host ""
Write-Host "Stay updated:" -ForegroundColor Yellow
Write-Host "  mothership update        # Update to latest version" -ForegroundColor Blue
Write-Host ""
"#)
    }
}

fn generate_unix_install_script(_server_url: &str) -> String {
    format!(r#"#!/bin/bash
# Already implemented in serve_install_script function
# This is just the platform-specific version
"#)
} 