# Mothership

Mothership is a version control system built around live collaboration. It keeps a history of every change, lets several people edit the same code at the same time, and records what happened as it happens.

Mothership is in active development. Commands, file formats, and interfaces can change between versions.

## What it does

- **Projects**: a project lives on a Mothership server. `mothership init` creates one from the current folder and writes a `.mothershipignore`. `mothership beam` links a local folder to a project.
- **Live sync**: a background daemon watches the beamed folder and sends changes to the server. People in the same rift receive them over a WebSocket.
- **Rifts**: separate lines of work inside a project, similar to branches. Members of a rift see each other's changes and can be given access individually.
- **Checkpoints**: named snapshots of a whole rift, or of selected files and folders. You can list the history, restore a checkpoint in full or only some paths, and delete checkpoints.
- **Diffs**: compare a rift with another rift, or with a checkpoint. Results are a list of changed files with line counts, or full unified diffs.
- **Event log and timeline**: each rift records saves, checkpoints, restores, chat, joins and leaves, wormhole actions, and access changes. The GUI timeline shows these as they happen.
- **Chat**: each rift has a chat. Messages can refer to code as `path:line` or `path/`.
- **Wormholes**: a preview that merges two or more rifts before you converge them into a new rift. Conflicts are resolved inside the wormhole, and the source rifts are not changed.
- **Conflict safety**: an edit made against an outdated version is kept in a conflict rift. Local files that a sync or restore overwrites are backed up under `.mothership/local-backup`.
- **Secret scanning**: before a file is sent, the daemon checks it for common secret patterns: cloud and service tokens, private key headers, and secret values in assignments. A file with an unapproved secret stays on your machine until you allow it.
- **Sign-in**: Google, or a GhostID wallet, through the browser.
- **Desktop hub (GUI)**: a Tauri app for projects, rifts, checkpoints, diffs, wormholes, secrets, the timeline, and an embedded Neovim editor. It currently targets Windows.
- **Interactive shell**: run `mothership` with no arguments to type commands without the `mothership` prefix.

## Requirements

- Rust (stable toolchain), to build the CLI, daemon, and server
- Docker and Docker Compose, to run a server with PostgreSQL
- Node.js 16 or later, to build the GUI frontend
- Windows, for the GUI's embedded terminal (ConPTY)


## Installing

Build from source:

```bash
git clone https://github.com/MGreenwood/Mothership.git
cd Mothership
cargo install --path mothership-cli
cargo install --path mothership-daemon
```

## Quick start

```bash
# Connect to a Mothership server and authenticate
mothership connect https://your-mothership-server.com
mothership auth google


# Create a project from the current folder
cd my-project
mothership init "My Project"

# Join a project from another machine, which syncs its files to that folder
mothership beam "My Project" --local-dir ./my-project

# Check what is running
mothership status
mothership daemon status
```

## Running a server

The server runs in Docker with PostgreSQL:

```bash
cp server.config.example server.config   # then edit it; see SERVER_CONFIG.md
./start-docker.sh                        # or start-docker.bat on Windows
```

