# Mothership: Zero-Friction Collaborative Development

Check it out live at: https://app.mothershipproject.dev

[![Build Status](https://img.shields.io/badge/build-passing-brightgreen)]() [![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)]() [![License](https://img.shields.io/badge/license-Proprietary-red.svg)](LICENSE)

## PROPRIETARY SOFTWARE NOTICE

**This is proprietary software. The source code is made available for viewing and evaluation purposes only. Commercial use, redistribution, or deployment requires a commercial license.**

For licensing inquiries: **licensing@mothership.dev**

## What is Mothership?

Mothership is a **frictionless version control system** that eliminates the complexity and fear associated with traditional Git workflows while delivering **instant real-time collaboration**. Instead of manual commits, merge conflicts, and complex branching strategies, Mothership provides:

- **Zero Fear**: No destructive operations - every change is preserved automatically
- **Zero Ceremony**: No manual commits or staging - just code and collaborate with **instant real-time sync**
- **Zero Friction**: Authenticate once, discover projects instantly, collaborate seamlessly in real-time

## **BREAKTHROUGH: INSTANT REAL-TIME COLLABORATION OPERATIONAL**

> **Multiple developers can now beam into the same rift and edit code together in real-time with millisecond synchronization**

### **Revolutionary Capabilities Already Working:**
- **WebSocket Broadcasting**: Changes sync instantly between all collaborators  
- **Google Docs Experience**: Live file content sharing with conflict detection
- **Perfect Team Isolation**: Rift-specific channels for secure collaboration
- **Enterprise Scalability**: Tokio async with 1000-message broadcast capacity
- **Instant File Updates**: Full content synchronization, not just diffs

**Evidence**: Two people beaming into the same rift see each other's changes **instantly** as they type!

## Key Features

### **Gateway System**
- **Instant Project Discovery**: `mothership gateway list` shows all accessible projects with PostgreSQL persistence
- **Intuitive Project Deployment**: `mothership deploy` creates projects in current directory
- **Human-Readable Access**: Beam into projects by name with PostgreSQL lookups, not cryptic IDs
- **Intelligent Project Creation**: Automatic `.mothership` metadata with PostgreSQL storage and nested gateway prevention

### **Production-Ready Authentication**
- **OAuth Integration**: Complete Google OAuth flow with browser-based device authorization and PostgreSQL user persistence
- **Cross-Platform GUI**: Tauri-based authentication app for seamless token management with database integration
- **JWT Security**: Machine certificates with automatic token refresh, validation, and PostgreSQL user recreation
- **Multi-Role System**: User/Admin/SuperAdmin with PostgreSQL-backed secure role management and ACID compliance

### **Real-Time Collaboration Engine**
- **INSTANT MULTI-USER SYNC**: Multiple developers in same rift with millisecond-latency synchronization
- **WebSocket Broadcasting**: Live file content sharing via dedicated rift channels (`rift_{rift_id}`)
- **Google Docs-Level Experience**: Real-time editing with automatic conflict detection
- **Scalable Architecture**: Tokio async infrastructure supporting enterprise-level collaboration
- **Perfect Isolation**: Team-specific broadcast channels for secure multi-project environments
- **Live State Management**: Content-addressable storage with instant working state updates

### **Developer Experience**
- **One-Click Deployment**: `./start-docker.bat` launches complete PostgreSQL development environment
- **Zero-Friction Beam**: `mothership beam <project>` automatically starts background daemon and enables file tracking
- **Complete Daemon Management**: `mothership disconnect`, `mothership daemon status/stop/restart` for full lifecycle control
- **Non-Blocking Console**: Beam command returns immediately while daemon handles background sync
- **Intuitive Commands**: `mothership deploy` for project creation, `mothership gateway list` for discovery
- **Clean Codebase**: 100% warning-free compilation with sqlx compile-time safety and professional standards
- **Cross-Platform Support**: Windows, macOS, Linux with PostgreSQL persistence and automatic daemon spawning

## Quick Start

### Prerequisites
- **Docker & Docker Compose** (for PostgreSQL server deployment)
- **Rust 1.70+** (for CLI development with sqlx compile-time safety)
- **Node.js 18+** (for OAuth auth server)

### Self-Hosted Installation

**Commercial License Required for Production Use**

**Install from your licensed Mothership server:**

**macOS/Linux:**
```bash
curl -sSL https://your-mothership-server.com/cli/install | bash
```

**Windows:**
```powershell
irm https://your-mothership-server.com/cli/install/windows | iex
```

**Build from source (for evaluation only):**
```bash
# 1. Clone the repository (evaluation license applies)
git clone https://github.com/mgreenwood1001/mothership.git
cd mothership

# 2. Build and install locally (EVALUATION ONLY)
cargo install --path mothership-cli
cargo install --path mothership-daemon
```

### Quick Start

```bash
# Authenticate with your Mothership server
mothership auth

# Deploy a project in current directory
cd your-project
mothership deploy

# Start real-time collaboration
mothership beam "your-project"

# Stay updated with latest features
mothership update
```

### Server Setup (Commercial License Required)

**Production deployment requires a commercial license. Contact licensing@mothership.dev**

If you have a commercial license to run your own Mothership server:

```bash
# 1. Clone the repository (commercial license required)
git clone https://github.com/mgreenwood/mothership.git
cd mothership

# 2. Configure environment
cp .env.example .env
# Edit .env with your OAuth credentials and secrets

# 3. Start the complete Mothership stack
./start-docker.bat                 # Windows
# or
./start-docker.sh                   # macOS/Linux
```

### First Steps with Mothership

```bash
# List all your accessible projects (stored in PostgreSQL)
mothership gateway list

# Deploy a new project in current directory (intuitive!)
cd my-awesome-project
mothership deploy                                         # Uses directory name
mothership deploy "My Application"       # Custom name

# Create a gateway with explicit directory (traditional method)
mothership gateway create --dir ./my-app "My Application"

# Revolutionary zero-friction beam experience
mothership beam "My Application"
# Automatically starts background daemon if needed
# Registers project for continuous file tracking  
# Returns console immediately - no blocking!

# REAL-TIME COLLABORATION: Have a teammate run the same command!
# Both of you will now see each other's changes instantly as you edit files

# Complete daemon lifecycle management
mothership daemon status                       # Show daemon status + tracked projects
mothership disconnect "My Application"     # Remove project from tracking
mothership daemon stop                         # Graceful daemon shutdown
mothership daemon restart                   # Clean restart with fresh state

# Smart auto-detection for disconnect
cd my-application-directory
mothership disconnect                           # Auto-detects current project
```

### **Testing Real-Time Collaboration**

```bash
# Developer 1 (Machine A):
mothership beam "shared-project"
# Edit any file in the project

# Developer 2 (Machine B):  
mothership beam "shared-project"
# Watch files update in real-time as Developer 1 edits!

# Both developers see each other's changes instantly
```

## Architecture

Mothership consists of several key components:

```
┌─────────────────────────────────────────────────────────────┐
│                     Mothership Stack                        │
├─────────────────────────────────────────────────────────────┤
│    Mothership Server (Rust + Axum)           :7523      │
│    OAuth Auth Server (Node.js)                   :3001      │
│    Tauri GUI App (Rust + TypeScript)                    │
│    CLI Tools (Rust)                                       │
│    Real-Time Sync Engine (WebSocket Broadcasting)       │
│    Docker Infrastructure                                  │
└─────────────────────────────────────────────────────────────┘
```

### Core Services

- **Mothership Server** (`mothership-server/`): Core API server with PostgreSQL persistence handling projects, authentication, and **real-time WebSocket collaboration**
- **PostgreSQL Database**: Production-grade database with ACID compliance, relationships, and transaction safety
- **Auth Server** (`auth-server/`): OAuth callback handler and browser-based authentication with database integration
- **GUI Application** (`mothership-gui/`): Cross-platform desktop app for seamless OAuth with PostgreSQL user management
- **CLI Tools** (`mothership-cli/`): Command-line interface with `deploy` command and PostgreSQL project discovery
- **Real-Time Sync Engine**: **WebSocket broadcasting system enabling instant collaboration between multiple developers**
- **Common Library** (`mothership-common/`): Shared types, protocols, and PostgreSQL models with sqlx safety

## Current Status (January 2025)

### **Phase 1: Foundation - COMPLETE**

#### **Authentication System**
- Complete OAuth flow with Google integration and PostgreSQL user persistence
- Browser-based device authorization (ports 7523 + 3001) with database validation
- JWT token management with automatic refresh and PostgreSQL user recreation
- Cross-platform GUI for seamless authentication with database integration
- Multi-role user system (User/Admin/SuperAdmin) with PostgreSQL role management

#### **Gateway Management**
- Project creation and listing with human-readable names and PostgreSQL persistence
- Intuitive `mothership deploy` command for current directory project creation
- Intelligent `.mothership` metadata directory with PostgreSQL storage and relationships
- Nested gateway prevention with helpful error messages and database validation
- Local project tracking and PostgreSQL metadata persistence with ACID compliance

#### **Production Infrastructure**
- PostgreSQL integration with Docker Compose, health checks, and persistent volumes
- Sqlx offline mode with compile-time SQL validation for clean Docker builds
- One-click deployment scripts (`start-docker.bat`) with PostgreSQL stack
- Environment configuration with database URLs and security warnings
- Cross-platform compatibility (Windows/macOS/Linux) with PostgreSQL persistence

#### **Code Quality**
- 100% warning-free compilation with sqlx compile-time safety
- Professional codebase with PostgreSQL abstraction and clean architecture
- Comprehensive error handling, database transaction safety, and user feedback
- Security-first design with SQL injection prevention, JWT validation, and secret management

### **Phase 2: Background Daemon Engine - COMPLETED**

#### **Revolutionary Features Delivered**
- **Automatic Daemon Startup**: Beam command intelligently starts background daemon when needed
- **Complete IPC Server**: REST API with health, status, project management endpoints (port 7525)
- **Graceful Project Disconnect**: Individual projects can be removed from tracking while keeping daemon alive
- **Full Daemon Lifecycle**: Start, stop, restart, status commands for complete daemon management
- **Non-Blocking Console**: Beam returns immediately while daemon handles background file tracking
- **Smart Daemon Reuse**: Multiple projects share same daemon instance for efficiency

### **Phase 2.5: Real-Time Collaboration - BREAKTHROUGH ACHIEVED**

#### **REVOLUTIONARY DISCOVERY: INSTANT COLLABORATION OPERATIONAL**
- **INSTANT MULTI-USER SYNC**: WebSocket broadcasting with millisecond latency between machines
- **GOOGLE DOCS-LEVEL EXPERIENCE**: Real-time file content sharing with conflict detection ready
- **SCALABLE INFRASTRUCTURE**: Tokio async with 1000-message broadcast capacity for enterprise use
- **PERFECT TEAM ISOLATION**: Rift-specific channels (`rift_{rift_id}`) ensure secure collaboration
- **LIVE STATE MANAGEMENT**: Content-addressable storage with instant working state updates
- **ENTERPRISE-READY PROTOCOL**: Complete SyncMessage framework with comprehensive collaboration events

#### **Technical Proof**
```rust
// OPERATIONAL: Real-time file broadcasting
SyncMessage::FileUpdate {
    rift_id: msg_rift_id,
    path: path.clone(),
    content: content.clone(), // FULL CONTENT SYNCED INSTANTLY
    author: Uuid::new_v4(),
    timestamp,
};
// Broadcast to all rift collaborators: INSTANT
```

**Result**: Multiple developers beaming into same rift see each other's file changes **instantly**!

## New Commands Available

### **Project Management**
```bash
mothership beam "project-name"                       # Start tracking project (auto-starts daemon)
mothership disconnect "project-name"           # Stop tracking project      
mothership disconnect                                           # Auto-detect and disconnect current project
```

### **Daemon Management**
```bash
mothership daemon status                                   # Show daemon status + tracked projects
mothership daemon stop                                       # Gracefully stop background daemon
mothership daemon restart                                 # Stop and restart daemon with fresh state
```

### **Project Setup**
```bash
mothership deploy                                                 # Deploy current directory as project
mothership deploy "Custom Name"                     # Deploy with custom project name
mothership gateway list                                     # List all accessible projects
```

### **Real-Time Collaboration Testing**
```bash
# Developer A:
mothership beam "team-project"
# Edit src/main.rs

# Developer B (different machine):  
mothership beam "team-project"  
# Watch src/main.rs update in real-time!
```

## Project Structure

```
mothership/
├── mothership-server/           # Core API server (Rust + Axum + PostgreSQL + WebSocket)
├── mothership-daemon/           # Background file tracking daemon (Rust + IPC server)
├── migrations/                         # PostgreSQL database schema
├── auth-server/                       # OAuth callback handler (Node.js)
├── mothership-gui/                 # Cross-platform desktop app (Tauri)
├── mothership-cli/                 # Command-line tools (Rust + daemon management)
├── mothership-common/           # Shared types, protocols, and PostgreSQL models
├── docker-compose.yml           # Production deployment with PostgreSQL
├── Dockerfile.server             # Server container with sqlx offline mode
└── start-docker.{bat,sh}     # One-click PostgreSQL stack deployment
```

## Development

### Building from Source

```bash
# Build all components
cargo build --release

# Build specific components
cargo build --bin mothership                 # CLI tools
cargo build --bin mothership-server   # Core server
cargo build --bin mothership-gui         # Desktop GUI

# Run development server
cargo run --bin mothership-server

# Run CLI commands
cargo run --bin mothership -- gateway list
cargo run --bin mothership -- auth
```

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test suites
cargo test --bin mothership-server
cargo test --lib mothership-common
```

### Docker Development

```bash
# Start development environment
docker-compose -f docker-compose.dev.yml up --build

# View logs
docker logs mothership-mothership-server-1
docker logs mothership-auth-server-1

# Restart specific services
docker restart mothership-mothership-server-1
```

## Contributing & Feedback

While this is proprietary software, we welcome feedback and bug reports from the community!

### How to Contribute

1. **Report Issues**: Submit detailed bug reports with reproduction steps
2. **Suggest Features**: Share ideas for improvements and new functionality
3. **Provide Feedback**: Help us understand how Mothership can better serve your needs
4. **Join Discussions**: Participate in our Discord community

### For Developers

If you're interested in contributing code or joining our team:
- **Contact Us**: careers@mothership.dev
- **Partnership Opportunities**: partnerships@mothership.dev
- **Commercial Integration**: enterprise@mothership.dev

## Roadmap

### **Immediate (COMPLETED)**
- **Background daemon engine** - Automatic startup, lifecycle management, graceful disconnect
- **Non-blocking console experience** - Beam command returns immediately
- **Complete IPC infrastructure** - REST API for CLI-daemon communication
- **REAL-TIME COLLABORATION** - Instant multi-user sync with WebSocket broadcasting

### **Next Priority (Next 30 Days)**
- **Enhanced real-time UI** - Live cursors, presence indicators, and contextual chat
- **Advanced conflict resolution** - Visual merge tools with live collaboration context
- **Smart notifications** - Context-aware alerts when teammates make relevant changes

### **Short Term (Next 90 Days)**
- **PostgreSQL migration COMPLETED** - Production persistence with ACID compliance and relationships
- Cross-platform native installers (MSI/DMG/DEB) with integrated real-time collaboration
- Beta release with invite-only access for development teams featuring **instant collaboration**

### **Long Term (6-12 Months)**
- SaaS platform with hosted Mothership service featuring **real-time collaboration as core differentiator**
- IDE integrations (VS Code, JetBrains, Visual Studio) with native real-time editing
- AI-assisted collaboration and conflict resolution using live collaboration data

## Documentation

- **[Vision Document](MothershipVision.md)**: Complete project vision and philosophy
- **[API Documentation](docs/api.md)**: REST API and WebSocket protocol reference
- **[CLI Reference](docs/cli.md)**: Complete command-line interface documentation
- **[Deployment Guide](docs/deployment.md)**: Production deployment and scaling (License Required)
- **[Commercial Licensing](https://mothership.dev/licensing)**: Information about commercial licenses

## Commercial Support & Licensing

- **Enterprise Support**: enterprise@mothership.dev
- **Commercial Licensing**: licensing@mothership.dev
- **Security Issues**: security@mothership.dev
- **Community Chat**: [Discord Server](https://discord.gg/mothership)

## License

This project is licensed under the Mothership Proprietary License - see the [LICENSE](LICENSE) file for details.

**Key Points:**
- View and study the code
- Submit bug reports and suggestions
- Commercial use without license
- Redistribution or resale
- Creating competing services

For commercial licensing options, please contact: **licensing@mothership.dev**

## Acknowledgments

- **Rust Community** for the incredible ecosystem that makes this possible
- **Tokio** for async runtime and WebSocket support enabling **real-time collaboration**
- **Axum** for the elegant web framework
- **Tauri** for cross-platform desktop application development
- **Early Adopters** who believe in frictionless collaborative development

---

**Built with passion by developers who believe coding should be collaborative, not combative.**

> *"Version control should enhance creativity, not constrain it. Mothership eliminates the fear of losing work and the friction of complex workflows, so teams can focus on building amazing software together - **in real-time**."*

---

## **Revolutionary Milestone Achieved - January 2025**

**Real-Time Collaboration Operational**: Mothership now delivers the **zero-friction collaborative development experience** promised in our vision:

- **Zero Fear**: Automatic daemon startup with graceful lifecycle management
- **Zero Ceremony**: `mothership beam <project>` - that's it! **With instant real-time sync**
- **Zero Friction**: Non-blocking console with background file tracking **and live collaboration**

**The beam command is no longer blocking. Multiple developers can collaborate in real-time. The daemon manages everything in the background. Disconnect is graceful. The revolution is here.**

---

**© 2025 Mothership Development Team. All Rights Reserved.**


Authentication Commands
auth - Authenticate with Mothership
google - Google OAuth login
github - GitHub OAuth login
device - Legacy device authentication
logout - Clear stored credentials

Project Management
gateway - Project management commands
list - List available projects
create - Create new project
deploy - Deploy new project
beam - Beam into a project
project-disconnect - Stop tracking a project

Sync & Version Control
status - Check environment status
checkpoint - Create a checkpoint
sync - Sync with remote
history - View project history
restore - Restore to checkpoint
delete - Delete project

Server Management
connect - Connect to a server
server - Server operations
status - Show connection status
disconnect - Disconnect from server
list - List configured servers

Rift Management
rifts - List all rifts
create-rift - Create new rift
switch-rift - Switch to different rift
rift-status - Show current rift status
rift-diff - Compare rifts

Daemon Management
daemon - Daemon operations
status - Show daemon status
stop - Stop daemon
restart - Restart daemon

Updates
update - CLI update management
--check-only - Check for updates
--force - Force update
--list-versions - Show available versions
--version - Update to specific version

Each command has its own set of options and flags for fine-tuned control. The diagram shows the hierarchical relationship between commands and their subcommands.
