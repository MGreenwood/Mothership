# Mothership: A Frictionless Version Control System

## Vision Statement

Mothership is an experimental version control system that eliminates the complexity and fear associated with traditional version control while adding collaborative features that others never had. Developers should never lose work, never need to manually commit changes, and should be able to collaborate in real-time within isolated development spaces - with conversations and context preserved as part of the project's living history.

## Core Philosophy

### The Four Pillars

1. **Zero Fear**: No destructive operations. Every change is preserved automatically.
2. **Zero Ceremony**: No manual commits, staging, or complex workflows. Just code.
3. **Zero Friction**: Authenticate once, discover projects instantly, collaborate seamlessly.
4. **Zero Context Loss**: Conversations, decisions, and development context preserved forever as part of the project history.

### The Problems We Want to Solve

- **Git Complexity**: Developers avoid powerful features due to steep learning curves
- **Data Loss Fear**: Simple commands can destroy hours of work
- **Commit Overhead**: Manual commits interrupt flow and create meaningless history
- **Merge Uncertainty**: You don't know if your merge will work until after you've committed to it
- **Lost Context**: Development decisions and conversations disappear into Slack/Discord, disconnected from the code

## **Project Scope: Real-Time Collaborative Development**

> **Mothership: What if version control felt like Google Docs, but for code, with context and conversations preserved forever?**

### **Core Features We're Building**

#### **Real-Time Collaboration Within Rifts** *Working Now*
- **Live editing**: Multiple developers see each other's changes instantly
- **WebSocket sync**: Millisecond-latency file synchronization 
- **Conflict detection**: Built-in collision handling
- **Perfect isolation**: Each rift has its own collaboration space

#### **Contextual Chat Integration** *Partially Working*
- **Basic rift chat**: `mothership rift chat` sends, reads, and interactively enters chat for a rift
- **Chat lives in rifts**: Conversations tied to specific development contexts
- **Persistent history**: All discussions preserved as part of the project timeline
- **File/line discussions**: Chat about specific code with context preserved *(planned)*
- **Decision trails**: Why changes were made, captured forever *(planned)*
- **Smart notifications**: Know when teammates discuss your code *(planned)*

#### **Wormholes: Preview Merge System** *Working, Experimental*
- **Live merge preview**: Create a wormhole from two or more rifts and see the merged state and conflicts
- **Collaborative resolution**: Resolve conflicts per file and conflict, and share the result within the wormhole
- **Non-destructive testing**: Try integrations without affecting source rifts
- **Converge when clean**: Create a new rift from the merged result once no conflicts remain

#### **Background Intelligence** *Working Now*
- **Checkpoints**: `mothership checkpoint` snapshots your work, with a message
- **Smart daemon**: Background sync without console blocking
- **Zero-ceremony beaming**: Join projects instantly, leave gracefully
- **File watching**: Changes detected and synced automatically

### **The Vision: Development with Full Context**

```bash
# Traditional development loses context:
git log --oneline
# a1b2c3d Added auth
# d4e5f6g Fixed bug
# g7h8i9j Updated UI

# What was the bug? Why this auth approach? What discussions happened?
# Context is lost forever in chat apps.

# Mothership preserves everything:
mothership history
# 2025-01-28 15:30 - Alice: "Let's use OAuth2 for this"
# 2025-01-28 15:32 - Bob: "Good idea, but what about refresh tokens?"
# 2025-01-28 15:35 - Alice: [Checkpoint] Added OAuth2 implementation
# 2025-01-28 15:40 - Charlie: "This breaks mobile login"
# 2025-01-28 15:45 - [Wormhole created] alice/auth + bob/mobile
# 2025-01-28 16:00 - Team resolves conflict collaboratively
# 2025-01-28 16:15 - [Convergence] Perfect integration achieved

# Full context, preserved forever.
```

## Core Concepts & Terminology

### Mothership
The central server that orchestrates all development activity. It manages projects, user access, and coordinates real-time collaboration across all connected clients.

### Gateway
The entry point to your development universe. Running `mothership gateway list` reveals all projects you have access to, and `mothership status` shows the current environment.

