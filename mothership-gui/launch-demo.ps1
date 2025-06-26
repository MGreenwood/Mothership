# Mothership GUI Demo Launcher
Write-Host "🚀 Mothership GUI Demo Launcher" -ForegroundColor Cyan
Write-Host "=================================" -ForegroundColor Cyan

# Check if we're in the right directory
if (!(Test-Path "package.json")) {
    Write-Host "❌ Error: Please run this script from the mothership-gui directory" -ForegroundColor Red
    exit 1
}

# Check if Node.js is installed
try {
    $nodeVersion = node --version
    Write-Host "✅ Node.js version: $nodeVersion" -ForegroundColor Green
} catch {
    Write-Host "❌ Error: Node.js is not installed or not in PATH" -ForegroundColor Red
    Write-Host "Please install Node.js from https://nodejs.org/" -ForegroundColor Yellow
    exit 1
}

# Check if Rust is installed
try {
    $rustVersion = rustc --version
    Write-Host "✅ Rust version: $rustVersion" -ForegroundColor Green
} catch {
    Write-Host "❌ Error: Rust is not installed or not in PATH" -ForegroundColor Red
    Write-Host "Please install Rust from https://rustup.rs/" -ForegroundColor Yellow
    exit 1
}

# Check if dependencies are installed
if (!(Test-Path "node_modules")) {
    Write-Host "📦 Installing Node.js dependencies..." -ForegroundColor Yellow
    npm install
    if ($LASTEXITCODE -eq 0) {
        Write-Host "✅ Dependencies installed successfully" -ForegroundColor Green
    } else {
        Write-Host "❌ Failed to install dependencies" -ForegroundColor Red
        exit 1
    }
} else {
    Write-Host "✅ Dependencies already installed" -ForegroundColor Green
}

# Check if Mothership server is running
Write-Host "🔍 Checking if Mothership server is running..." -ForegroundColor Yellow
try {
    $response = Invoke-WebRequest -Uri "http://localhost:7523/health" -Method GET -TimeoutSec 5
    Write-Host "✅ Mothership server is running" -ForegroundColor Green
} catch {
    Write-Host "⚠️  Mothership server is not running" -ForegroundColor Yellow
    Write-Host "Please start the server first:" -ForegroundColor Yellow
    Write-Host "   cd .." -ForegroundColor Cyan
    Write-Host "   docker-compose up" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "Would you like to continue anyway? (y/n)" -ForegroundColor Yellow
    $continue = Read-Host
    if ($continue -ne "y" -and $continue -ne "Y") {
        Write-Host "Demo cancelled" -ForegroundColor Red
        exit 1
    }
}

Write-Host ""
Write-Host "🎯 Launching Mothership GUI..." -ForegroundColor Green
Write-Host "This will open the Tauri application with Monaco editor and vim integration" -ForegroundColor Cyan
Write-Host ""
Write-Host "Demo features to try:" -ForegroundColor Yellow
Write-Host "• Click 'Authenticate' to connect to Mothership server" -ForegroundColor White
Write-Host "• Browse and open files from the sidebar" -ForegroundColor White  
Write-Host "• Toggle vim mode with the 'Vim Mode' button" -ForegroundColor White
Write-Host "• Create checkpoints with the 'Checkpoint' button" -ForegroundColor White
Write-Host "• Try vim keybindings: hjkl, i/a/o, v, dd, yy, etc." -ForegroundColor White
Write-Host ""

# Launch the application
npm run dev 