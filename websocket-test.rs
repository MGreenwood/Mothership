use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    println!("🔍 Testing WebSocket connection to Mothership server...");
    
    // Use the exact same token from the credentials
    let token = "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJkNGFhZmU5MS0xMzVkLTQ5YTgtODhhMC03MzQ5ZDY4Yjk0MTMiLCJtYWNoaW5lX2lkIjoid2ViLW9hdXRoIiwidXNlcm5hbWUiOiJtZ3JlZW53b29kZGV2IiwiZW1haWwiOiJtZ3JlZW53b29kLmRldkBnbWFpbC5jb20iLCJpYXQiOjE3NTEwNTQyNTIsImV4cCI6MTc1MzY0NjI1MiwiYXVkIjoibW90aGVyc2hpcCIsImlzcyI6Im1vdGhlcnNoaXAtc2VydmVyIn0.WqrXnL9ywd3PuO72iXf1Oi3e7XKlw7CmR5Ba6pL2BRk";
    let rift_id = "4b6592b6-e72c-458c-b995-ff80042d4c93";
    
    // Construct the exact same WebSocket URL the daemon uses
    let encoded_token = urlencoding::encode(token);
    let ws_url = format!("wss://api.mothershipproject.dev/sync/{}?token={}", rift_id, encoded_token);
    
    println!("🔌 Connecting to: {}", ws_url.replace(&encoded_token.to_string(), "***TOKEN***"));
    
    // Try to connect
    match connect_async(&ws_url).await {
        Ok((mut ws_stream, response)) => {
            println!("✅ Successfully connected to WebSocket!");
            println!("📋 Response status: {:?}", response.status());
            println!("📋 Response headers: {:?}", response.headers());
            
            // Send a test message
            let test_message = r#"{"type":"FileChanged","file_path":"test.txt","content":"test content","rift_id":"4b6592b6-e72c-458c-b995-ff80042d4c93"}"#;
            let message = Message::Text(test_message.to_string());
            
            if let Err(e) = ws_stream.send(message).await {
                println!("❌ Failed to send message: {}", e);
            } else {
                println!("📤 Successfully sent test message");
            }
            
            // Close connection
            ws_stream.close(None).await?;
            println!("🔌 Connection closed gracefully");
        }
        Err(e) => {
            println!("❌ WebSocket connection failed!");
            println!("🔍 Error: {}", e);
            println!("🔍 Error details: {:?}", e);
            
            // Analyze the error
            let error_str = e.to_string().to_lowercase();
            if error_str.contains("certificate") || error_str.contains("tls") || error_str.contains("ssl") {
                println!("🔒 SSL/TLS certificate issue detected");
                println!("💡 This is likely a certificate validation problem with Cloudflare");
            } else if error_str.contains("connection") {
                println!("🌐 Network connection issue");
            } else if error_str.contains("handshake") {
                println!("🤝 WebSocket handshake failed");
            } else if error_str.contains("timeout") {
                println!("⏰ Connection timeout");
            } else {
                println!("❓ Unknown connection error");
            }
            
            return Err(e.into());
        }
    }
    
    Ok(())
} 