### Beam
The action of connecting to and syncing with a project. "Beaming into" a project either syncs an existing local copy or creates a new one. Once beamed in, you're live-connected to the project's collaborative space.

### Rift
A parallel development dimension within a project. Like Git branches, but with real-time collaboration and integrated chat. Multiple developers can work within the same rift, seeing each other's changes and discussing them in context - all preserved as part of the development history.

### Checkpoint
Snapshots of your work, created with `mothership checkpoint -m "message"`. You can view them with `mothership history` and restore one with `mothership restore`. The goal is for every meaningful change to be captured without manual steps, with conversations linked to each checkpoint.

### Wormhole
A **live preview bridge** connecting two or more rifts, allowing developers to see exactly how their separate features will integrate **before** converging them permanently. Think of it as a "test merge" that you can establish and dissolve instantly without affecting the source rifts.

### Convergence
The process of merging rifts back together. Unlike traditional merging, this can be guaranteed to work perfectly when teams have already resolved all conflicts collaboratively within wormholes.

## User Workflows

### Initial Setup
```bash
# One-time machine setup: point at a server, then sign in
mothership connect https://your-mothership-server.com
mothership auth google    # or: mothership auth ghostid
# Opens the browser sign-in flow
# Stores machine certificate locally
```

### The Complete Development Experience

#### **Project Discovery & Setup**
```bash
# Discover projects
mothership gateway list

# Deploy new project  
cd my-awesome-app
mothership init "My Awesome App"

# Beam into existing project
mothership beam "My Awesome App"
# Automatically starts daemon, begins file watching
# Joins the project's collaborative space
```

#### **Real-Time Collaborative Development**
```bash
# Multiple developers beam into the same rift
mothership beam "My Awesome App"
mothership rift switch new-feature
# Rifts are created with: mothership rift new <name>

# Now you can:
# - See each other's changes in real-time
# - Chat about the code as you write it
# - Ask questions with immediate context
# - Preserve all decisions as part of project history
```

#### **Contextual Conversations** *The Innovation*
```bash
# Chat is integrated into the development flow
mothership rift chat enter
> alice: "Why are we using this approach for auth?"
> bob: "Let me show you" [shares code at line 47]
> alice: "Oh I see, but what about refresh tokens?"
> bob: [makes change to handle refresh tokens]
> alice: "Perfect! Let's checkpoint this"

# Chat history is read separately from checkpoint history
mothership rift chat history
mothership history
```

#### **Advanced Integration with Wormholes**
```bash
# Test how features will integrate before merging
mothership wormhole create auth-ui alice-auth bob-ui
# Both teams see combined result immediately
# Chat in wormhole about integration challenges (planned)
# Resolve conflicts collaboratively
mothership wormhole resolve auth-ui <path> <conflict-id> --side alice-auth

# Converge only when no conflicts remain
mothership wormhole converge auth-ui
```

#### **Time Navigation with Full Context**
```bash
# Review the checkpoint history
mothership history

mothership restore <checkpoint-id>

mothership rift diff checkpoint:abc123 checkpoint:def456
# See what changed between two checkpoints
```

## Technical Architecture

### High-Level System Design

```
┌─────────────────────────────────────────────────────────────┐
│                  Mothership Central                         │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │   Gateway   │  │  Rift Sync  │  │   Checkpoint        │  │
│  │   Service   │  │   Engine    │  │   Storage           │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │    Auth     │  │ Chat Engine │  │   Wormhole          │  │
│  │  Manager    │  │ & History   │  │   Manager           │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                              │
                    WebSocket/HTTP API
                              │
┌─────────────────────────────────────────────────────────────┐
│                   Mothership Client                         │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │    CLI      │  │    File     │  │    Chat             │  │
│  │  Interface  │  │   Watcher   │  │    Interface        │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │    Sync     │  │ Background  │  │    Local Cache      │  │
│  │   Engine    │  │   Daemon    │  │    & Metadata       │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### Core Data Models

> Design sketch. Field names and types may differ from the current implementation.

```rust
struct Project {
    id: ProjectId,
    name: String,
    description: String,
    members: Vec<UserId>,
    created_at: DateTime,
    settings: ProjectSettings,
}

