# Mothership Integration Test Script (PowerShell)
# This script tests most features of the Mothership CLI in sequence

# Colors for output
$Red = [System.ConsoleColor]::Red
$Green = [System.ConsoleColor]::Green
$Yellow = [System.ConsoleColor]::Yellow

# Test project settings
$TestProject = "test-project-$([DateTimeOffset]::Now.ToUnixTimeSeconds())"
$TestRift = "feature-test"
$ServerUrl = "https://api.mothershipproject.dev"  # Production API server

function Write-Step {
    param([string]$Message)
    Write-Host "`n==== $Message ====" -ForegroundColor $Yellow
}

function Write-Success {
    param([string]$Message)
    Write-Host "[+] $Message" -ForegroundColor $Green
}

function Write-Error {
    param([string]$Message)
    Write-Host "[-] $Message" -ForegroundColor $Red
    exit 1
}

function Test-Command {
    param([string]$Command)
    if (!(Get-Command $Command -ErrorAction SilentlyContinue)) {
        Write-Error "$Command could not be found. Please install it first."
    }
}

# Error handling
$ErrorActionPreference = "Stop"

# Check prerequisites
Write-Step "Checking prerequisites"
Test-Command "mothership"
Test-Command "curl"
Write-Success "All prerequisites found"

# Test server connection
Write-Step "Testing server connection"
try {
    Invoke-WebRequest -Uri "$ServerUrl/health" -Method GET -UseBasicParsing | Out-Null
    Write-Success "Server is running"
}
catch {
    Write-Error "Server is not running at $ServerUrl"
}

# Clean up any existing connection
Write-Step "Cleaning up existing connections"
mothership server disconnect 2>$null
mothership logout 2>$null
Write-Success "Environment cleaned"

# Connect to server
Write-Step "Connecting to server"
try {
    mothership connect $ServerUrl
    $status = mothership server status
    if (!($status -match "Connected")) {
        Write-Error "Server connection failed"
    }
    Write-Success "Connected to server"
}
catch {
    Write-Error "Failed to connect to server"
}

# Authenticate (using Google OAuth for automation)
Write-Step "Authenticating"
try {
    mothership auth google
    Write-Success "Authentication successful"
}
catch {
    Write-Error "Authentication failed"
}

# List existing projects
Write-Step "Listing projects (pre-creation)"
try {
    mothership gateway list
    Write-Success "Project listing successful"
}
catch {
    Write-Error "Failed to list projects"
}

# Create test directory
Write-Step "Creating test project directory"
$TestDir = Join-Path $env:TEMP $TestProject
if (Test-Path $TestDir) { Remove-Item -Recurse -Force $TestDir }
New-Item -ItemType Directory -Path $TestDir | Out-Null
Set-Location $TestDir

# Initialize test project
Write-Step "Initializing test project"
"# Test Project" | Out-File -FilePath "README.md"
"print('Hello Mothership')" | Out-File -FilePath "main.py"
git init
Write-Success "Test files created"

# Deploy project
Write-Step "Deploying project"
try {
    mothership deploy $TestProject
    Write-Success "Project deployed"
}
catch {
    Write-Error "Project deployment failed"
}

# Verify project exists
Write-Step "Verifying project creation"
$projects = mothership gateway list
if (!($projects -match $TestProject)) {
    Write-Error "Project not found in gateway"
}
Write-Success "Project verified in gateway"

# Test rift operations
Write-Step "Testing rift operations"

# Create new rift
try {
    mothership create-rift $TestRift --description "Test rift for integration testing"
    Write-Success "Created test rift"
}
catch {
    Write-Error "Failed to create rift"
}

# List rifts
try {
    mothership rifts --detailed
    Write-Success "Listed rifts"
}
catch {
    Write-Error "Failed to list rifts"
}

# Switch to new rift
try {
    mothership switch-rift $TestRift
    Write-Success "Switched to test rift"
}
catch {
    Write-Error "Failed to switch rift"
}

# Check rift status
try {
    mothership rift-status
    Write-Success "Rift status checked"
}
catch {
    Write-Error "Failed to get rift status"
}

# Make some changes
Write-Step "Making test changes"
"# Additional changes" | Out-File -FilePath "README.md" -Append
"print('More changes')" | Out-File -FilePath "main.py" -Append

# Create checkpoint
Write-Step "Creating checkpoint"
try {
    mothership checkpoint "Test checkpoint"
    Write-Success "Checkpoint created"
}
catch {
    Write-Error "Failed to create checkpoint"
}

# View history
Write-Step "Viewing history"
try {
    mothership history --limit 5
    Write-Success "History viewed"
}
catch {
    Write-Error "Failed to view history"
}

# Test sync
Write-Step "Testing sync"
try {
    mothership sync
    Write-Success "Sync successful"
}
catch {
    Write-Error "Sync failed"
}

# Compare rifts
Write-Step "Comparing rifts"
try {
    mothership rift-diff --to "main"
    Write-Success "Rift comparison successful"
}
catch {
    Write-Error "Failed to compare rifts"
}

# Test daemon operations
Write-Step "Testing daemon operations"
try {
    mothership daemon status
    mothership daemon restart
    Write-Success "Daemon operations successful"
}
catch {
    Write-Error "Failed to perform daemon operations"
}

# Check for updates
Write-Step "Checking for updates"
try {
    mothership update --check-only
    mothership update --list-versions
    Write-Success "Update check successful"
}
catch {
    Write-Error "Update check failed"
}

# Clean up
Write-Step "Cleaning up"

# Disconnect from project
try {
    mothership project-disconnect $TestProject
    Write-Success "Disconnected from project"
}
catch {
    Write-Error "Failed to disconnect from project"
}

# Delete test project
try {
    mothership delete $TestProject --force
    Write-Success "Deleted test project"
}
catch {
    Write-Error "Failed to delete project"
}

# Disconnect from server
try {
    mothership server disconnect
    Write-Success "Disconnected from server"
}
catch {
    Write-Error "Failed to disconnect from server"
}

# Logout
try {
    mothership logout
    Write-Success "Logged out"
}
catch {
    Write-Error "Logout failed"
}

# Clean up test directory
Set-Location ..
Remove-Item -Recurse -Force $TestDir
Write-Success "Test directory cleaned up"

# Final status
Write-Step "Test Summary"
Write-Host "All tests completed successfully!" -ForegroundColor $Green
Write-Host "Tested features:"
Write-Host "[+] Server connection"
Write-Host "[+] Authentication"
Write-Host "[+] Project creation and management"
Write-Host "[+] Rift operations"
Write-Host "[+] File synchronization"
Write-Host "[+] Checkpoint creation"
Write-Host "[+] History viewing"
Write-Host "[+] Daemon operations"
Write-Host "[+] Update checking"
Write-Host "[+] Cleanup operations" 