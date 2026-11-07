# Mothership: Zero-Friction Collaborative Development

Check it out live at: https://app.mothership.rest

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
- **Instant Project Discovery**: `mothership gateway list` shows all accessible projects with PostgreSQL persistence and single-pass pruned local project indexing
- **Intuitive Project Initialization**: `mothership init` creates projects in the current directory and generates a default `.mothershipignore`
- **Human-Readable Access**: Beam into projects by name or ID with PostgreSQL lookups
- **Intelligent Project Creation**: Automatic `.mothership` metadata with PostgreSQL storage and nested gateway prevention

### **Production-Ready Authentication**
- **OAuth Integration**: Google sign-in and GhostID wallet sign-in (`mothership auth google`, `mothership auth ghostid`) with browser-based authorization and PostgreSQL user persistence. The CLI and GUI receive the token on a local callback, and the browser then lands on the success page.
- **Cross-Platform GUI**: Tauri-based desktop hub (`mothership-gui`) for managing projects, rifts, checkpoints, and wormholes
- **JWT Security**: Machine certificates with automatic token refresh and validation. Tokens for users missing from the database are rejected (401) and never recreate the user; re-login issues a fresh server-generated ID
- **Multi-Role System**: User/Admin/SuperAdmin with PostgreSQL-backed secure role management and ACID compliance

### **Real-Time Collaboration Engine**
- **INSTANT MULTI-USER SYNC**: Multiple developers in the same rift with millisecond-latency synchronization
- **Line-Diff & Full-Content Sync**: Incremental line diffs (`FileDiffChanged`) with base-hash verification, automatic fallback to full content, and chunked streaming for large files (>1 MB)
- **Non-Destructive Conflict Handling**: Edits against a stale base are automatically preserved in a conflict rift and local overwrites are backed up under `.mothership/local-backup`
- **Wormholes (Live Merge Previews)**: `mothership wormhole` lets teams preview, resolve, test (`wormhole beam`), watch, and converge merges across multiple rifts without mutating the source rifts
- **Built-In Rift Chat**: `mothership rift chat` supports sending messages, viewing history, and interactive terminal chat within a rift

### **Developer Experience**
- **One-Click Deployment**: `./start-docker.bat` launches complete PostgreSQL development environment
- **Zero-Friction Beam**: `mothership beam <project>` automatically starts the background daemon and enables file tracking
- **Complete Daemon Management**: `mothership gateway release`, `mothership daemon status/stop/restart` for full lifecycle control
- **Ignore-Aware Sync & Fast Discovery**: `.mothershipignore` rules prune `node_modules`, `target`, `.git`, and build artifacts across `init`, file watching, and daemon force-sync; local project discovery prunes ignored trees in a single walk
- **Cross-Platform Support**: Windows, macOS, Linux with PostgreSQL persistence, shell completions, and automatic daemon spawning

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
# Connect to a Mothership server and authenticate
mothership connect https://your-mothership-server.com
mothership auth

# Initialize a project in the current directory
cd your-project
mothership init

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

# Initialize a new project in the current directory
cd my-awesome-project
mothership init                          # Uses directory name and creates .mothershipignore
mothership init "My Application"         # Custom project name

# Revolutionary zero-friction beam experience
mothership beam "My Application"
# Automatically starts background daemon if needed
# Registers project for continuous file tracking
# Returns console immediately - no blocking!

# REAL-TIME COLLABORATION: Have a teammate run the same command!
# Both of you will now see each other's changes instantly as you edit files

# Complete daemon & gateway lifecycle management
mothership daemon status                 # Show daemon status + tracked projects
mothership gateway release "My Application" # Stop tracking project in daemon
mothership gateway release               # Auto-detects current project directory
mothership daemon stop                   # Graceful daemon shutdown
mothership daemon restart                # Clean restart with fresh state
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
- **CLI Tools** (`mothership-cli/`): Command-line interface with `init`, `gateway`, `beam`, `rift`, and `wormhole` commands
- **Real-Time Sync Engine**: **WebSocket broadcasting system enabling instant collaboration between multiple developers**
- **Common Library** (`mothership-common/`): Shared types, protocols, diff/merge engines, `.mothershipignore` rules, and PostgreSQL models with sqlx safety

## Current Status

### **Phase 1: Foundation - COMPLETE**

#### **Authentication System**
- Complete OAuth flow with Google and GitHub integration and PostgreSQL user persistence
- Browser-based device authorization (ports 7523 + 3001) with database validation
- JWT token management with automatic refresh; tokens for users missing from the database are rejected and require re-login
- Cross-platform GUI for seamless authentication with database integration
- Multi-role user system (User/Admin/SuperAdmin) with PostgreSQL role management

#### **Gateway Management**
- Project creation and listing with human-readable names and PostgreSQL persistence
- Intuitive `mothership init` command for current directory project creation and `.mothershipignore` generation
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

## CLI Commands Reference

### **Interactive shell**
```bash
mothership                                            # Open the shell. Type commands without the "mothership" prefix, e.g. "gateway list"
# Inside the shell: "help" lists commands, "exit" leaves. Ctrl+Z then Enter (Ctrl+D on macOS/Linux) also leaves.
```

