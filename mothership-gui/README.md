# Mothership GUI

A Tauri 2 desktop hub for Mothership. It runs the real Neovim binary in embedded terminals and provides buttons for the main CLI features.

## Features

- **Embedded Neovim**: Each explorer click or "New Neovim" opens its own `nvim` process in an xterm.js tab, backed by a ConPTY. The user's normal Neovim config loads, and nothing is passed like `-u NONE`.
- **Tab handling**: Opening a file that is already open switches to its tab. Closing a tab with unsaved buffers asks for confirmation. Closing quits that Neovim process.
- **Open guards**: Files over 500 KB, binary extensions, and files with NUL bytes in their first 8 KB are refused with a message.
- **Explorer**: A tree view of the chosen root. Folders come first, and `.git`, `node_modules`, and `target` are skipped.
- **Projects**: Create, init, delete, and beam. Each project shows whether it is local (green) or remote-only (red). Beam downloads the project and overwrites existing local files, matching the CLI.
- **Rifts**: List, create, and switch.
- **Checkpoints**: Create and list.
- **Wormholes**: Create (two or more rifts), list, show, resolve conflicts, converge, dissolve, beam to a folder, and watch (polls every 3 s).
- **Daemon panel**: Shows status, tracked projects, and server connection. Start, stop, and restart are available. The GUI starts the daemon on launch if it is not already running.
- **Server panel**: Shows the server URL, health, and counts of projects, rifts, and active wormholes.
- **Sign in**: Opens the browser for OAuth. Paste the code shown there into the GUI.

## Prerequisites

- Node.js (v16 or higher)
- Rust (latest stable)
- Neovim v0.11 or later at `C:\Program Files\Neovim\bin\nvim.exe`, or on `PATH`
- Mothership server running on `localhost:7523`
- Windows (the ConPTY terminal is Windows-specific)

## Development

Start the Vite dev server, which `devUrl` points to on port 1420:

```bash
cd mothership-gui
npm run dev
```

In a second shell, build and run the Tauri app from the repo root:

```powershell
$env:SQLX_OFFLINE='true'; cargo run -p mothership-gui
```

`cargo run` does not start Vite on its own. If the window shows "localhost refused to connect", the Vite server is not running.

## Building

```bash
npm run build                   # frontend only, output in dist/
cargo build -p mothership-gui   # binary in target\debug\mothership-gui.exe
```

## Usage

1. **Start the server** (in the parent directory): `docker-compose up`.
2. **Start the app**: `npm run dev` in one shell, then `cargo run -p mothership-gui` in another.
3. **Sign in**: click Sign in. The browser shows a code. Paste it into the GUI.
4. **Projects and rifts**: pick a project to see its rifts, checkpoints, and wormholes. Beam downloads a project into a local folder.
5. **Explorer and Neovim**: the explorer starts empty. Open a folder from the explorer to browse it, and the choice is remembered. Click a file to open it in Neovim. Click "New Neovim" to open an empty Neovim tab.
6. **Wormholes**: create one from two or more rifts. Resolve conflicts with the per-side Use buttons. Converge is disabled while conflicts remain, and the server returns 409 if you try anyway.
7. **Daemon**: the GUI starts the daemon if it is not running. Stop and restart use the daemon's `/shutdown` route and then start it again.

## Architecture

- **Backend (Rust, `src/`)**: Tauri commands. Each module handles one area.
- **Frontend (React + Vite, `ui/`)**: Components for the hub, panels, explorer, and terminal tabs.
- **Shared logic**: Server calls and beam, index, and daemon-control logic live in `mothership-common`. The CLI uses the same code.

## Key Components

Backend (`src/`):

- `main.rs`: Tauri setup, plugins, and the `generate_handler!` command list.
- `hub.rs`: Hub commands for auth, projects, rifts, checkpoints, server info, daemon control, wormholes, and beam.
- `nvim.rs`: Neovim sessions on ConPTY (`portable-pty`). Guards open. Close polls `try_wait`, because the ConPTY output pipe can stay open after nvim exits.
- `explorer.rs`: `explorer_list_dir`, which returns folders first and skips noise directories.

Frontend (`ui/`):

- `App.jsx`: Layout, tabs, and explorer root.
- `Hub.jsx`: Projects, rifts, checkpoints, server card, and sign in or sign out.
- `DaemonPanel.jsx`: Daemon status, tracked projects, and start, stop, restart.
- `WormholePanel.jsx`: Wormhole list, detail, conflicts, watch, converge, beam, and dissolve.
- `Explorer.jsx`: Tree view.
- `TerminalTab.jsx`: xterm.js terminal bound to a Neovim session.
- `styles.css`: Dark theme. Green is `.ok` and red is `.bad`.

## File Structure

```
mothership-gui/
├── src/                  # Rust backend: main.rs, hub.rs, nvim.rs, explorer.rs
├── ui/                   # React frontend: App, Hub, DaemonPanel, WormholePanel, Explorer, TerminalTab
├── icons/                # App icons
├── capabilities/main.json
├── tauri.conf.json       # Tauri config (root copy is the one devUrl uses)
├── vite.config.js        # Vite config
├── hub.html              # Vite entry page
├── package.json          # Node.js dependencies and scripts
└── Cargo.toml            # Rust dependencies
```

Legacy files still in the repo and not used by the app: `demo.md`, `launch-demo.ps1`, and the `src-tauri/` folder.

## Future Enhancements

- Checkpoint restore (the CLI has it; the GUI does not yet)
- Git-style diff viewing
- Multi-tab file editing
