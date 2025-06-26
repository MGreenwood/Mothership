# Mothership GUI

A proof-of-concept desktop application for Mothership version control system with integrated Monaco editor and vim mode support.

## Features

- **Monaco Editor Integration**: Full-featured code editor with syntax highlighting
- **Vim Mode Support**: Toggle between normal mode and vim mode
- **File Explorer**: Browse and open files from local projects
- **Project Management**: Authenticate and load projects from Mothership server
- **Auto-save**: Automatic file saving after changes
- **Modern UI**: Dark theme with VS Code-inspired interface
- **Mothership Integration**: Connect to the Mothership server for version control

## Prerequisites

- Node.js (v16 or higher)
- Rust (latest stable)
- Mothership server running on `localhost:7523`

## Setup

1. **Install dependencies:**
   ```bash
   cd mothership-gui
   npm install
   ```

2. **Install Rust dependencies:**
   ```bash
   cargo fetch
   ```

## Development

Run in development mode with hot reloading:

```bash
npm run dev
```

This will:
- Start the Vite development server on `http://localhost:1420`
- Launch the Tauri application
- Enable hot reloading for frontend changes

## Building

Build the application for production:

```bash
npm run build
```

The built application will be available in the `src-tauri/target/release/` directory.

## Usage

1. **Start the Mothership server** (in the parent directory):
   ```bash
   docker-compose up
   ```

2. **Launch the GUI application:**
   ```bash
   npm run dev
   ```

3. **Authenticate with Mothership:**
   - Click the "Authenticate" button
   - Complete the authentication flow in your browser
   - The application will load your projects

4. **Open files:**
   - Select a project from the left sidebar
   - Browse files and click to open them in the editor
   - Files will auto-save as you edit

5. **Vim Mode:**
   - Toggle vim mode on/off with the "Vim Mode" button
   - Vim mode indicator shows current mode (NORMAL, INSERT, VISUAL)
   - All standard vim keybindings are supported

6. **Create Checkpoints:**
   - Click "Checkpoint" to create a version snapshot
   - Add an optional message describing the changes

## Architecture

- **Backend (Rust)**: Tauri application with file system access and Mothership API integration
- **Frontend (JavaScript)**: Monaco editor with vim mode plugin and modern UI
- **Integration**: Seamless communication between frontend and backend via Tauri's invoke system

## Key Components

- **Monaco Editor**: Microsoft's VS Code editor component
- **monaco-vim**: Vim mode plugin for Monaco
- **Tauri**: Rust-based desktop application framework
- **Vite**: Fast frontend build tool and dev server

## File Structure

```
mothership-gui/
├── src/
│   └── main.rs           # Tauri backend (Rust)
├── index.html            # Main HTML template
├── main.js              # Frontend application (JavaScript)
├── package.json         # Node.js dependencies
├── Cargo.toml           # Rust dependencies
├── tauri.conf.json      # Tauri configuration
└── vite.config.js       # Vite configuration
```

## Future Enhancements

- Real-time collaboration features
- Integrated terminal
- Git-style diff viewing
- Plugin system for custom vim configurations
- Multi-tab file editing
- Project templates and scaffolding
- Advanced search and replace
- Integrated debugging support 