Set the secrets the server requires, such as `JWT_SECRET`, and the OAuth credentials for the sign-in methods you enable. [SERVER_CONFIG.md](SERVER_CONFIG.md) describes the options.
The full command list is in the [CLI reference](#cli-commands-reference).

## Architecture

- **Server** (`mothership-server/`): HTTP API and WebSocket endpoint for projects, rifts, checkpoints, chat, diffs, and wormholes. Handles sign-in. Listens on port 7523 by default.
- **Daemon** (`mothership-daemon/`): runs in the background, watches beamed folders, and syncs changes over WebSocket. Serves a local API on `127.0.0.1:7525`.
- **CLI** (`mothership-cli/`): the `mothership` command.
- **GUI** (`mothership-gui/`): a Tauri desktop app with a React frontend.
- **Common** (`mothership-common/`): shared types, protocol messages, diff and merge logic, ignore rules, and the API client used by the CLI and GUI.
- **PostgreSQL**: users, projects, rifts, chat, the event log, and wormholes. Schema changes are in `migrations/`.

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
mothership delete <project-name> [-f]                 # Delete a project from the server
```

### **Rift & Checkpoint Management**
```bash
mothership rift list [--detailed]                     # List rifts in the current project
mothership rift new <name> [-d <description>] [-p]    # Create a rift forked from the current rift; -p makes it private
mothership rift switch <name>                         # Switch the local folder and daemon to another rift
mothership rift status                                # Show the current rift
mothership rift users <username> -a|-r                # Owner: add (-a) or remove (-r) a rift member
mothership rift diff [from] [to] [--full] [--file <path>]  # Compare rifts or checkpoints; a side is a rift name or checkpoint:<id>
mothership rift purge                                 # List and delete .mothership/local-backup files
mothership rift chat send -r <rift> <message...>      # Send a chat message to a rift
mothership rift chat history -r <rift> [-l <limit>]   # Show recent chat messages
mothership rift chat enter [-r <rift>]                # Interactive chat prompt (Ctrl+Z then Enter to leave)
mothership checkpoint [-m <message>] [PATH...]        # Checkpoint the whole rift, or only the named files and folders (folders end in /)
mothership checkpoint delete <checkpoint-id> [-f]     # Delete a checkpoint
mothership history [-l <limit>]                       # Show checkpoint history
mothership restore <checkpoint-id> [PATH...] [-f]     # Restore the whole checkpoint, or only the named paths
mothership secrets                                    # Review files held back because they contain a secret, and allow or keep them
```

### **Wormholes (Merge Previews)**
```bash
mothership wormhole create <name> <rift1> <rift2>... [-p]  # Preview merging two or more rifts; -p makes it private
mothership wormhole list                              # List wormholes in the current project
mothership wormhole show <wormhole>                   # Show merged state, muted rifts, and conflicts
mothership wormhole toggle <wormhole> <rift> [--enable|--disable]  # Turn a rift's changes on or off in the preview
mothership wormhole resolve <wormhole> <path> <conflict> --side <rift> # Resolve a conflict by side, or with --text or --file
mothership wormhole beam <wormhole> [--dir <path>]    # Copy the merged files into a read-only folder
mothership wormhole watch <wormhole>                  # Report when any of its rifts change
mothership wormhole converge <wormhole> [--name <n>]  # Create a new rift from the merged result (blocked while conflicts remain)
mothership wormhole dissolve <wormhole> [-d]          # Archive the wormhole, or delete it with -d
```

### **Daemon, Server & Account Management**
```bash
mothership connect <server-url>                       # Connect the CLI to a Mothership server
mothership server status                              # Show the current server connection
mothership server list                                # List configured servers
mothership server disconnect                          # Switch to local-only mode
mothership auth [google|ghostid]                      # Sign in through the browser
mothership logout                                     # Clear stored credentials
mothership daemon status                              # Show daemon status and tracked projects
mothership daemon stop                                # Stop the background daemon
mothership daemon restart                             # Restart the background daemon
mothership status                                     # Show environment, server, and project status
mothership sync                                       # Sync the local folder with the server now
mothership update [--check-only|--force|--list-versions|--version <v>]  # Update the CLI
```

### **Ignore Rules & Local Discovery Notes**
- **`.mothershipignore`**: `mothership init` writes a default `.mothershipignore` (excluding `.git/`, `node_modules/`, `/target/`, `/dist/`, `/build/`, secrets, and binaries). Both the file watcher and daemon force-sync honor `.mothershipignore`, and `.mothership/` is always excluded.
- **Local Project Discovery**: When resolving local projects (`mothership gateway list`, `mothership beam`), the CLI walks search roots (`cwd`, `%USERPROFILE%\mothership`, `%USERPROFILE%`) up to depth 3 and prunes `node_modules`, `target`, `.git`, dot-prefixed directories other than `.mothership`, and `AppData` so lookups stay fast. Projects stored inside those pruned directories will not be auto-discovered.

## Project Structure

```
Mothership/
├── mothership-server/     # HTTP API, WebSocket endpoint, storage (Rust + Axum + PostgreSQL)
├── mothership-daemon/     # Background sync daemon and local IPC API (Rust)
├── mothership-cli/        # The mothership command (Rust)
├── mothership-common/     # Shared types, protocol, diffs, secrets, and API client
├── mothership-gui/        # Desktop app (Tauri + React)
├── migrations/            # PostgreSQL schema
├── docs/                  # Browsable documentation pages
├── scripts/               # Build and test helper scripts
├── docker-compose.yml     # Server and PostgreSQL stack
├── Dockerfile.server      # Server container
└── start-docker.{bat,sh}  # Start the stack
```

## Development

### Building from source

The server build uses sqlx offline mode, so set `SQLX_OFFLINE=true` (see [docs/SQLX_OFFLINE.md](docs/SQLX_OFFLINE.md)). Build the GUI frontend once before building the GUI:

```bash
export SQLX_OFFLINE=true                  # PowerShell: $env:SQLX_OFFLINE='true'
cargo build -p mothership-cli             # the mothership command
cargo build -p mothership-daemon
cargo build -p mothership-server
cd mothership-gui && npm install && npm run build && cd ..
cargo build -p mothership-gui
```

### Running tests

```bash
cargo test -p mothership-cli
cargo test -p mothership-daemon
cargo test -p mothership-common --lib
```

Server tests that talk to PostgreSQL are marked `#[ignore]`. Point `TEST_DATABASE_URL` at a database with the migrations applied, then run:

```bash
cargo test -p mothership-server -- --ignored --test-threads=1
```

### Docker

```bash
docker compose -f docker-compose.dev.yml up --build
docker compose logs -f
```

## Contributing

Bug reports and feature requests are welcome as issues on the [GitHub repository](https://github.com/MGreenwood/Mothership/issues).

## Documentation

- [MothershipVision.md](MothershipVision.md): goals and planned features
- [docs/index.html](docs/index.html): browsable documentation for the CLI, server, daemon, and test analysis
- [SERVER_CONFIG.md](SERVER_CONFIG.md): server configuration
