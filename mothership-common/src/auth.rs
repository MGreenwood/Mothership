use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use thiserror::Error;

/// OAuth provider types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum OAuthProvider {
    Google,
    GitHub,
}

/// OAuth source type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum OAuthSource {
    Web,        // Web browser download flow
    CLI,        // CLI authentication flow
    GUI,        // GUI application flow
}

impl Default for OAuthSource {
    fn default() -> Self {
        Self::Web
    }
}

/// OAuth authentication request (initiate flow)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthRequest {
    pub provider: OAuthProvider,
    pub machine_id: String,
    pub machine_name: String,
    pub platform: String,
    pub hostname: String,
    #[serde(default)]
    pub source: OAuthSource,
}

/// OAuth authentication response (with redirect URL)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthResponse {
    pub auth_url: String,
    pub state: String, // CSRF protection
    pub expires_in: u64,
}

/// OAuth callback data (from redirect)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCallback {
    pub code: String,
    pub state: String,
    pub provider: OAuthProvider,
}

/// User profile from OAuth provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthProfile {
    pub provider: OAuthProvider,
    pub provider_id: String,
    pub email: String,
    pub name: String,
    pub username: Option<String>,
    pub avatar_url: Option<String>,
}

/// Legacy device flow (keeping for backward compatibility)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequest {
    pub machine_id: String,    // Unique machine identifier
    pub machine_name: String,  // Human-readable machine name
    pub platform: String,     // OS platform (Windows, macOS, Linux)
    pub hostname: String,      // Machine hostname
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub auth_url: String,      // URL to open in browser for OAuth
    pub device_code: String,   // Device code for polling
    pub expires_in: u64,       // Expiration time in seconds
    pub interval: u64,         // Polling interval in seconds
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRequest {
    pub device_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub user_id: Uuid,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineAuth {
    pub machine_id: String,
    pub user_id: Uuid,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// JWT Claims structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,           // Subject (user_id)
    pub machine_id: String,    // Machine identifier
    pub username: String,      // Username for display
    pub email: Option<String>, // User email (for OAuth user recreation)
    pub iat: i64,             // Issued at
    pub exp: i64,             // Expiration time
    pub aud: String,          // Audience (mothership)
    pub iss: String,          // Issuer (mothership-server)
}

/// Authentication errors
#[derive(Debug, Clone, Serialize, Deserialize, Error)]
pub enum AuthError {
    #[error("Invalid request")]
    InvalidRequest,
    #[error("Invalid token")]
    InvalidToken,
    #[error("Expired token")]
    ExpiredToken,
    #[error("Authorization pending")]
    AuthorizationPending,
    #[error("Access denied")]
    AccessDenied,
    #[error("Server error: {0}")]
    ServerError(String),
    #[error("OAuth error: {0}")]
    OAuthError(String),
}



// Machine information for display in UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineInfo {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub hostname: String,
    pub last_seen: DateTime<Utc>,
    pub is_active: bool,
} 