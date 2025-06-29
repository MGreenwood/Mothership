use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use mothership_common::auth::{
    AuthError, AuthRequest, AuthResponse, Claims, TokenRequest, TokenResponse,
};
use std::collections::HashMap;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Device flow state for OAuth-like authentication
#[derive(Debug, Clone)]
struct DeviceFlow {
    machine_id: String,
    machine_name: String,
    platform: String,  
    hostname: String,
    expires_at: chrono::DateTime<Utc>,
    user_authorized: Option<(Uuid, String)>, // (user_id, username)
}

#[derive(Clone)]
pub struct AuthService {
    jwt_secret: String,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    device_flows: std::sync::Arc<RwLock<HashMap<String, DeviceFlow>>>,
}

impl AuthService {
    pub fn new(jwt_secret: String) -> Self {
        let encoding_key = EncodingKey::from_secret(jwt_secret.as_bytes());
        let decoding_key = DecodingKey::from_secret(jwt_secret.as_bytes());

        Self {
            jwt_secret,
            encoding_key,
            decoding_key,
            device_flows: std::sync::Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start the device authorization flow
    pub async fn start_auth_flow(&self, request: AuthRequest) -> Result<AuthResponse> {
        let device_code = Uuid::new_v4().to_string();
        let expires_at = Utc::now() + Duration::minutes(10); // 10 minute expiration

        let flow = DeviceFlow {
            machine_id: request.machine_id,
            machine_name: request.machine_name,
            platform: request.platform,
            hostname: request.hostname,
            expires_at,
            user_authorized: None,
        };

        // Store the device flow
        {
            let mut flows = self.device_flows.write().await;
            flows.insert(device_code.clone(), flow);
        }

        // Point to the auth server for browser authentication
        // Use AUTH_SERVER_URL environment variable for production, fallback to localhost for development
        let auth_server_url = std::env::var("AUTH_SERVER_URL")
            .unwrap_or_else(|_| "http://localhost:3001".to_string());
        let auth_url = format!(
            "{}/auth/authorize?device_code={}",
            auth_server_url.trim_end_matches('/'),
            device_code
        );

        Ok(AuthResponse {
            auth_url,
            device_code,
            expires_in: 600, // 10 minutes
            interval: 5,     // Poll every 5 seconds
        })
    }

    /// Exchange device code for access token
    pub async fn exchange_token(&self, request: TokenRequest) -> Result<TokenResponse, AuthError> {
        let mut flows = self.device_flows.write().await;
        
        let flow = flows
            .get(&request.device_code)
            .ok_or(AuthError::InvalidRequest)?;

        // Check if expired
        if Utc::now() > flow.expires_at {
            flows.remove(&request.device_code);
            return Err(AuthError::ExpiredToken);
        }

        // Check if user has authorized
        let (user_id, username) = flow
            .user_authorized
            .as_ref()
            .ok_or(AuthError::AuthorizationPending)?
            .clone();

        // Generate JWT token
        let claims = Claims {
            sub: user_id.to_string(),
            machine_id: flow.machine_id.clone(),
            username: username.clone(),
            email: None, // Device flow doesn't have email information
            iat: Utc::now().timestamp(),
            exp: (Utc::now() + Duration::days(30)).timestamp(), // 30 day expiration
            aud: "mothership".to_string(),
            iss: "mothership-server".to_string(),
        };

        let access_token = encode(&Header::default(), &claims, &self.encoding_key)
            .map_err(|e| AuthError::ServerError(e.to_string()))?;

        let refresh_token = Uuid::new_v4().to_string(); // Simplified refresh token

        // Remove the device flow as it's been consumed
        flows.remove(&request.device_code);

        Ok(TokenResponse {
            access_token,
            refresh_token,
            expires_in: 30 * 24 * 60 * 60, // 30 days in seconds
            user_id,
            username,
        })
    }

    /// Verify and decode a JWT token
    pub fn verify_token(&self, token: &str) -> Result<Claims, AuthError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_audience(&["mothership"]);
        validation.set_issuer(&["mothership-server"]);

        let token_data = decode::<Claims>(token, &self.decoding_key, &validation)
            .map_err(|_| AuthError::InvalidToken)?;

        // Check if token is expired (JWT library should handle this, but double-check)
        let now = Utc::now().timestamp();
        if token_data.claims.exp < now {
            return Err(AuthError::ExpiredToken);
        }

        Ok(token_data.claims)
    }

    /// Simulate user authorization (in a real app, this would be a web interface)
    pub async fn simulate_user_authorization(
        &self,
        device_code: &str,
        user_id: Uuid,
        username: String,
    ) -> Result<()> {
        let mut flows = self.device_flows.write().await;
        
        if let Some(flow) = flows.get_mut(device_code) {
            if Utc::now() <= flow.expires_at {
                flow.user_authorized = Some((user_id, username));
                Ok(())
            } else {
                flows.remove(device_code);
                Err(anyhow!("Device code expired"))
            }
        } else {
            Err(anyhow!("Device code not found"))
        }
    }

    /// Clean up expired device flows
    pub async fn cleanup_expired_flows(&self) {
        let mut flows = self.device_flows.write().await;
        let now = Utc::now();
        flows.retain(|_, flow| flow.expires_at > now);
    }

    /// Get machine info from token
    pub fn get_machine_info(&self, token: &str) -> Result<(Uuid, String), AuthError> {
        let claims = self.verify_token(token)?;
        let user_id = Uuid::parse_str(&claims.sub).map_err(|_| AuthError::InvalidToken)?;
        Ok((user_id, claims.machine_id))
    }

    /// Encode a JWT token with given claims (public method for OAuth)
    pub fn encode_token(&self, claims: &Claims) -> Result<String, AuthError> {
        encode(&Header::default(), claims, &self.encoding_key)
            .map_err(|e| AuthError::ServerError(e.to_string()))
    }
} 