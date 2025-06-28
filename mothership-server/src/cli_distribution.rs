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
use tracing::{info, warn};

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
) -> Result<Response, StatusCode> {
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
) -> Result<Response, StatusCode> {
    let server_url = get_server_url(&state).await;
    
    let script = match platform.as_str() {
        "windows" => generate_windows_install_script(&server_url),
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
async fn list_versions() -> Result<axum::Json<Vec<VersionInfo>>, StatusCode> {
    let versions = get_available_versions().await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    Ok(axum::Json(versions))
}

/// Get the latest version info
async fn get_latest_version() -> Result<axum::Json<VersionInfo>, StatusCode> {
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
    // Extract and verify authentication token
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = auth_header.trim_start_matches("Bearer ");
    
    // Verify the token
    state.auth.verify_token(token)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    // Validate inputs
    if !is_valid_version(&version) || !is_valid_platform(&platform) || !is_valid_binary(&binary) {
        return Err(StatusCode::BAD_REQUEST);
    }
    
    let binary_path = format!("cli-binaries/{}/{}/{}", version, platform, binary);
    
    match fs::read(&binary_path).await {
        Ok(data) => {
            info!("📦 Serving binary: {} for {}", binary, platform);
            
            Ok(Response::builder()
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", binary))
                .body(data.into())
                .unwrap())
        }
        Err(_) => {
            warn!("❌ Binary not found: {}", binary_path);
            Err(StatusCode::NOT_FOUND)
        }
    }
}

/// Check for CLI updates
async fn check_for_updates(
    Query(query): Query<UpdateCheckQuery>,
    State(state): State<crate::AppState>,
) -> Result<axum::Json<UpdateCheckResponse>, StatusCode> {
    let current_version = query.current_version.unwrap_or_default();
    let platform = query.platform.unwrap_or_default();
    let binary = query.binary.unwrap_or_else(|| "mothership".to_string());
    
    let latest = get_latest_version().await?;
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

fn generate_windows_install_script(server_url: &str) -> String {
    format!(r#"# Mothership CLI Installation Script for Windows (Self-Hosted)
# Server: {server_url}

$ErrorActionPreference = "Stop"

Write-Host "🚀 Mothership CLI Installation" -ForegroundColor Cyan
Write-Host "📡 Server: {server_url}" -ForegroundColor Blue
Write-Host ""

# Get latest version
Write-Host "🔍 Checking latest version..." -ForegroundColor Yellow
$LatestInfo = Invoke-RestMethod -Uri "{server_url}/cli/latest"
$LatestVersion = $LatestInfo.version

Write-Host "📦 Latest version: $LatestVersion" -ForegroundColor Green

# Detect architecture
$Arch = if ([Environment]::Is64BitOperatingSystem) {{ "x86_64" }} else {{ "i686" }}
$Platform = "$Arch-pc-windows-msvc"

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

fn generate_unix_install_script(_server_url: &str) -> String {
    format!(r#"#!/bin/bash
# Already implemented in serve_install_script function
# This is just the platform-specific version
"#)
} 