### **Project & Gateway Management**
```bash
mothership init [name] [-p|--private]                 # Initialize current directory as a project (writes .mothershipignore); -p makes it private
mothership gateway list [--include-inactive]          # List all accessible projects and local status
mothership beam [project] [--local-dir <path>]        # Beam into a project and start background sync
mothership gateway release [project]                  # Stop background tracking for a project (defaults to current dir)
mothership delete <project-name> [--force]            # Delete a project from the server
```

### **Rift & Checkpoint Management**
```bash
mothership rift list [--detailed]                     # List rifts in the current project
mothership rift new <name> [-d <description>] [-p]    # Create a new rift forked from the current rift; -p makes it private
mothership rift switch <name>                         # Switch local workspace and daemon to another rift (non-members are offered an access request)
mothership rift status                                # Show current rift status; owners also see pending access requests
mothership rift users <username> -a|--add             # Owner: grant a user access to the rift (also adds them to the gateway)
mothership rift users <username> -r|--remove          # Owner: remove a user's access to the rift
mothership rift diff [from] [to]                      # Compare changes between rifts
mothership rift purge                                 # List and clean up .mothership/local-backup files
mothership rift chat send -r <rift> <message...>      # Send a chat message to a rift
mothership rift chat history -r <rift> [-l <limit>]   # View recent chat messages in a rift
mothership rift chat enter [-r <rift>]                # Interactive live chat prompt
mothership checkpoint [-m <message>]                  # Create a checkpoint on the current rift
mothership checkpoint delete <checkpoint-id> [--force]# Delete a checkpoint
mothership history [-l <limit>]                       # View checkpoint history
mothership restore <checkpoint-id> [--force]          # Restore workspace to a checkpoint
```

### **Wormholes (Live Multi-Rift Merge Previews)**
```bash
mothership wormhole create <name> <rift1> <rift2>... [-p|--private]  # Create a live merge preview across two or more rifts; -p makes it private
mothership wormhole list                              # List wormholes in the current project
mothership wormhole show <wormhole>                   # Inspect merged state, active/muted rifts, and conflicts
mothership wormhole toggle <wormhole> <rift> [--enable|--disable] # Toggle a developer's rift on or off in the wormhole
mothership wormhole resolve <wormhole> <path> <id> --side <rift>  # Resolve a conflict by side, --text, or --file
mothership wormhole beam <wormhole> [--dir <path>]    # Materialize merged files locally to build/test
mothership wormhole watch <wormhole>                  # Stream live updates (auto-refreshes beamed folder when clean)
mothership wormhole converge <wormhole> [--name <n>]  # Create a new rift from the resolved wormhole
mothership wormhole dissolve <wormhole> [-d|--delete] # Archive a wormhole, or permanently delete on server (y/n prompt)
```

### **Daemon, Server & Account Management**
```bash
mothership connect <server-url>                       # Connect to a Mothership server
mothership server status                              # Show current server connection status
mothership server list                                # List configured servers
mothership server disconnect                          # Switch to local-only mode
mothership auth [google|github]                       # Authenticate via OAuth
mothership logout                                     # Clear stored credentials
mothership daemon status                              # Show daemon status and tracked projects
mothership daemon stop                                # Gracefully stop background daemon
mothership daemon restart                             # Stop and restart daemon
mothership status                                     # Show environment, server, and project status
mothership sync                                       # Trigger manual synchronization
mothership update [--check-only|--force|--list-versions|--version <v>] # Manage CLI updates
```

### **Ignore Rules & Local Discovery Notes**
- **`.mothershipignore`**: `mothership init` writes a default `.mothershipignore` (excluding `.git/`, `node_modules/`, `/target/`, `/dist/`, `/build/`, secrets, and binaries). Both the file watcher and daemon force-sync honor `.mothershipignore`, and `.mothership/` is always excluded.
- **Local Project Discovery**: When resolving local projects (`mothership gateway list`, `mothership beam`), the CLI walks search roots (`cwd`, `%USERPROFILE%\mothership`, `%USERPROFILE%`) up to depth 3 and prunes `node_modules`, `target`, `.git`, dot-prefixed directories other than `.mothership`, and `AppData` so lookups stay fast. Projects stored inside those pruned directories will not be auto-discovered.

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

# Run specific crate test suites
cargo test -p mothership-common
cargo test -p mothership-cli
cargo test -p mothership-daemon
cargo test -p mothership-server

# Run release-mode performance/profiling benchmarks (ignored during normal test runs)
cargo test -p mothership-cli --release profile_ -- --ignored --nocapture --test-threads=1
cargo test -p mothership-daemon --release profile_ -- --ignored --nocapture --test-threads=1
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
- **[Documentation Hub](docs/index.html)**: Browsable docs for Mothership: CLI, server, daemon, and test analysis- **[Commercial Licensing](https://mothership.dev/licensing)**: Information about commercial licenses

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

**© 2025–2026 Mothership Development Team. All Rights Reserved.**