struct Rift {
    id: RiftId,
    project_id: ProjectId,
    name: String,
    parent_rift: Option<RiftId>,
    collaborators: Vec<UserId>,
    created_at: DateTime,
    last_checkpoint: CheckpointId,
    chat_history: Vec<ChatMessage>,
}

struct Checkpoint {
    id: CheckpointId,
    rift_id: RiftId,
    author: UserId,
    timestamp: DateTime,
    changes: Vec<FileChange>,
    parent: Option<CheckpointId>,
    message: Option<String>,
    related_chat: Vec<ChatMessageId>, // Link to conversations about this change
}

struct ChatMessage {
    id: ChatMessageId,
    rift_id: RiftId,
    author: UserId,
    timestamp: DateTime,
    content: String,
    message_type: ChatMessageType, // Text, CodeShare, FileReference, etc.
    context: Option<CodeContext>, // File/line references
}

struct Wormhole {
    id: WormholeId,
    project_id: ProjectId,
    rifts: Vec<RiftId>, // Can connect multiple rifts
    created_by: UserId,
    created_at: DateTime,
    status: WormholeStatus,
    chat_channel: ChatChannelId, // Wormholes have their own chat space
}
```

### Real-Time Communication Protocol

> Design sketch. Message names may differ from the current implementation.

```rust
enum SyncMessage {
    // Rift collaboration
    JoinRift { rift_id: RiftId },
    LeaveRift { rift_id: RiftId },
    FileChanged { rift_id: RiftId, path: PathBuf, content: String, timestamp: DateTime },
    
    // Chat integration
    SendChatMessage { rift_id: RiftId, message: String, context: Option<CodeContext> },
    ChatMessageReceived { rift_id: RiftId, message: ChatMessage },
    
    // Wormhole operations
    CreateWormhole { rifts: Vec<RiftId>, mode: WormholeMode },
    JoinWormhole { wormhole_id: WormholeId },
    WormholeUpdate { wormhole_id: WormholeId, combined_state: ProjectState },
    DissolveWormhole { wormhole_id: WormholeId },
    
    // Checkpointing
    CreateCheckpoint { rift_id: RiftId, message: Option<String> },
    CheckpointCreated { checkpoint: Checkpoint },
    
    // Server responses
    RiftUpdate { rift_id: RiftId, changes: Vec<FileChange> },
    UserJoined { rift_id: RiftId, user: UserInfo },
    UserLeft { rift_id: RiftId, user_id: UserId },
    
