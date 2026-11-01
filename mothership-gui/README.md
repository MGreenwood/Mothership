# Mothership GUI

A Tauri 2 desktop hub for Mothership. It runs the real Neovim binary in embedded terminals and provides buttons for the main CLI features.

## Features

- **Embedded Neovim**: Each explorer click or "New Neovim" opens its own `nvim` process in an xterm.js tab, backed by a ConPTY. The user's normal Neovim config loads, and nothing is passed like `-u NONE`.
- **Tab handling**: Opening a file that is already open switches to its tab. Closing a tab with unsaved buffers asks for confirmation. Closing quits that Neovim process.
- **Open guards**: Files over 500 KB, binary extensions, and files with NUL bytes in their first 8 KB are refused with a message.
- **Explorer**: A tree view of the chosen root. Folders come first, and `.git`, `node_modules`, and `target` are skipped.
- **Top bar**: Shows the selected gateway and the rift its local folder is beamed into, or "Not local" or "Not beamed into a rift". It stays visible in every view.
- **Gateways**: Create, init, delete, and beam. Each gateway shows whether it is local (green) or remote-only (red). Beam downloads the gateway and overwrites existing local files, matching the CLI.
- **Rifts**: List, create, and switch.
- **Checkpoints**: Create and list.
- **Wormholes**: Create (two or more rifts), list, show, resolve conflicts, converge, dissolve, and watch (polls every 3 s). An active wormhole can be beamed to a snapshot folder it picks. The daemon removes that snapshot once the wormhole is dissolved or converted, provided the folder's marker still matches. A converged wormhole shows "Beam converted rift", which beams the rift it created through the same flow as the gateway Beam button.
- **Daemon panel**: Shows status, tracked gateways, and server connection. Start, stop, and restart are available. The GUI starts the daemon on launch if it is not already running. The daemon also cleans up registered snapshots every 60 s.
- **Server panel**: Shows the server URL, health, and counts of gateways, rifts, and active wormholes.
- **Sign in**: Opens the browser for OAuth. Paste the code shown there into the GUI.

## Prerequisites

- Node.js (v16 or higher)
- Rust (latest stable)
- Neovim v0.11 or later at `C:\Program Files\Neovim\bin\nvim.exe`, or on `PATH`
- Mothership server running on `localhost:7523`
- Windows (the ConPTY terminal is Windows-specific)

## Development

The `custom-protocol` feature is on by default, so the app loads the built frontend from `dist/`. No Vite dev server is needed to run it. Build the frontend first, then build or run the app from the repo root:

```bash
cd mothership-gui
npm run build
```

```powershell
$env:SQLX_OFFLINE='true'; cargo run -p mothership-gui
```

To work on the frontend with hot reload, run `npm run dev` in `mothership-gui` and build with `--no-default-features` so the window loads `devUrl` (port 1420) instead.

## Building

```bash
npm run build                   # frontend, output in mothership-gui/dist/
cargo build -p mothership-gui   # binary in target\debug\mothership-gui.exe
```

`cargo build` runs `npm run build` automatically when the frontend sources (`ui/`, `hub.html`, `vite.config.js`, `package*.json`) change. Dev builds with `--no-default-features` skip it.

## Usage

1. **Start the server** (in the parent directory): `docker-compose up`.
2. **Start the app**: run `npm run build` in `mothership-gui`, then `cargo run -p mothership-gui`. No Vite server is needed.
3. **Sign in**: click Sign in. The browser shows a code. Paste it into the GUI.
4. **Gateways and rifts**: pick a gateway to see its rifts, checkpoints, and wormholes. Beam downloads a gateway into a local folder. The top bar shows which rift that folder is beamed into.
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
- `hub.rs`: Hub commands for auth, gateways (projects in the API), rifts, checkpoints, server info, daemon control, wormholes, beam, and the active-rift lookup for the top bar.
- `nvim.rs`: Neovim sessions on ConPTY (`portable-pty`). Guards open. Close polls `try_wait`, because the ConPTY output pipe can stay open after nvim exits.
- `explorer.rs`: `explorer_list_dir`, which returns folders first and skips noise directories.

Frontend (`ui/`):

- `App.jsx`: Layout, top bar, tabs, and explorer root.
- `ActiveRiftBar.jsx`: Top bar showing the selected gateway's active rift.
- `Hub.jsx`: Gateways, rifts, checkpoints, server card, and sign in or sign out. Reports the selected gateway to the top bar.
- `DaemonPanel.jsx`: Daemon status, tracked gateways, and start, stop, restart.
- `WormholePanel.jsx`: Wormhole list, detail, conflicts, watch, converge, beam, and dissolve.
- `Explorer.jsx`: Tree view.
- `TerminalTab.jsx`: xterm.js terminal bound to a Neovim session.
- `styles.css`: Dark theme. Green is `.ok` and red is `.bad`.

## File Structure

```
mothership-gui/
├── src/                  # Rust backend: main.rs, hub.rs, nvim.rs, explorer.rs
├── ui/                   # React frontend: App, ActiveRiftBar, Hub, DaemonPanel, WormholePanel, Explorer, TerminalTab
├── icons/                # App icons
├── capabilities/main.json
├── tauri.conf.json       # Tauri config (root copy is the one devUrl uses)
├── vite.config.js        # Vite config
├── hub.html              # Vite entry page
├── package.json          # Node.js dependencies and scripts
└── Cargo.toml            # Rust dependencies
```

## Future Enhancements

- Optional file deletion during checkpoint restore (opt-in, with confirmation)
- Multi-tab file editing
