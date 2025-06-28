# Mothership Server Configuration

The Mothership server uses a flexible configuration system that allows you to customize behavior, enable/disable features, and control access. This document explains all available configuration options.

## Quick Start

1. **Copy example config**: Copy `server.config.example` to `server.config`
2. **Customize settings**: Edit the config file to match your needs
3. **Start server**: The server will automatically load the configuration

If no `server.config` file exists, the server will create one with default settings.

## Configuration Formats

The server supports two configuration formats:

### TOML Format (Recommended)
```toml
[server]
host = "0.0.0.0"
port = 7523

[features]
chat_enabled = true
oauth_enabled = false
```

### Simple Key=Value Format (Legacy)
```
host=0.0.0.0
port=7523
chat_enabled=true
oauth_enabled=false
```

## Configuration Sections

### `[server]` - Basic Server Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `host` | `"0.0.0.0"` | Server bind address. Use `127.0.0.1` for localhost only |
| `port` | `7523` | API server port |
| `web_port` | `None` | Web UI port (optional - if set, web UI runs on separate port) |
| `max_connections` | `1000` | Maximum concurrent connections |
| `request_timeout` | `30` | Request timeout in seconds |
| `debug_logging` | `false` | Enable detailed debug logging |

### `[features]` - Feature Toggles

| Setting | Default | Description |
|---------|---------|-------------|
| `chat_enabled` | `true` | Enable real-time chat in rifts |
| `file_uploads_enabled` | `true` | Enable file sharing via uploads |
| `project_creation_enabled` | `true` | Allow users to create new projects |
| `cli_distribution_enabled` | `true` | Enable CLI download endpoints |
| `oauth_enabled` | `true` | Enable OAuth authentication (Google/GitHub) |
| `websocket_sync_enabled` | `true` | Enable WebSocket real-time sync |

### `[auth]` - Authentication & Access Control

| Setting | Default | Description |
|---------|---------|-------------|
| `whitelist_enabled` | `false` | Enable user whitelist (only whitelisted users can access) |
| `whitelist_path` | `"whitelist.config"` | Path to whitelist file |
| `require_auth` | `true` | Require authentication for all endpoints |
| `token_expiration_days` | `30` | JWT token expiration time in days |
| `max_login_attempts` | `5` | Max failed login attempts before temporary ban |
| `ban_duration_minutes` | `15` | Temporary ban duration in minutes |

### `[collaboration]` - Real-time Collaboration

| Setting | Default | Description |
|---------|---------|-------------|
| `max_users_per_rift` | `50` | Maximum users per collaboration session |
| `max_chat_message_length` | `1000` | Maximum chat message length |
| `store_chat_history` | `true` | Store chat message history |
| `max_chat_history` | `1000` | Maximum stored chat messages per rift |
| `presence_enabled` | `true` | Show who's online/offline |
| `presence_update_interval` | `30` | Presence update interval in seconds |

### `[cli_distribution]` - CLI Distribution System

| Setting | Default | Description |
|---------|---------|-------------|
| `binaries_path` | `"cli-binaries"` | Directory containing CLI binaries |
| `require_auth_for_downloads` | `true` | Require authentication for downloads |
| `max_downloads_per_hour` | `100` | Rate limit for downloads per user |
| `track_downloads` | `true` | Enable download analytics |

## Server Deployment Modes

The Mothership server can be deployed in different modes depending on your infrastructure needs:

### Single Port Mode (Default)
```toml
[server]
port = 7523
# web_port not set - everything on one port
```
- **API + Web UI**: Both on `http://server:7523/`
- **Web interface**: `http://server:7523/` (landing page)
- **API endpoints**: `http://server:7523/auth/*`, `http://server:7523/projects/*`, etc.
- **Best for**: Simple deployments, development, small teams

### Dual Port Mode
```toml
[server]
port = 7523      # API server
web_port = 8080  # Web UI server
```
- **API only**: `http://server:7523/` (API endpoints only)
- **Web UI only**: `http://server:8080/` (authentication, downloads)
- **Best for**: Production deployments, reverse proxy setups, security separation

### Benefits of Dual Port Mode
- **Security isolation**: Different firewall rules for API vs Web
- **Reverse proxy friendly**: Easy to route API and Web through different paths
- **Load balancing**: Can scale API and Web servers independently
- **CDN integration**: Web UI can be cached/CDN'd separately from API

## Web Authentication Flow

The Mothership server includes a built-in web interface for user authentication and CLI downloads. This solves the chicken-and-egg problem where users need CLI to authenticate, but need authentication to download CLI.

### Authentication Flow

1. **Visit Web Interface**: Users go to `http://server:7523/`
2. **Click "Sign In"**: Redirects to `/login` page
3. **Choose OAuth Provider**: Google or GitHub authentication
4. **Complete OAuth**: Standard OAuth2 flow in browser
5. **Download Page**: Redirected to `/download/authenticated` with embedded tokens
6. **One-Click Install**: Pre-authenticated download links and install scripts