    // System
    Heartbeat,
    Error { message: String },
}
```

## Implementation Phases

### Phase 1: Foundation **COMPLETED - January 2025**
- [x] **Server Architecture** - Rust + Tokio + Axum running on port 7523
- [x] **Authentication System** - OAuth + JWT with PostgreSQL persistence
- [x] **CLI Tools** - Complete command set with intuitive UX
- [x] **Project Management** - Gateway creation, `init`, beaming
- [x] **Database Foundation** - PostgreSQL with proper schemas and relationships
- [x] **Docker Deployment** - One-click development environment
- [x] **Cross-platform Support** - Windows, macOS, Linux compatibility

### Phase 2: Real-Time Collaboration **COMPLETED - January 2025**
- [x] **WebSocket Infrastructure** - Real-time sync engine
- [x] **Multi-user Editing** - Live collaboration within rifts
- [x] **Background Daemon** - Non-blocking file watching and sync
- [x] **Conflict Detection** - Built-in collision handling framework
- [x] **Perfect Isolation** - Rift-specific broadcast channels

### Phase 3: Chat Integration & Context **IN PROGRESS**
**Goal: Make conversations part of the development history**
- [x] **Chat Engine** - Messaging within rifts (`mothership rift chat`)
- [ ] **Context Preservation** - Link conversations to code, files, and changes
- [ ] **Persistent History** - All conversations stored with checkpoints
- [ ] **Smart Notifications** - Context-aware alerts about relevant discussions
- [ ] **Code References** - Share and discuss specific lines/files in chat
- [ ] **Decision Trails** - Capture why changes were made, not just what changed

### Phase 4: Wormhole System **IN PROGRESS**
**Goal: Preview merges with collaborative resolution**
- [x] **Wormhole Creation** - Connect two or more rifts for integration testing
- [x] **Live Merge Preview** - See the combined state and conflicts
- [x] **Collaborative Resolution** - Resolve conflicts per file and conflict
- [ ] **Wormhole Chat** - Dedicated discussion space for integration
- [x] **Zero-surprise Convergence** - Converge blocked while conflicts remain
- [x] **Multi-rift Wormholes** - Create with two or more rifts

### Phase 5: Advanced User Experience (Future)
**Goal: Professional development environment**
- [~] **Desktop GUI** - Tauri app in `mothership-gui/` for sign-in and project management
- [ ] **IDE Plugins** - VS Code, JetBrains integration
- [ ] **Mobile Companion** - Review and chat from phone/tablet
- [ ] **Timeline Visualization** - Interactive history with chat context
- [ ] **AI Assistant** - Smart suggestions based on chat context and code patterns
- [ ] **Video/Voice Calls** - Built-in communication for complex discussions

### Phase 6: Enterprise Features (Future)
**Goal: Team and organization scale**
- [ ] **Multi-tenant Architecture** - Organization isolation
- [ ] **SSO Integration** - Enterprise authentication
- [ ] **Analytics Dashboard** - Team productivity and collaboration insights
- [ ] **Compliance Features** - Audit logs, security controls
- [ ] **API Ecosystem** - Third-party integrations

## Current Status (November 2026)

### **What's Working Now**
- **Real-time collaboration**: Multiple developers can edit the same rift simultaneously
- **Background sync**: File changes detected and synced automatically
- **Zero-friction beaming**: Join projects instantly with automatic daemon management
- **Checkpoints and history**: Create, view, and restore checkpoints
- **Rift chat**: Send and read chat messages within a rift
- **Wormholes (experimental)**: Create, inspect, resolve, and converge merge previews
- **PostgreSQL persistence**: Projects, rifts, and checkpoints stored in PostgreSQL
- **Cross-platform CLI**: Works on Windows, macOS, Linux
- **Sign-in**: Google login or GhostID wallet login

### **What We're Building Next**
- **Context preservation**: Link conversations to specific code, files, and changes
- **Smart notifications**: Know when teammates discuss your code
- **Wormhole chat**: A dedicated discussion space for each wormhole
- **Desktop GUI**: Bring chat and file views into the Tauri app

### **The Big Dream**
A development environment where:
- Code changes happen in real-time with your team
- Conversations about the code are preserved forever as context
- You can preview how features will integrate before merging
- The entire development story - decisions, discussions, iterations - is captured
- New team members can understand not just what was built, but why and how

## Migration & Adoption

### From Git to Mothership
- **Start small**: Use for new experimental projects
- **Gradual adoption**: Import existing Git repositories
- **Parallel workflow**: Keep Git for releases, use Mothership for development
- **Team consensus**: Adopt when the collaborative features prove valuable

### Target Use Cases
- **Pair programming teams** - Real-time collaboration with context
- **Remote teams** - Async collaboration with preserved context
- **Learning environments** - Students can see how experienced developers think
- **Experimental projects** - Low-friction version control for rapid iteration
- **Documentation-heavy projects** - Conversations become living documentation

---

## The Vision

Mothership isn't trying to replace Git tomorrow. It's an experiment in what version control could be if we designed it today, with real-time collaboration, persistent context, and zero-friction workflows.

We're building a development environment where the conversations, decisions, and collaborative process are preserved as part of the project's living history. Where you can see not just what changed, but why it changed, who discussed it, and how the team arrived at the solution.

**This is a fun project exploring the future of collaborative development.** If it works well, maybe it becomes something bigger. If not, we'll have learned a lot and built some cool tech along the way.

*The revolution might be coming, but first we're just trying to make development more collaborative and less lonely.* 