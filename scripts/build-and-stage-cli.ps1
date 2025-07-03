# Build and stage Mothership CLI and Daemon for Windows distribution

$ErrorActionPreference = "Stop"

Write-Host "Building Mothership CLI and Daemon (Windows x86_64)" -ForegroundColor Cyan

# Set paths
$ProjectRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$CliTarget = "$ProjectRoot\..\target\release\mothership.exe"
$DaemonTarget = "$ProjectRoot\..\target\release\mothership-daemon.exe"

# Auto-increment patch version in Cargo.toml
$CargoTomlPath = "$ProjectRoot\..\mothership-cli\Cargo.toml"
$CargoToml = Get-Content $CargoTomlPath
$VersionLineIndex = $CargoToml | Select-String '^version\s*=\s*".*"' | Select-Object -First 1
if ($VersionLineIndex) {
    $CurrentVersion = ($VersionLineIndex -replace '.*version\s*=\s*"(.*)".*', '$1')
    $Parts = $CurrentVersion -split '\.'
    if ($Parts.Length -eq 3) {
        $Major = [int]$Parts[0]
        $Minor = [int]$Parts[1]
        $Patch = [int]$Parts[2] + 1
        $NewVersion = "$Major.$Minor.$Patch"
        $CargoToml = $CargoToml -replace '(?<=^version\s*=\s*")([^"]+)(?=")', $NewVersion
        Set-Content -Path $CargoTomlPath -Value $CargoToml
        Write-Host "Auto-incremented version: $CurrentVersion -> $NewVersion" -ForegroundColor Yellow
        $DistDir = "$ProjectRoot\..\cli-binaries\$NewVersion\x86_64-pc-windows-msvc"
    } else {
        Write-Host "ERROR: Version format not recognized" -ForegroundColor Red
        exit 1
    }
} else {
    Write-Host "ERROR: Could not find version in Cargo.toml" -ForegroundColor Red
    exit 1
}

# Ensure output directory exists
if (!(Test-Path $DistDir)) {
    New-Item -ItemType Directory -Path $DistDir | Out-Null
}

# Build CLI and Daemon
cd "$ProjectRoot\.."
cargo build --release -p mothership-cli -p mothership-daemon

# Copy binaries
Copy-Item $CliTarget "$DistDir\mothership.exe" -Force
Copy-Item $DaemonTarget "$DistDir\mothership-daemon.exe" -Force

Write-Host "SUCCESS: Binaries built and staged to $DistDir" -ForegroundColor Green 