### Web UI Endpoints

- `/` - Main landing page with server information
- `/login` - OAuth provider selection page  
- `/download` - Public download page (if auth not required)
- `/download/authenticated` - Authenticated download page with tokens
- `/auth/oauth/start` - Start OAuth flow (API endpoint)
- `/auth/callback/*` - OAuth callback handlers

### Security Features

- **Token Embedding**: Download links include temporary auth tokens
- **Whitelist Integration**: Web auth respects user whitelist settings
- **Secure Redirects**: OAuth callbacks validate state parameters
- **Session Security**: Tokens are scoped and time-limited

## User Whitelist

When `whitelist_enabled = true`, only users in the whitelist file can access the server.

### Whitelist Format

The whitelist file supports three types of entries:

```
# Exact usernames
alice
bob_developer
admin

# Exact email addresses
john@company.com
jane@partner.org

# Email domains (all users from domain)
@mycompany.com
@university.edu
```

### Whitelist Examples

**Corporate Deployment**:
```
# Team members
@mycompany.com
external-consultant@partner.com
```

**Educational Deployment**:
```
# Students and faculty
@university.edu
@staff.university.edu
guest-researcher@external.org
```

**Small Team**:
```
alice
bob
charlie@freelancer.com
```

## Common Configuration Scenarios

### 1. Corporate/Enterprise Deployment
```toml
[features]
chat_enabled = false  # Focus on code, not chat
oauth_enabled = true  # Required for web auth flow
cli_distribution_enabled = true

[auth]
whitelist_enabled = true
whitelist_path = "company-team.config"

[cli_distribution]
require_auth_for_downloads = true  # Secure downloads

[collaboration]
max_users_per_rift = 10  # Small teams
```

**Installation Flow for Corporate Deployment:**
1. Users visit `https://server.company.com/login`
2. Complete OAuth authentication (Google/GitHub)
3. Get redirected to authenticated download page
4. Download CLI with embedded authentication tokens

### 2. Public Beta/Demo Server
```toml
[features]
chat_enabled = true
oauth_enabled = true
project_creation_enabled = false  # Admin creates projects

[auth]
whitelist_enabled = true  # Invited users only
whitelist_path = "beta-testers.config"

[collaboration]
max_users_per_rift = 5  # Small demo sessions
```

### 3. Local Development
```toml
[server]
host = "127.0.0.1"  # Localhost only
debug_logging = true

[features]
oauth_enabled = false  # Skip OAuth setup
cli_distribution_enabled = false

[auth]
whitelist_enabled = false
require_auth = false  # Easy testing
```

### 4. CLI-Only Deployment
```toml
[features]
chat_enabled = false
oauth_enabled = false
websocket_sync_enabled = false
cli_distribution_enabled = true

[auth]
whitelist_enabled = true
whitelist_path = "cli-users.config"
```

## Environment Variable Overrides

Some settings can be overridden with environment variables:

- `MOTHERSHIP_PORT` - Override the port setting
- `JWT_SECRET` - Set JWT signing secret
- `DATABASE_URL` - Set database connection
- `GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET` - OAuth credentials

## Security Considerations

### Production Deployment Checklist

- [ ] Set `whitelist_enabled = true` for private deployments
- [ ] Use `host = "127.0.0.1"` if server should only be local
- [ ] Set `debug_logging = false` to reduce log verbosity
- [ ] Configure appropriate `max_connections` for your server
- [ ] Set strong `JWT_SECRET` environment variable
- [ ] Review enabled features - disable unused ones
- [ ] Set up proper SSL/TLS reverse proxy (nginx, caddy, etc.)

### Whitelist Best Practices

- Use email domains (`@company.com`) for organization-wide access
- Combine domains with specific external users
- Regularly review and update the whitelist
- Use comments in the whitelist file to document users
- Keep whitelist files secure and version controlled

## Troubleshooting

### Server won't start
- Check `server.config` syntax (TOML format)
- Verify port is not in use
- Check host address is valid IP
- Review server logs for specific error messages

### Authentication issues
- Verify OAuth environment variables are set
- Check whitelist file exists and has correct format
- Ensure users are in whitelist (if enabled)
- Check JWT_SECRET is consistent across restarts

### Features not working
- Verify feature is enabled in config
- Check server logs for feature-specific messages
- Ensure required dependencies are available
- Test with minimal configuration first

## Configuration Validation

The server validates configuration on startup and will:
- Create default config if none exists
- Log warnings for invalid settings
- Use defaults for missing settings
- Report configuration loading status

Watch the server startup logs for configuration messages:
```
📋 Server config loaded from: server.config
✅ Server configuration loaded
✅ User whitelist loaded and active
🔐 OAuth endpoints enabled
🔄 WebSocket sync enabled
📦 CLI distribution endpoints enabled
``` 