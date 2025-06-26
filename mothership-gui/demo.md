# Mothership GUI Demo

This guide demonstrates the key features of the Mothership GUI application.

## Quick Demo Setup

### 1. Start the Mothership Server

In the main Mothership directory:
```bash
docker-compose up
```

### 2. Install GUI Dependencies

```bash
cd mothership-gui
npm install
```

### 3. Launch the GUI Application

```bash
npm run dev
```

## Demo Walkthrough

### 🔐 Authentication Flow
1. Click the **"Authenticate"** button in the toolbar
2. Your browser will open automatically to complete authentication
3. Once authenticated, the button will show "Authenticated ✓"
4. Projects will automatically load

### 📁 Project Management
- View all your Mothership projects in the left sidebar
- Click on a project to select it and load its files
- Local projects show file browsers, remote projects show metadata

### 📝 File Editing Experience

#### Opening Files
- Browse files in the file explorer (left sidebar)
- Click on any file to open it in the Monaco editor
- Syntax highlighting is automatically applied based on file extension

#### Vim Mode Integration
- **Toggle vim mode** with the "Vim Mode" button in the toolbar
- Vim mode indicator shows current state:
  - 🟢 **NORMAL** - Navigation and commands
  - 🔵 **INSERT** - Text insertion
  - 🟠 **VISUAL** - Text selection
- All standard vim keybindings work:
  - `h,j,k,l` for navigation
  - `i,a,o` to enter insert mode
  - `v` for visual mode
  - `:w` to save (or auto-save works)
  - `dd` to delete lines
  - `yy` to copy lines
  - And many more!

#### Auto-Save
- Files automatically save 1 second after you stop typing
- No need to manually save files
- Console logs show successful auto-saves

### 🎯 Checkpoints (Version Control)
1. Make changes to a file
2. Click the **"Checkpoint"** button
3. Add an optional message describing your changes
4. The checkpoint is created and synced with the Mothership server

### 🎨 Modern Code Editor Features

#### Syntax Highlighting
The editor supports syntax highlighting for:
- JavaScript/TypeScript
- Python
- Rust
- Go
- Java
- C/C++
- HTML/CSS
- JSON/YAML
- Markdown
- And many more languages

#### Editor Features
- **Minimap** for file overview
- **Line numbers** and current line highlighting
- **Code folding** for functions and blocks
- **Word wrapping** for long lines
- **Smooth scrolling** and cursor animations
- **Multiple cursors** (Ctrl+Click)
- **Find and replace** (Ctrl+F, Ctrl+H)

## Architecture Highlights

### 🦀 Rust Backend (Tauri)
- Fast, secure native application
- File system access and management
- HTTP client for Mothership API integration
- Cross-platform compatibility

### 🌐 Modern Frontend
- Monaco Editor (VS Code's editor component)
- Vim mode integration via monaco-vim
- Responsive, dark-themed UI
- Real-time communication with Rust backend

### 🔄 Seamless Integration
- Tauri invoke system for frontend-backend communication
- Authentication state management
- File watching and auto-sync capabilities
- Project and workspace management

## Keyboard Shortcuts

### General
- `Ctrl+S` - Manual save (auto-save also works)
- `Ctrl+F` - Find in file
- `Ctrl+H` - Find and replace
- `Ctrl+/` - Toggle line comment
- `F11` - Toggle fullscreen

### Vim Mode (when enabled)
All standard vim keybindings including:
- Navigation: `h,j,k,l`, `w,b,e`, `gg,G`, `0,$`
- Editing: `i,a,o,O`, `x,dd,yy`, `p,P`
- Visual: `v,V,Ctrl+v`
- Commands: `:w,:q,:wq` and more

## Next Steps

This proof-of-concept demonstrates the potential for a revolutionary version control experience. Future enhancements could include:

- **Real-time collaboration** with live cursors
- **Integrated terminal** for command execution  
- **Advanced diff viewing** for change visualization
- **Plugin system** for custom configurations
- **Multi-tab editing** for multiple files
- **Project templates** and scaffolding tools

The foundation is solid for building a complete development environment that could compete with traditional tools while providing a superior user experience. 