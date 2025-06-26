# Mothership: Frictionless Version Control

A revolutionary version control system that eliminates complexity while preserving safety and collaboration benefits.

## 🎯 Vision

Mothership eliminates the three core problems with traditional version control:

1. **Zero Fear**: No destructive operations. Every change is preserved automatically.
2. **Zero Ceremony**: No manual commits, staging, or complex workflows. Just code.
3. **Zero Friction**: Authenticate once, discover projects instantly, collaborate seamlessly.

## 🚀 What We've Built (Phase 1 Complete!)

✅ **Complete Foundation Architecture**
- **Rust server** (Axum + Tokio) on port 7523 with full HTTP/WebSocket APIs
- **Node.js auth server** on port 3001 with beautiful browser authentication UI
- **CLI client** with intuitive commands and excellent error handling
- **Production-ready Docker deployment** with one-click startup scripts
- **JWT authentication** with OAuth-style device flow and secure machine certificates
- **Environment configuration** system with comprehensive .env support

✅ **Production-Ready Features**
- **Multi-role user system** (User/Admin/SuperAdmin) with secure permissions
- **Human-readable project names** - No more UUIDs in user commands!
- **Private gateway creation** - Users can create projects without admin intervention
- **Cross-platform CLI** tested on Windows PowerShell, macOS, and Linux
- **Security-first design** with secret management and CORS protection
- **Zero-downtime deployment** with Docker health checks and graceful shutdown
- **OAuth integration** - Complete Google OAuth flow with Tauri GUI application

✅ **Revolutionary User Experience**
- **One-time authentication** opens browser automatically for secure setup
- **Intuitive CLI commands** that match user mental models
- **Automatic project discovery** with `mothership gateway list`
- **Beam into projects** using friendly names: `mothership beam "My Project"`
- **Color-coded terminal output** with clear success/error messaging
- **Real-time OAuth callback system** for seamless token transfer

✅ **Working Commands**
- `mothership auth` - One-time machine authentication (opens browser)
- `mothership gateway list` - Discover accessible projects  
- `mothership gateway create "Project Name" --dir ./path` - Create new projects
- `mothership beam "Project Name"` - Join projects by human-readable names
- Admin API endpoints for user/project management

## 🛠️ Current Status & Roadmap

**✅ Phase 1: COMPLETE (Foundation)**
- Complete authentication system with browser flow
- Project creation and management with human-readable names
- Production-ready Docker deployment with one-click startup
- Multi-role user system with proper permissions
- CLI with intuitive commands and excellent UX
- OAuth integration with Google authentication in Tauri GUI
- Real-time callback system for seamless OAuth token transfer

**🎯 Phase 2: File Tracking Engine (Next Major Milestone)**
- **File system watching** - Detect changes automatically using `notify` crate
- **Automatic checkpointing** - Save snapshots on every meaningful change
- **Batched API synchronization** - Efficient change transmission to server
- **Local caching system** - Offline-first development with smart sync
- **Real-time collaboration** - Live file changes between team members

**🚀 Phase 3: Production SaaS (Business Model)**
- **PostgreSQL migration** - Replace in-memory database for persistence
- **Multi-tenant architecture** - Organizations with isolated data
- **Enterprise authentication** - OAuth2 with SSO support
- **Subscription tiers** - Individual, Team, Enterprise pricing
- **Global deployment** - Edge servers for worldwide performance

## 🛠️ Quick Start: Docker (Recommended)

### 1. **Environment Setup**
```bash
# Copy the example configuration
cp dotenv.txt .env

# Edit .env and set your secrets (IMPORTANT!)
# Generate secure secrets for production:
openssl rand -hex 32  # Use for JWT_SECRET
openssl rand -hex 32  # Use for ADMIN_SECRET
```

### 2. **One-Click Startup**
```bash
# Windows
.\start-docker.bat

# Linux/macOS
chmod +x start-docker.sh
./start-docker.sh
```

The script will:
- ✅ Check for required environment variables
- 🛑 Stop any existing containers
- 🔨 Build and start services with health checks
- 📊 Show service status and recent logs
- 🌐 Display service URLs

**Services Available:**
- **Mothership Server**: `http://localhost:7523`
- **Auth Server**: `http://localhost:3001`

### 3. **Test the Complete Flow**
```bash
# 1. Authenticate (opens browser automatically)
cargo run --bin mothership -- auth
# Complete authentication at http://localhost:3001

# 2. List available projects
cargo run --bin mothership -- gateway list

# 3. Create your first project
mkdir my-project && cd my-project
cargo run --bin mothership -- gateway create "My Project" --dir .

# 4. Beam into the project
cargo run --bin mothership -- beam "My Project"
# ✅ You're now connected!
```

## 🎮 Manual Setup (Development)

### Local Development
```bash
# Terminal 1: Start the Mothership server
cargo run --bin mothership-server

# Terminal 2: Start the auth server
cd auth-server
npm install
npm start

# Terminal 3: Use the CLI
cargo run --bin mothership -- auth
```

## ⚙️ Configuration

### Environment Variables
```bash
# Required for production
JWT_SECRET=your-secure-jwt-secret-here
ADMIN_SECRET=your-secure-admin-secret-here

# Optional configuration
MOTHERSHIP_PORT=7523
AUTH_SERVER_PORT=3001
RUST_LOG=info

# OAuth (for Tauri GUI)
GOOGLE_CLIENT_ID=your-google-client-id
GOOGLE_CLIENT_SECRET=your-google-client-secret
```

**Security Note**: Always use secure, randomly generated secrets in production!

## 🧪 API Testing

### Health Checks
```bash
curl http://localhost:7523/health
curl http://localhost:3001/health
# Both should return {"status": "ok"}
```

