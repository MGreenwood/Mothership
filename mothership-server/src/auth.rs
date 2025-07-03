use anyhow::Result;
use chrono::Utc;
use jsonwebtoken::{decode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use mothership_common::auth::{
    AuthError, Claims, OAuthProfile, OAuthProvider, OAuthRequest, OAuthResponse, OAuthSource,
};
use tracing::{error, info, warn};

/// Authentication service for handling JWT tokens
#[derive(Clone)]
pub struct AuthService {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
}

impl AuthService {
    pub fn new(secret: String) -> Self {
        let encoding_key = EncodingKey::from_secret(secret.as_bytes());
        let decoding_key = DecodingKey::from_secret(secret.as_bytes());
        Self {
            encoding_key,
            decoding_key,
        }
    }

    /// Encode a JWT token with the given claims
    pub fn encode_token(&self, claims: &Claims) -> Result<String, AuthError> {
        let header = Header::new(Algorithm::HS256);
        jsonwebtoken::encode(&header, claims, &self.encoding_key)
            .map_err(|_| AuthError::InvalidToken)
    }

    /// Verify and decode a JWT token
    pub fn verify_token(&self, token: &str) -> Result<Claims, AuthError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = true;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        validation.leeway = 0;

        let token_data = decode::<Claims>(token, &self.decoding_key, &validation)
            .map_err(|_| AuthError::InvalidToken)?;

        Ok(token_data.claims)
    }

    /// Simulate user authorization for device code flow
    pub async fn simulate_user_authorization(&self, device_code: &str, user_id: uuid::Uuid, username: String) -> Result<(), AuthError> {
        let now = Utc::now();
        
        // Create claims for the device
        let claims = Claims {
            sub: user_id.to_string(),
            machine_id: device_code.to_string(),
            username,
            email: None,
            iat: now.timestamp(),
            exp: (now + chrono::Duration::days(30)).timestamp(),
            aud: "mothership".to_string(),
            iss: "mothership-server".to_string(),
        };

        // Encode token and store it (the actual storage would be handled by the sessions system)
        self.encode_token(&claims)?;
        Ok(())
    }
} 