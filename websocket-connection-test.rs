use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCredentials {
    access_token: String,
    user_email: Option<String>,
    user_name: Option<String>,
    stored_at: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("🔍 Testing WebSocket connection exactly like the daemon...");
    
    // Load OAuth token from the same location as daemon
    let credentials_path = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?
        .join("mothership")
        .join("credentials.json");
    
    if !credentials_path.exists() {
        return Err(anyhow::anyhow!("Credentials file not found at: {}", credentials_path.display()));
    }
    
    let credentials_content = std::fs::read_to_string(&credentials_path)?;
    let credentials: StoredCredentials = serde_json::from_str(&credentials_content)?;
    
    println!("✅ Loaded OAuth token ({} chars)", credentials.access_token.len());
    
    // Use the exact values from the project
    let server_url = "https://api.mothershipproject.dev";
    let rift_id = "98478276-10a5-4cfa-a673-f588a315c6a3";
    
    // Construct WebSocket URL exactly like the daemon
    let ws_url = if server_url.starts_with("https://") {
        let ws_base = server_url.replace("https://", "wss://");
        format!("{}/sync/{}?token={}", ws_base, rift_id, urlencoding::encode(&credentials.access_token))
    } else if server_url.starts_with("http://") {
        let ws_base = server_url.replace("http://", "ws://");
        format!("{}/sync/{}?token={}", ws_base, rift_id, urlencoding::encode(&credentials.access_token))
    } else {
        format!("wss://{}/sync/{}?token={}", server_url, rift_id, urlencoding::encode(&credentials.access_token))
    };
    
    println!("🔌 Connecting to: {}", ws_url.replace(&urlencoding::encode(&credentials.access_token).to_string(), "***TOKEN***"));
    
    // Try to connect
    match connect_async(&ws_url).await {
        Ok((mut ws_stream, response)) => {
            println!("✅ WebSocket connection successful!");
            println!("📋 Response status: {}", response.status());
            println!("🔐 Connection authenticated and established");
            
            // Send a test message
            let test_message = Message::Text("Test connection from manual test".to_string());
            if let Err(e) = ws_stream.send(test_message).await {
                println!("⚠️ Failed to send test message: {}", e);
            } else {
                println!("📤 Test message sent successfully");
            }
        }
        Err(e) => {
            println!("❌ WebSocket connection failed: {}", e);
            println!("🔍 Error details: {:?}", e);
            
            // Analyze the error
            let error_str = e.to_string().to_lowercase();
            if error_str.contains("certificate") || error_str.contains("tls") || error_str.contains("ssl") {
                println!("🔒 SSL/TLS certificate issue detected");
                println!("💡 This is likely the same issue affecting the daemon");
            } else if error_str.contains("connection") {
                println!("🌐 Network connection issue");
            } else if error_str.contains("handshake") {
                println!("🤝 WebSocket handshake failed - possible authentication issue");
            } else if error_str.contains("timeout") {
                println!("⏰ Connection timeout");
            } else {
                println!("❓ Unknown connection error");
            }
            
            return Err(e);
        }
    }
    
    Ok(())
} 