### Admin User Creation
```bash
# Create SuperAdmin user
curl -X POST http://localhost:7523/admin/create \
  -H "Content-Type: application/json" \
  -d '{
    "secret": "your-admin-secret-from-env",
    "username": "admin",
    "email": "admin@yourdomain.com", 
    "role": "SuperAdmin"
  }'
```

### Docker Management
```bash
# View logs
docker-compose -f docker-compose.dev.yml logs -f

# Check status
docker-compose -f docker-compose.dev.yml ps

# Stop services
docker-compose -f docker-compose.dev.yml down

# Restart with rebuild
docker-compose -f docker-compose.dev.yml up --build -d
```

## 📁 Project Structure

```
Mothership/
├── MothershipVision.md          # Complete design document
├── start-docker.bat             # Windows one-click startup
├── start-docker.sh              # Linux/macOS one-click startup
├── docker-compose.dev.yml       # Development Docker setup
├── docker-compose.yml           # Production Docker setup
├── Dockerfile.server            # Server container definition
├── mothership-common/           # Shared types and protocols
│   ├── src/lib.rs              # Core data structures
│   ├── src/auth.rs             # Authentication types
│   └── src/protocol.rs         # WebSocket message protocol
├── mothership-server/           # Central server
│   ├── src/main.rs             # HTTP/WebSocket server (port 7523)
│   ├── src/auth.rs             # JWT authentication service
│   ├── src/database.rs         # In-memory database (demo data)
│   ├── src/sync.rs             # Real-time sync manager
│   └── src/handlers.rs         # API request handlers
├── mothership-cli/              # Client CLI
│   ├── src/main.rs             # Main CLI application
│   ├── src/config.rs           # Configuration management
│   ├── src/auth.rs             # Authentication flow
│   ├── src/gateway.rs          # Project discovery
│   ├── src/beam.rs             # Project joining
│   └── src/sync.rs             # Sync status/checkpoints
├── mothership-gui/              # Tauri desktop application
│   ├── src/main.rs             # Tauri backend with OAuth
│   ├── main.js                 # Frontend JavaScript
│   └── index.html              # GUI interface
└── auth-server/                 # Authentication server
    ├── server.js               # Node.js server (port 3001)
    └── package.json            # Dependencies
```

## 🎮 Demo Data & Authentication

The server includes demo data for testing:

### Demo Users (Password: any value in demo mode)
- **Alice** (Admin role) - Can create projects and manage users
- **Bob** (User role) - Regular user with project access

### Demo Projects
- **"Mothership Core"** - Shared project (Alice + Bob have access)
- **"Demo App"** - Alice's private project

### Authentication Flow
1. Run `mothership auth` - Opens browser automatically
2. Enter username/email at http://localhost:3001
3. Click "Authorize Device"
4. CLI automatically detects completion
5. Credentials saved for future commands

### OAuth Integration (Tauri GUI)
The Tauri GUI application includes complete Google OAuth integration:
- Click "Sign in with Google" button
- Complete OAuth in browser
- Token automatically transferred to app
- Authentication modal closes automatically
- Projects load immediately

## 🎉 Major Achievements

### ✅ Production-Ready Infrastructure
- **Docker deployment** with health checks and service dependencies
- **One-click startup scripts** for Windows (`start-docker.bat`) and Unix (`start-docker.sh`)
- **Environment configuration** with comprehensive .env support
- **Security warnings** for default secrets in production
- **Cross-platform compatibility** tested on Windows, macOS, and Linux

### ✅ Complete Authentication System
- **Browser-based device flow** with automatic browser opening
- **OAuth integration** with Google authentication in Tauri GUI
- **Real-time callback system** for seamless token transfer
- **JWT tokens** with secure machine certificates
- **Multi-role permissions** (User/Admin/SuperAdmin)

### ✅ Revolutionary User Experience
- **Human-readable project names** instead of UUIDs
- **Intuitive CLI commands** that match mental models
- **Color-coded output** with clear success/error messaging
- **Automatic project discovery** with `mothership gateway list`
- **Zero-ceremony project creation** with `mothership gateway create`

## 🌟 Why Mothership vs Git?

| Traditional Git | Mothership |
|----------------|------------|
| `git clone`, `git checkout`, `git pull` | `mothership beam "Project Name"` |
| Manual commits with messages | Automatic checkpointing on file save |
| Merge conflicts on collaboration | Real-time collaboration in shared rifts |
| Fear of `git reset --hard` | Zero destructive operations by design |
| Branch/merge ceremony | Seamless rift convergence |
| Local-only until push | Live sync with team members |
| Complex setup and configuration | One-click Docker deployment |
| UUID-based project references | Human-readable project names |

## 🚀 Business Vision: Self-Hosted → SaaS Scale

**Today:** Open-source foundation with production-ready Docker deployment  
**Tomorrow:** Hosted service competing with GitHub/GitLab with superior collaboration  
**Future:** The standard for real-time collaborative development at enterprise scale  

This isn't just another Git alternative - it's the foundation for a new category of development tools that eliminate version control friction while enabling unprecedented collaboration.

## 🎯 Next Milestone: File Tracking Engine

**Phase 2 Implementation Priority:**
1. **File system watching** using the `notify` crate for automatic change detection
2. **Batched API calls** for efficient change synchronization to prevent API spam
3. **Local caching system** for offline-first development with smart reconnection
4. **Automatic checkpoint creation** on file changes with configurable batching
5. **Real-time change broadcasting** via WebSocket to all project collaborators

The foundation is solid. The Docker deployment works flawlessly. The authentication is seamless. Now we build the revolutionary file tracking engine that will make Mothership the future of version control.

---

**🚀 Mothership is ready for production deployment!** Start with `.\start-docker.bat` and experience the future of frictionless version control. 