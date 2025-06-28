use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use tracing::{info, warn};
use tower_http::services::ServeDir;

/// Web UI routes for authentication and CLI downloads
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/", get(index_page))
        .route("/login", get(login_page))
        .route("/download", get(download_page))
        .route("/download/authenticated", get(authenticated_download_page))
        .route("/robots.txt", get(robots_txt))
        // Serve static files (icon.png, etc.)
        .nest_service("/static", ServeDir::new("content"))
}

/// Serve robots.txt
async fn robots_txt() -> Result<Response<String>, StatusCode> {
    match std::fs::read_to_string("content/robots.txt") {
        Ok(content) => Ok(Response::builder()
            .header("content-type", "text/plain")
            .body(content)
            .unwrap()),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

#[derive(Deserialize)]
struct DownloadPageQuery {
    token: Option<String>,
    user: Option<String>,
    email: Option<String>,
}

/// Main index page
async fn index_page(State(state): State<crate::AppState>) -> Html<String> {
    let auth_required = state.config.cli_distribution.require_auth_for_downloads || state.whitelist.is_some();
    
    let html = format!(r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Mothership Server</title>
    <link rel="icon" type="image/png" href="/static/icon.png">
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            line-height: 1.6;
            margin: 0;
            padding: 2rem;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            color: white;
        }}
        
        .container {{
            max-width: 800px;
            margin: 0 auto;
            background: rgba(255, 255, 255, 0.1);
            padding: 3rem;
            border-radius: 20px;
            backdrop-filter: blur(10px);
            box-shadow: 0 20px 40px rgba(0, 0, 0, 0.2);
        }}
        
        h1 {{
            font-size: 3rem;
            margin-bottom: 1rem;
            text-align: center;
        }}
        
        .subtitle {{
            text-align: center;
            font-size: 1.2rem;
            opacity: 0.9;
            margin-bottom: 3rem;
        }}
        
        .features {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(250px, 1fr));
            gap: 2rem;
            margin: 3rem 0;
        }}
        
        .feature {{
            background: rgba(255, 255, 255, 0.1);
            padding: 2rem;
            border-radius: 15px;
            text-align: center;
        }}
        
        .feature h3 {{
            margin-bottom: 1rem;
            font-size: 1.5rem;
        }}
        
        .cta {{
            text-align: center;
            margin: 3rem 0;
        }}
        
        .btn {{
            display: inline-block;
            padding: 1rem 2rem;
            background: rgba(255, 255, 255, 0.2);
            color: white;
            text-decoration: none;
            border-radius: 10px;
            font-weight: bold;
            margin: 0.5rem;
            transition: all 0.3s ease;
            border: 2px solid rgba(255, 255, 255, 0.3);
        }}
        
        .btn:hover {{
            background: rgba(255, 255, 255, 0.3);
            transform: translateY(-2px);
        }}
        
        .btn-primary {{
            background: rgba(72, 187, 120, 0.8);
            border-color: rgba(72, 187, 120, 1);
        }}
        
        .warning {{
            background: rgba(245, 101, 101, 0.2);
            border: 2px solid rgba(245, 101, 101, 0.5);
            padding: 1rem;
            border-radius: 10px;
            margin: 2rem 0;
        }}
        
        .info {{
            background: rgba(66, 153, 225, 0.2);
            border: 2px solid rgba(66, 153, 225, 0.5);
            padding: 1rem;
            border-radius: 10px;
            margin: 2rem 0;
        }}
        
        code {{
            background: rgba(0, 0, 0, 0.3);
            padding: 0.2rem 0.5rem;
            border-radius: 5px;
            font-family: 'Monaco', 'Courier New', monospace;
        }}
        
        .code-block {{
            background: rgba(0, 0, 0, 0.4);
            padding: 1rem;
            border-radius: 10px;
            margin: 1rem 0;
            overflow-x: auto;
        }}
    </style>
