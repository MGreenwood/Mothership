use anyhow::Result;
use mothership_common::auth::{AuthError, OAuthProfile, OAuthProvider, OAuthSource};
use oauth2::{
    basic::BasicClient, reqwest::async_http_client, AuthUrl, AuthorizationCode, ClientId,
    ClientSecret, CsrfToken, RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use serde::Deserialize;
use std::collections::HashMap;
use tokio::sync::RwLock;

/// OAuth configuration for a provider
#[derive(Clone)]
struct OAuthConfig {
    client: BasicClient,
    scopes: Vec<String>,
    user_info_url: String,
}

/// OAuth service for handling Google and GitHub authentication
#[derive(Clone)]
pub struct OAuthService {
    providers: HashMap<OAuthProvider, OAuthConfig>,
    pending_states: std::sync::Arc<RwLock<HashMap<String, (OAuthProvider, OAuthSource)>>>,
}

impl OAuthService {
    pub fn new() -> Result<Self> {
        let mut providers = HashMap::new();

        // Get OAuth base URL from environment or use default
        let oauth_base_url = std::env::var("OAUTH_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:7523".to_string());

        // Configure Google OAuth
        let google_client_id = std::env::var("GOOGLE_CLIENT_ID");
        let google_client_secret = std::env::var("GOOGLE_CLIENT_SECRET");
        
        println!("🔍 OAuth Debug - Google Client ID: {:?}", google_client_id.as_ref().map(|s| format!("{}...", &s[..10.min(s.len())])));
        println!("🔍 OAuth Debug - Google Client Secret: {:?}", google_client_secret.as_ref().map(|s| format!("{}...", &s[..10.min(s.len())])));
        println!("🔍 OAuth Debug - Base URL: {}", oauth_base_url);
        
        if let (Ok(client_id), Ok(client_secret)) = (google_client_id, google_client_secret) {
            let google_config = OAuthConfig {
                client: BasicClient::new(
                    ClientId::new(client_id),
                    Some(ClientSecret::new(client_secret)),
                    AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string())?,
                    Some(TokenUrl::new("https://oauth2.googleapis.com/token".to_string())?),
                )
                .set_redirect_uri(RedirectUrl::new(format!("{}/auth/callback/google", oauth_base_url))?),
                scopes: vec!["openid".to_string(), "email".to_string(), "profile".to_string()],
                user_info_url: "https://www.googleapis.com/oauth2/v2/userinfo".to_string(),
            };
            providers.insert(OAuthProvider::Google, google_config);
            println!("✅ Google OAuth provider configured successfully");
        } else {
            println!("❌ Google OAuth provider not configured - missing environment variables");
        }

        // Configure GitHub OAuth
        if let (Ok(client_id), Ok(client_secret)) = (
            std::env::var("GITHUB_CLIENT_ID"),
            std::env::var("GITHUB_CLIENT_SECRET"),
        ) {
            let github_config = OAuthConfig {
                client: BasicClient::new(
                    ClientId::new(client_id),
                    Some(ClientSecret::new(client_secret)),
                    AuthUrl::new("https://github.com/login/oauth/authorize".to_string())?,
                    Some(TokenUrl::new("https://github.com/login/oauth/access_token".to_string())?),
                )
                .set_redirect_uri(RedirectUrl::new(format!("{}/auth/callback/github", oauth_base_url))?),
                scopes: vec!["user:email".to_string()],
                user_info_url: "https://api.github.com/user".to_string(),
            };
            providers.insert(OAuthProvider::GitHub, github_config);
        }

        Ok(Self {
            providers,
            pending_states: std::sync::Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Generate authorization URL for OAuth flow
    pub async fn get_authorization_url(&self, provider: OAuthProvider, source: OAuthSource) -> Result<(String, String), AuthError> {
        let config = self.providers.get(&provider)
            .ok_or_else(|| AuthError::OAuthError(format!("Provider {:?} not configured", provider)))?;

        let scopes: Vec<Scope> = config.scopes.iter()
            .map(|s| Scope::new(s.clone()))
            .collect();

        let (auth_url, csrf_token) = config.client
            .authorize_url(CsrfToken::new_random)
            .add_scopes(scopes)
            .url();

        let state = csrf_token.secret().clone();
        
        // Store the state for validation along with source
        {
            let mut pending_states = self.pending_states.write().await;
            pending_states.insert(state.clone(), (provider, source));
        }

        Ok((auth_url.to_string(), state))
    }

    /// Exchange authorization code for user profile
    pub async fn exchange_code(&self, code: String, state: String) -> Result<(OAuthProfile, OAuthSource), AuthError> {
        // Validate state and get provider
        let (provider, source) = {
            let mut pending_states = self.pending_states.write().await;
            pending_states.remove(&state)
                .ok_or_else(|| AuthError::OAuthError("Invalid or expired state".to_string()))?
        };

        let config = self.providers.get(&provider)
            .ok_or_else(|| AuthError::OAuthError(format!("Provider {:?} not configured", provider)))?;

        // Exchange code for token
        let token = config.client
            .exchange_code(AuthorizationCode::new(code))
            .request_async(async_http_client)
            .await
            .map_err(|e| AuthError::OAuthError(format!("Token exchange failed: {}", e)))?;

        // Fetch user profile
        let profile = self.fetch_user_profile(&provider, token.access_token().secret()).await?;
        
        Ok((profile, source))
    }

    /// Fetch user profile from OAuth provider
    async fn fetch_user_profile(&self, provider: &OAuthProvider, access_token: &str) -> Result<OAuthProfile, AuthError> {
        let config = self.providers.get(provider)
            .ok_or_else(|| AuthError::OAuthError(format!("Provider {:?} not configured", provider)))?;

        let client = reqwest::Client::new();
        let response = client
            .get(&config.user_info_url)
            .bearer_auth(access_token)
            .header("User-Agent", "Mothership/1.0")
            .send()
            .await
            .map_err(|e| AuthError::OAuthError(format!("Failed to fetch user profile: {}", e)))?;

        if !response.status().is_success() {
            return Err(AuthError::OAuthError(format!(
                "Failed to fetch user profile: HTTP {}",
                response.status()
            )));
        }

        let user_data: serde_json::Value = response.json().await
            .map_err(|e| AuthError::OAuthError(format!("Failed to parse user profile: {}", e)))?;

        match provider {
            OAuthProvider::Google => {
                Ok(OAuthProfile {
                    provider: provider.clone(),
                    provider_id: user_data["id"].as_str().unwrap_or("").to_string(),
                    email: user_data["email"].as_str().unwrap_or("").to_string(),
                    name: user_data["name"].as_str().unwrap_or("").to_string(),
                    username: None, // Google doesn't provide username
                    avatar_url: user_data["picture"].as_str().map(|s| s.to_string()),
                })
            }
            OAuthProvider::GitHub => {
                // For GitHub, we might need to fetch email separately if it's private
                let email = if let Some(email) = user_data["email"].as_str() {
                    email.to_string()
                } else {
                    self.fetch_github_email(access_token).await?
                };

                Ok(OAuthProfile {
                    provider: provider.clone(),
                    provider_id: user_data["id"].as_u64().unwrap_or(0).to_string(),
                    email,
                    name: user_data["name"].as_str().unwrap_or("").to_string(),
                    username: user_data["login"].as_str().map(|s| s.to_string()),
                    avatar_url: user_data["avatar_url"].as_str().map(|s| s.to_string()),
                })
            }
        }
    }

    /// Fetch GitHub user's email (needed when email is private)
    async fn fetch_github_email(&self, access_token: &str) -> Result<String, AuthError> {
        let client = reqwest::Client::new();
        let response = client
            .get("https://api.github.com/user/emails")
            .bearer_auth(access_token)
            .header("User-Agent", "Mothership/1.0")
            .send()
            .await
            .map_err(|e| AuthError::OAuthError(format!("Failed to fetch GitHub emails: {}", e)))?;

        if !response.status().is_success() {
            return Err(AuthError::OAuthError("Failed to fetch user emails".to_string()));
        }

        let emails: Vec<GitHubEmail> = response.json().await
            .map_err(|e| AuthError::OAuthError(format!("Failed to parse emails: {}", e)))?;

        // Find primary email or first verified email
        emails.iter()
            .find(|e| e.primary)
            .or_else(|| emails.iter().find(|e| e.verified))
            .or_else(|| emails.first())
            .map(|e| e.email.clone())
            .ok_or_else(|| AuthError::OAuthError("No email found".to_string()))
    }

    /// Clean up expired states
    pub async fn cleanup_expired_states(&self) {
        // For now, just clear all states older than 10 minutes
        // In production, you'd want to track timestamps
        let mut pending_states = self.pending_states.write().await;
        pending_states.clear();
    }
}

#[derive(Deserialize)]
struct GitHubEmail {
    email: String,
    verified: bool,
    primary: bool,
} 