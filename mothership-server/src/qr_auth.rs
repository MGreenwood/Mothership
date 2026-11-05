use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::Mutex;
use uuid::Uuid;
use mothership_common::UserRole;
use crate::AppState;
use tracing::error;

// In-memory storage for QR challenges
lazy_static::lazy_static! {
    static ref QR_CHALLENGES: Mutex<HashMap<String, QrChallenge>> = Mutex::new(HashMap::new());
    static ref QR_RESULTS: Mutex<HashMap<String, QrResult>> = Mutex::new(HashMap::new());
    static ref QR_USED: Mutex<std::collections::HashSet<String>> = Mutex::new(std::collections::HashSet::new());
}

// Public access functions for QR challenges and results
pub async fn get_qr_challenge(challenge_id: &str) -> Option<QrChallenge> {
    let challenges = QR_CHALLENGES.lock().await;
    challenges.get(challenge_id).cloned()
}

pub async fn update_qr_result(challenge_id: String, result: QrResult) {
    let mut results = QR_RESULTS.lock().await;
    results.insert(challenge_id, result);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QrChallenge {
    pub challenge_id: String,
    pub domain: String,
    pub action: String,
    pub nonce: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

impl QrChallenge {
    /// Canonical message the wallet signs. Must match the GhostID auth-server
    /// `canonicalAuthMessage` (protocolVersion 1) field order exactly.
    pub fn canonical_auth_message(&self, public_key: &str, token_id: &str) -> String {
        let s = |v: &str| serde_json::to_string(v).unwrap_or_else(|_| "\"\"".to_string());
        format!(
            "{{\"protocolVersion\":1,\"challengeId\":{},\"domain\":{},\"action\":{},\"nonce\":{},\"issuedAt\":{},\"expiresAt\":{},\"publicKey\":{},\"tokenId\":{}}}",
            s(&self.challenge_id),
            s(&self.domain),
            s(&self.action),
            s(&self.nonce),
            self.created_at.timestamp(),
            self.expires_at.timestamp(),
            s(public_key),
            s(token_id),
        )
    }
}

/// Atomically marks a challenge as used. Returns false if it was already used.
pub async fn mark_qr_challenge_used(challenge_id: &str) -> bool {
    let mut used = QR_USED.lock().await;
    used.insert(challenge_id.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QrResult {
    pub status: String, // "pending", "verified", "failed"
    pub user_address: Option<String>,
    pub verified_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct QrStartRequest {
    pub domain: String,
    pub action: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct QrStartResponse {
    pub challenge: String,
    pub challenge_id: String,
    pub expires_at: String,
}

// The validate endpoint now accepts the full signed QR JSON payload produced by operators
// and forwarded by wallets. We forward the payload to the blockchain node which performs
// signature and DNS binding verification, then create a local session on success.

// POST /auth/qr/start - Create QR auth challenge
pub async fn start_qr_challenge(
    State(_state): State<AppState>,
    Json(req): Json<QrStartRequest>,
) -> Result<Json<QrStartResponse>, StatusCode> {
    let challenge_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::minutes(5);
    
    let challenge = QrChallenge {
        challenge_id: challenge_id.clone(),
        domain: req.domain,
        action: req.action.unwrap_or_else(|| "login".to_string()),
        nonce: Uuid::new_v4().to_string(),
        created_at: now,
        expires_at,
    };
    
    // Store challenge
    {
        let mut challenges = QR_CHALLENGES.lock().await;
        challenges.insert(challenge_id.clone(), challenge.clone());
    }
    
    // Initialize result as pending
    {
        let mut results = QR_RESULTS.lock().await;
        results.insert(challenge_id.clone(), QrResult {
            status: "pending".to_string(),
            user_address: None,
            verified_at: None,
        });
    }
    
    Ok(Json(QrStartResponse {
        challenge: challenge_id.clone(),
        challenge_id,
        expires_at: expires_at.to_rfc3339(),
    }))
}

// GET /auth/qr/status/:id - Check QR auth status
pub async fn get_qr_status(
    Path(challenge_id): Path<String>,
) -> Result<Json<QrResult>, StatusCode> {
    let results = QR_RESULTS.lock().await;
    match results.get(&challenge_id) {
        Some(result) => Ok(Json(result.clone())),
        None => Err(StatusCode::NOT_FOUND),
    }
}

// POST /auth/qr/validate - Validate signed QR response and create session
pub async fn validate_qr_auth(
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Expect the payload to include a challenge id so we can map back to the pending challenge
    let challenge_id = payload.get("challenge_id")
        .or_else(|| payload.get("challengeId"))
        .and_then(|v| v.as_str())
        .ok_or(StatusCode::BAD_REQUEST)?
        .to_string();

    // Ensure challenge exists and not expired
    let challenge = {
        let challenges = QR_CHALLENGES.lock().await;
        challenges.get(&challenge_id).cloned()
    };
    let challenge = challenge.ok_or(StatusCode::NOT_FOUND)?;
    if chrono::Utc::now() > challenge.expires_at {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Forward the signed QR payload to the blockchain node for verification and issuance.
    let blockchain_api = std::env::var("BLOCKCHAIN_API_BASE").unwrap_or_else(|_| "http://127.0.0.1:23457/api/v1".to_string());
    let client = reqwest::Client::new();

    let resp = client.post(format!("{}/auth-token/issue-by-qr", blockchain_api))
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
            error!("Failed to call blockchain issue-by-qr: {}", e);
            StatusCode::BAD_GATEWAY
        })?;

    if !resp.status().is_success() {
        // Mark result as failed
        {
            let mut results = QR_RESULTS.lock().await;
            results.insert(challenge_id.clone(), QrResult {
                status: "failed".to_string(),
                user_address: None,
                verified_at: Some(chrono::Utc::now()),
            });
        }
        let text = resp.text().await.unwrap_or_else(|_| "".to_string());
        error!("Blockchain rejected QR issuance: {}", text);
        return Ok(Json(serde_json::json!({"status":"failed","error":"blockchain-rejected","detail": text })));
    }

    // On success, create/get user by subject (wallet address) included in payload
    let subject = payload.get("subject").and_then(|v| v.as_str())
        .or_else(|| payload.get("address").and_then(|v| v.as_str()))
        .ok_or(StatusCode::BAD_REQUEST)?
        .to_string();

    // Create or get user by GhostID address - BYPASS WHITELIST for GhostID auth
    let user = match state.db.get_user_by_username(&subject).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            let user_id = Uuid::new_v4();
            let email = format!("{}@ghostid.wallet", subject);
            state.db.create_user_with_id(user_id, subject.clone(), email, UserRole::User).await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        }
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    // Generate JWT token using the auth service (same as OAuth)
    let claims = mothership_common::auth::Claims {
        sub: user.id.to_string(),
        machine_id: "ghostid-wallet".to_string(),
        username: user.username.clone(),
        email: Some(user.email.clone()),
        iat: chrono::Utc::now().timestamp(),
    exp: (chrono::Utc::now() + chrono::Duration::days(30)).timestamp(),
        aud: "mothership".to_string(),
        iss: "mothership-server".to_string(),
    };

    let token = state.auth.encode_token(&claims)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Update result to verified and include the user address
    {
        let mut results = QR_RESULTS.lock().await;
        results.insert(challenge_id.clone(), QrResult {
            status: "verified".to_string(),
            user_address: Some(subject.clone()),
            verified_at: Some(chrono::Utc::now()),
        });
    }

    // Clean up challenge
    {
        let mut challenges = QR_CHALLENGES.lock().await;
        challenges.remove(&challenge_id);
    }

    Ok(Json(serde_json::json!({
        "status": "verified",
        "token": token,
        "user": {
            "id": user.id,
            "username": user.username,
            "email": user.email
        }
    })))
}