</head>
<body>
    <div class="container">
        <div style="text-align: center; margin-bottom: 2rem;">
            <img src="/static/icon.png" alt="Mothership" style="height: 80px; width: auto; margin-bottom: 1rem;" />
            <h1>Mothership</h1>
        </div>
        <p class="subtitle">Collaborative Development Platform</p>
        
        <div class="features">
            <div class="feature">
                <h3>🔄 Real-time Sync</h3>
                <p>Collaborate on code in real-time with seamless file synchronization across your team.</p>
            </div>
            <div class="feature">
                <h3>💬 Live Chat</h3>
                <p>Built-in chat system for discussing changes and coordinating development efforts.</p>
            </div>
            <div class="feature">
                <h3>📦 CLI Tools</h3>
                <p>Powerful command-line interface for project management and deployment.</p>
            </div>
            <div class="feature">
                <h3>🔒 Secure Access</h3>
                <p>Enterprise-grade authentication and access controls for your team.</p>
            </div>
        </div>
        
        {}
        
        <div class="cta">
            <h2>Get Started</h2>
            <p>Download the Mothership CLI to begin collaborating with your team</p>
            {}
        </div>
    </div>
</body>
</html>
"#,
        if auth_required {
            r#"<div class="warning">
                <h3>🔐 Authentication Required</h3>
                <p>This server requires authentication to download CLI tools. Please sign in first to access the download page.</p>
            </div>"#
        } else {
            r#"<div class="info">
                <h3>🌐 Public Access</h3>
                <p>CLI downloads are publicly available. Authentication is required for server usage.</p>
            </div>"#
        },
        if auth_required {
            r#"<a href="/login" class="btn btn-primary">Sign In to Download CLI</a>"#
        } else {
            r#"<a href="/download" class="btn btn-primary">Download CLI</a>"#
        }
    );

    Html(html)
}

/// Login page that starts OAuth flow
async fn login_page(State(state): State<crate::AppState>) -> Result<Html<String>, StatusCode> {
    if !state.config.features.oauth_enabled {
        return Ok(Html(format!(r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Authentication Disabled - Mothership</title>
    <link rel="icon" type="image/png" href="/static/icon.png">
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            line-height: 1.6;
            margin: 0;
            padding: 2rem;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            color: white;
            display: flex;
            align-items: center;
            justify-content: center;
        }}
        
        .container {{
            max-width: 500px;
            background: rgba(255, 255, 255, 0.1);
            padding: 3rem;
            border-radius: 20px;
            backdrop-filter: blur(10px);
            text-align: center;
        }}
        
        .error {{
            background: rgba(245, 101, 101, 0.3);
            border: 2px solid rgba(245, 101, 101, 0.6);
            padding: 2rem;
            border-radius: 15px;
            margin: 2rem 0;
        }}
        
        .btn {{
            display: inline-block;
            padding: 1rem 2rem;
            background: rgba(255, 255, 255, 0.2);
            color: white;
            text-decoration: none;
            border-radius: 10px;
            font-weight: bold;
            margin: 1rem;
            transition: all 0.3s ease;
            border: 2px solid rgba(255, 255, 255, 0.3);
        }}
        
        .btn:hover {{
            background: rgba(255, 255, 255, 0.3);
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>❌ Authentication Disabled</h1>
        <div class="error">
            <h3>OAuth authentication is disabled on this server</h3>
            <p>Contact your administrator to enable OAuth authentication or use device code authentication via the CLI.</p>
        </div>
        <a href="/" class="btn">← Back to Home</a>
    </div>
</body>
</html>
"#)));
    }

    let html = format!(r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Sign In - Mothership</title>
    <link rel="icon" type="image/png" href="/static/icon.png">
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            line-height: 1.6;
            margin: 0;
            padding: 2rem;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            color: white;
            display: flex;
            align-items: center;
            justify-content: center;
        }}
        
        .container {{
            max-width: 500px;
            background: rgba(255, 255, 255, 0.1);
            padding: 3rem;
            border-radius: 20px;
            backdrop-filter: blur(10px);
            text-align: center;
        }}
        
        h1 {{
            font-size: 2.5rem;
            margin-bottom: 1rem;
        }}
        
        .subtitle {{
            font-size: 1.1rem;
            opacity: 0.9;
            margin-bottom: 3rem;
        }}
        
        .auth-options {{
            display: flex;
            flex-direction: column;
            gap: 1rem;
            margin: 2rem 0;
        }}
        
        .auth-btn {{
            display: flex;
            align-items: center;
            justify-content: center;
            padding: 1rem 2rem;
            background: rgba(255, 255, 255, 0.1);
            color: white;
            text-decoration: none;
            border-radius: 10px;
            font-weight: bold;
            transition: all 0.3s ease;
            border: 2px solid rgba(255, 255, 255, 0.3);
        }}
        
        .auth-btn:hover {{
            background: rgba(255, 255, 255, 0.2);
            transform: translateY(-2px);
        }}
        
        .auth-btn.google {{
            background: rgba(219, 68, 55, 0.8);
            border-color: rgba(219, 68, 55, 1);
        }}
        
        .auth-btn.github {{
            background: rgba(51, 51, 51, 0.8);
            border-color: rgba(51, 51, 51, 1);
        }}
        
        .back-link {{
            margin-top: 2rem;
        }}
        
        .back-link a {{
            color: rgba(255, 255, 255, 0.8);
            text-decoration: none;
        }}
        
        .back-link a:hover {{
            color: white;
        }}
    </style>
</head>
<body>
    <div class="container">
        <div style="text-align: center; margin-bottom: 2rem;">
            <img src="/static/icon.png" alt="Mothership" style="height: 60px; width: auto; margin-bottom: 1rem;" />
            <h1>🔐 Sign In</h1>
        </div>
        <p class="subtitle">Choose your authentication method to access CLI downloads</p>
        
        <div class="auth-options">
            <button class="auth-btn google" onclick="startOAuth('google')">
                📧 Continue with Google
            </button>
            <button class="auth-btn github" onclick="startOAuth('github')">
                🐙 Continue with GitHub
            </button>
            <button class="auth-btn" onclick="testOAuth()" style="background: rgba(100, 100, 100, 0.8);">
                🔍 Test OAuth Setup
            </button>
        </div>
        
        <div class="back-link">
            <a href="/">← Back to Home</a>
        </div>
    </div>
    
    <script>
        async function startOAuth(provider) {{
            try {{
                console.log('Starting OAuth for provider:', provider);
                
                const response = await fetch('/auth/oauth/start', {{
                    method: 'POST',
                    headers: {{
                        'Content-Type': 'application/json',
                    }},
                    body: JSON.stringify({{
                        provider: provider === 'google' ? 'Google' : 'GitHub', // Proper enum case
                        machine_id: 'web-' + Math.random().toString(36).substr(2, 9),
                        machine_name: 'web-browser-oauth',
                        platform: navigator.platform || 'unknown',
                        hostname: window.location.hostname
                    }})
                }});
                
                console.log('Response status:', response.status);
                console.log('Response headers:', response.headers);
                
                if (!response.ok) {{
                    const errorText = await response.text();
                    console.error('Server error response:', errorText);
                    alert('Server error (' + response.status + '): ' + errorText.substring(0, 200));
                    return false;
                }}
                
                const contentType = response.headers.get('content-type');
                if (!contentType || !contentType.includes('application/json')) {{
                    const responseText = await response.text();
                    console.error('Non-JSON response:', responseText);
                    alert('Server returned non-JSON response: ' + responseText.substring(0, 200));
                    return false;
                }}
                
                const data = await response.json();
                console.log('OAuth response data:', data);
                
                if (data.success && data.data && data.data.auth_url) {{
                    console.log('Redirecting to:', data.data.auth_url);
                    window.location.href = data.data.auth_url;
                }} else {{
                    console.error('Invalid response structure:', data);
                    alert('Failed to start authentication: ' + (data.error || JSON.stringify(data)));
                }}
            }} catch (error) {{
                console.error('JavaScript error:', error);
                alert('Error starting authentication: ' + error.message);
            }}
            return false;
        }}
        
        async function testOAuth() {{
            try {{
                const response = await fetch('/auth/oauth/test');
                const data = await response.json();
                
                if (data.success) {{
                    console.log('OAuth test results:', data.data);
                    
                    let message = 'OAuth Configuration Status:\\n\\n';
                    message += `OAuth Enabled: ${{data.data.oauth_enabled}}\\n`;
                    message += `Google Client ID: ${{data.data.google_client_id_set ? 'SET' : 'NOT SET'}}\\n`;
                    message += `Google Client Secret: ${{data.data.google_client_secret_set ? 'SET' : 'NOT SET'}}\\n`;
                    message += `GitHub Client ID: ${{data.data.github_client_id_set ? 'SET' : 'NOT SET'}}\\n`;
                    message += `GitHub Client Secret: ${{data.data.github_client_secret_set ? 'SET' : 'NOT SET'}}\\n`;
                    
                    if (!data.data.oauth_enabled) {{
                        message += '\\n❌ OAuth is disabled in server config!';
                    }} else if (!data.data.google_client_id_set || !data.data.google_client_secret_set) {{
                        message += '\\n⚠️ Google OAuth credentials missing!';
                        message += '\\nSet GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET environment variables.';
                    }} else {{
                        message += '\\n✅ OAuth should be working!';
                    }}
                    
                    alert(message);
                }} else {{
                    alert('OAuth test failed: ' + (data.error || 'Unknown error'));
                }}
            }} catch (error) {{
                console.error('OAuth test error:', error);
                alert('OAuth test error: ' + error.message);
            }}
        }}
    </script>
</body>
</html>
"#);

    Ok(Html(html))
}

/// Public download page (when auth not required)
async fn download_page(State(state): State<crate::AppState>) -> Html<String> {
    let auth_required = state.config.cli_distribution.require_auth_for_downloads || state.whitelist.is_some();
    
    if auth_required {
        return Html(format!(r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Authentication Required - Mothership</title>
    <link rel="icon" type="image/png" href="/static/icon.png">
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            line-height: 1.6;
            margin: 0;
            padding: 2rem;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            color: white;
            display: flex;
            align-items: center;
            justify-content: center;
        }}
        
        .container {{
            max-width: 500px;
            background: rgba(255, 255, 255, 0.1);
            padding: 3rem;
            border-radius: 20px;
            backdrop-filter: blur(10px);
            text-align: center;
        }}
        
        .warning {{
            background: rgba(245, 101, 101, 0.3);
            border: 2px solid rgba(245, 101, 101, 0.6);
            padding: 2rem;
            border-radius: 15px;
            margin: 2rem 0;
        }}
        
        .btn {{
            display: inline-block;
            padding: 1rem 2rem;
            background: rgba(72, 187, 120, 0.8);
            color: white;
            text-decoration: none;
            border-radius: 10px;
            font-weight: bold;
            margin: 1rem;
            transition: all 0.3s ease;
            border: 2px solid rgba(72, 187, 120, 1);
        }}
        
        .btn:hover {{
            background: rgba(72, 187, 120, 1);
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>🔐 Authentication Required</h1>
        <div class="warning">
            <h3>This server requires authentication</h3>
            <p>To download CLI tools, you must first sign in with your authorized account.</p>
        </div>
        <a href="/login" class="btn">Sign In</a>
        <a href="/" class="btn" style="background: rgba(255, 255, 255, 0.2); border-color: rgba(255, 255, 255, 0.3);">← Back to Home</a>
    </div>
</body>
</html>
        "#));
    }

    // Public download page
    generate_download_page_html(None, None, None, &state).await
}

/// Authenticated download page (after successful OAuth)
async fn authenticated_download_page(
    Query(query): Query<DownloadPageQuery>,
    State(state): State<crate::AppState>,
) -> Result<Html<String>, StatusCode> {
    // This page is typically reached via OAuth callback redirect
    let token = query.token.clone();
    let username = query.user.clone();
    let email = query.email.clone();
    
    if token.is_none() {
        warn!("Authenticated download page accessed without token");
        return Ok(Html(format!(r#"
<!DOCTYPE html>
<html>
<head><title>Missing Token</title></head>
<body>
    <h1>Missing Authentication Token</h1>
    <p>Please <a href="/login">sign in again</a>.</p>
</body>
</html>
        "#)));
    }
    
    info!("Authenticated download page accessed by user: {:?} ({:?})", username, email);
    
    Ok(generate_download_page_html(token, username, email, &state).await)
}

/// Generate the download page HTML
async fn generate_download_page_html(
    token: Option<String>,
    username: Option<String>,
    email: Option<String>,
    _state: &crate::AppState,
) -> Html<String> {
    let server_url = std::env::var("OAUTH_BASE_URL")
        .or_else(|_| std::env::var("MOTHERSHIP_SERVER_URL"))
        .unwrap_or_else(|_| "http://localhost:7523".to_string());
    
    let is_authenticated = token.is_some();
    let auth_header = if let Some(ref t) = token {
        format!("-H \"Authorization: Bearer {}\"", t)
    } else {
        String::new()
    };
    
    let version = env!("CARGO_PKG_VERSION");
    
    // Pre-compute auth header strings to avoid temporary value issues
    
    // Pre-compute formatted strings for download links
    let auth_attr_linux_x64_cli = if is_authenticated { format!(" {}", auth_header) } else { String::new() };
    let auth_attr_linux_x64_daemon = if is_authenticated { format!(" {}", auth_header) } else { String::new() };
    let auth_attr_linux_arm64_cli = if is_authenticated { format!(" {}", auth_header) } else { String::new() };
    let auth_attr_linux_arm64_daemon = if is_authenticated { format!(" {}", auth_header) } else { String::new() };
    let auth_attr_macos_x64_cli = if is_authenticated { format!(" {}", auth_header) } else { String::new() };
    let auth_attr_macos_x64_daemon = if is_authenticated { format!(" {}", auth_header) } else { String::new() };
    let auth_attr_macos_arm64_cli = if is_authenticated { format!(" {}", auth_header) } else { String::new() };
    let auth_attr_macos_arm64_daemon = if is_authenticated { format!(" {}", auth_header) } else { String::new() };
    let auth_attr_windows_cli = if is_authenticated { format!(" {}", auth_header) } else { String::new() };
    let auth_attr_windows_daemon = if is_authenticated { format!(" {}", auth_header) } else { String::new() };
    
    let html = format!(r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Download CLI - Mothership</title>
    <link rel="icon" type="image/png" href="/static/icon.png">
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            line-height: 1.6;
            margin: 0;
            padding: 2rem;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            color: white;
        }}
        
        .container {{
            max-width: 900px;
            margin: 0 auto;
            background: rgba(255, 255, 255, 0.1);
            padding: 3rem;
            border-radius: 20px;
            backdrop-filter: blur(10px);
        }}
        
        h1 {{
            font-size: 2.5rem;
            text-align: center;
            margin-bottom: 1rem;
        }}
        
        .user-info {{
            background: rgba(72, 187, 120, 0.2);
            border: 2px solid rgba(72, 187, 120, 0.5);
            padding: 1rem;
            border-radius: 10px;
            margin: 2rem 0;
            text-align: center;
        }}
        
        .download-methods {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
            gap: 2rem;
            margin: 3rem 0;
        }}
        
        .method {{
            background: rgba(255, 255, 255, 0.1);
            padding: 2rem;
            border-radius: 15px;
        }}
        
        .method h3 {{
            margin-bottom: 1rem;
            color: #48bb78;
        }}
        
        .code-block {{
            background: rgba(0, 0, 0, 0.4);
            padding: 1rem;
            border-radius: 10px;
            font-family: 'Monaco', 'Courier New', monospace;
            font-size: 0.9rem;
            overflow-x: auto;
            margin: 1rem 0;
            position: relative;
        }}
        
        .copy-btn {{
            position: absolute;
            top: 10px;
            right: 10px;
            background: rgba(72, 187, 120, 0.8);
            border: none;
            color: white;
            padding: 0.5rem;
            border-radius: 5px;
            cursor: pointer;
            font-size: 0.8rem;
        }}
        
        .copy-btn:hover {{
            background: rgba(72, 187, 120, 1);
        }}
        
        .platform-downloads {{
            margin: 3rem 0;
        }}
        
        .platforms {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 1rem;
            margin: 2rem 0;
        }}
        
        .platform {{
            background: rgba(255, 255, 255, 0.1);
            padding: 1.5rem;
            border-radius: 10px;
            text-align: center;
        }}
        
        .download-btn {{
            display: inline-block;
            padding: 0.8rem 1.5rem;
            background: rgba(72, 187, 120, 0.8);
            color: white;
            text-decoration: none;
            border-radius: 8px;
            font-weight: bold;
            margin: 0.5rem;
            transition: all 0.3s ease;
        }}
        
        .download-btn:hover {{
            background: rgba(72, 187, 120, 1);
            transform: translateY(-2px);
        }}
        
        .warning {{
            background: rgba(245, 101, 101, 0.2);
            border: 2px solid rgba(245, 101, 101, 0.5);
            padding: 1rem;
            border-radius: 10px;
            margin: 2rem 0;
        }}
        
        .note {{
            background: rgba(66, 153, 225, 0.2);
            border: 2px solid rgba(66, 153, 225, 0.5);
            padding: 1rem;
            border-radius: 10px;
            margin: 2rem 0;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>📦 Download Mothership CLI</h1>
        
        {}
        
        <div class="download-methods">
            <div class="method">
                <h3>🚀 Quick Install (Unix/Linux/macOS)</h3>
                <p>One-liner installation script:</p>
                <div class="code-block">
                    <button class="copy-btn" onclick="copyToClipboard('unix-install')">Copy</button>
                    <code id="unix-install">{}curl -sSL {}/cli/install | bash</code>
                </div>
            </div>
            
            <div class="method">
                <h3>🪟 Windows Installation</h3>
                <p>PowerShell installation script:</p>
                <div class="code-block">
                    <button class="copy-btn" onclick="copyToClipboard('windows-install')">Copy</button>
                    <code id="windows-install">{}irm {}/cli/install/windows | iex</code>
                </div>
            </div>
        </div>
        
        <div class="platform-downloads">
            <h2>💾 Direct Downloads</h2>
            <p>Download specific binaries for your platform:</p>
            
            <div class="platforms">
                <div class="platform">
                    <h4>🐧 Linux x64</h4>
                    <a href="{}/cli/download/{}/x86_64-unknown-linux-gnu/mothership" class="download-btn"{}>CLI</a>
                    <a href="{}/cli/download/{}/x86_64-unknown-linux-gnu/mothership-daemon" class="download-btn"{}>Daemon</a>
                </div>
                
                <div class="platform">
                    <h4>🐧 Linux ARM64</h4>
                    <a href="{}/cli/download/{}/aarch64-unknown-linux-gnu/mothership" class="download-btn"{}>CLI</a>
                    <a href="{}/cli/download/{}/aarch64-unknown-linux-gnu/mothership-daemon" class="download-btn"{}>Daemon</a>
                </div>
                
                <div class="platform">
                    <h4>🍎 macOS x64</h4>
                    <a href="{}/cli/download/{}/x86_64-apple-darwin/mothership" class="download-btn"{}>CLI</a>
                    <a href="{}/cli/download/{}/x86_64-apple-darwin/mothership-daemon" class="download-btn"{}>Daemon</a>
                </div>
                
                <div class="platform">
                    <h4>🍎 macOS ARM64</h4>
                    <a href="{}/cli/download/{}/aarch64-apple-darwin/mothership" class="download-btn"{}>CLI</a>
                    <a href="{}/cli/download/{}/aarch64-apple-darwin/mothership-daemon" class="download-btn"{}>Daemon</a>
                </div>
                
                <div class="platform">
                    <h4>🪟 Windows x64</h4>
                    <a href="{}/cli/download/{}/x86_64-pc-windows-msvc/mothership.exe" class="download-btn"{}>CLI</a>
                    <a href="{}/cli/download/{}/x86_64-pc-windows-msvc/mothership-daemon.exe" class="download-btn"{}>Daemon</a>
                </div>
            </div>
        </div>
        
        <div class="note">
            <h3>📋 Next Steps</h3>
            <ol>
                <li>Download and install the CLI using one of the methods above</li>
                <li>Run <code>mothership auth</code> to authenticate with this server</li>
                <li>Use <code>mothership --help</code> to see all available commands</li>
                <li>Run <code>mothership update</code> to check for updates</li>
            </ol>
        </div>
        
        {}
    </div>
    
    <script>
        function copyToClipboard(elementId) {{
            const element = document.getElementById(elementId);
            const text = element.textContent;
            navigator.clipboard.writeText(text).then(() => {{
                const btn = element.parentElement.querySelector('.copy-btn');
                const originalText = btn.textContent;
                btn.textContent = 'Copied!';
                setTimeout(() => {{
                    btn.textContent = originalText;
                }}, 2000);
            }});
        }}
    </script>
</body>
</html>
"#,
        if is_authenticated {
            format!(r#"<div class="user-info">
                <h3>✅ Authenticated as {}</h3>
                <p>Email: {}</p>
                <p>You have access to download all CLI tools.</p>
            </div>"#,
                username.as_deref().unwrap_or("Unknown"),
                email.as_deref().unwrap_or("Unknown")
            )
        } else {
            String::new()
        },
        if is_authenticated { 
            format!("MOTHERSHIP_TOKEN={} ", token.as_ref().unwrap()) 
        } else { 
            String::new() 
        },
        server_url,
        if is_authenticated { 
            format!("$env:MOTHERSHIP_TOKEN=\"{}\"; ", token.as_ref().unwrap()) 
        } else { 
            String::new() 
        },
        server_url,
        // Platform downloads with auth headers
        server_url, version, &auth_attr_linux_x64_cli,
        server_url, version, &auth_attr_linux_x64_daemon,
        server_url, version, &auth_attr_linux_arm64_cli,
        server_url, version, &auth_attr_linux_arm64_daemon,
        server_url, version, &auth_attr_macos_x64_cli,
        server_url, version, &auth_attr_macos_x64_daemon,
        server_url, version, &auth_attr_macos_arm64_cli,
        server_url, version, &auth_attr_macos_arm64_daemon,
        server_url, version, &auth_attr_windows_cli,
        server_url, version, &auth_attr_windows_daemon,
        if is_authenticated {
            r#"<div class="warning">
                <h3>🔒 Secure Token</h3>
                <p>Your authentication token is embedded in the download links above. Keep this page secure and don't share the URLs with others.</p>
            </div>"#
        } else {
            r#"<div class="note">
                <h3>🌐 Public Downloads</h3>
                <p>These downloads are publicly available. You'll still need to authenticate when using the CLI to access this server.</p>
            </div>"#
        }
    );

    Html(html)
} 