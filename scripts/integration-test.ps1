# Mothership End-to-End Integration Test Suite (CLI + GUI Neovim + Daemon + Rifts + Wormholes)
# Reproduces and verifies real-disk Neovim saves, rift creation/switching, subdirectory sync, and wormhole convergence.

param(
    [string]$MothershipBin = "C:\Users\craft\Mothership\target\debug\mothership.exe",
    [string]$NvimBin = "C:\Program Files\Neovim\bin\nvim.exe"
)

$ErrorActionPreference = "Stop"

function Write-Step($msg) { Write-Host "`n==== $msg ====" -ForegroundColor Yellow }
function Write-Pass($msg) { Write-Host "[PASS] $msg" -ForegroundColor Green }
function Write-Fail($msg) { Write-Host "[FAIL] $msg" -ForegroundColor Red; throw $msg }

function Assert-Contains($text, $expected, $context) {
    if ($null -eq $text -or -not $text.Contains($expected)) {
        Write-Fail "$context - expected to contain '$expected', got:`n$text"
    }
    Write-Pass "$context contains '$expected'"
}

function Assert-NotContains($text, $unexpected, $context) {
    if ($null -ne $text -and $text.Contains($unexpected)) {
        Write-Fail "$context - expected NOT to contain '$unexpected', got:`n$text"
    }
    Write-Pass "$context does not contain '$unexpected'"
}

if (-not (Test-Path $MothershipBin)) {
    $MothershipBin = "mothership"
}
if (-not (Test-Path $NvimBin)) {
    $NvimBin = "nvim"
}

$Stamp = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
$TestProject = "e2e-sync-$Stamp"
$TestDir = Join-Path $env:TEMP $TestProject
$Pipe = "\\.\pipe\mothership-e2e-nvim-$PID-$Stamp"
$NvimProc = $null
$OrigLocation = Get-Location

try {
    Write-Step "1. Preparing test workspace at $TestDir"
    New-Item -ItemType Directory -Path $TestDir -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $TestDir "src") -Force | Out-Null
    Set-Content -Path (Join-Path $TestDir "stack.txt") -Value "walkdir`negui" -NoNewline
    Set-Content -Path (Join-Path $TestDir "src\main.rs") -Value "fn main() { println!(`"v1`"); }" -NoNewline
    Set-Location $TestDir

    Write-Step "2. Initializing project '$TestProject' and beaming in"
    $initOut = & $MothershipBin init $TestProject 2>&1 | Out-String
    Write-Host $initOut
    if ($LASTEXITCODE -ne 0) { Write-Fail "mothership init failed" }
    Assert-Contains (Get-Content ".mothership\project.json" -Raw) $TestProject "project.json"

    Write-Step "3. Creating 'feature' rift forked from 'main'"
    $riftNewOut = & $MothershipBin rift new feature -d "Feature rift for e2e test" 2>&1 | Out-String
    Write-Host $riftNewOut
    if ($LASTEXITCODE -ne 0) { Write-Fail "mothership rift new failed" }

    Write-Step "4. Launching real Neovim session (matching mothership-gui nvim_open) and saving new lines on 'main'"
    $StackPath = Join-Path $TestDir "stack.txt"
    $NvimProc = Start-Process -FilePath $NvimBin -ArgumentList @(
        "--headless",
        "--listen", $Pipe,
        "--cmd", '"set autoread backupcopy=yes noswapfile"',
        "`"$StackPath`""
    ) -PassThru -WindowStyle Hidden

    # Wait for Neovim RPC pipe
    Start-Sleep -Milliseconds 600
    if ($NvimProc.HasExited) { Write-Fail "Neovim exited unexpectedly" }
    Write-Pass "Neovim RPC session listening on $Pipe"

    # Append a line in Neovim and write to disk (:w)
    & $NvimBin --server $Pipe --remote-send "<C-\><C-N>Gomain-nvim-line-123<Esc>:w<CR>" | Out-Null
    Start-Sleep -Milliseconds 500
    $diskMain = Get-Content $StackPath -Raw
    Assert-Contains $diskMain "main-nvim-line-123" "Disk stack.txt after Neovim :w on main"

    # Also edit subdirectory file src\main.rs on main
    Set-Content -Path (Join-Path $TestDir "src\main.rs") -Value "fn main() { println!(`"main-v2`"); }" -NoNewline
    Start-Sleep -Milliseconds 500

    Write-Step "5. Switching to 'feature' rift and reloading Neovim (matching GUI bumpRift -> nvim_reload_files)"
    $switchFeatureOut = & $MothershipBin rift switch feature 2>&1 | Out-String
    Write-Host $switchFeatureOut
    if ($LASTEXITCODE -ne 0) { Write-Fail "mothership rift switch feature failed" }

    & $NvimBin --server $Pipe --remote-send "<C-\><C-N>:set autoread backupcopy=yes<CR>:silent! checktime<CR>" | Out-Null
    Start-Sleep -Milliseconds 300

    $diskFeatureInitial = Get-Content $StackPath -Raw
    Assert-NotContains $diskFeatureInitial "main-nvim-line-123" "Disk stack.txt after switching to feature"

    $subFeatureInitial = Get-Content (Join-Path $TestDir "src\main.rs") -Raw
    Assert-Contains $subFeatureInitial "v1" "Disk src/main.rs on feature rift"

    Write-Step "6. Adding lines in Neovim on 'feature' rift and saving (:w), then immediately switching back to 'main'"
    & $NvimBin --server $Pipe --remote-send "<C-\><C-N>Gofeature-nvim-line-456<Esc>:w<CR>" | Out-Null
    Start-Sleep -Milliseconds 300
    $diskFeatureSaved = Get-Content $StackPath -Raw
    Assert-Contains $diskFeatureSaved "feature-nvim-line-456" "Disk stack.txt after Neovim :w on feature"
    Assert-NotContains $diskFeatureSaved "main-nvim-line-123" "Disk stack.txt on feature (proving Neovim reloaded feature buffer before editing)"

    # Switch back to main right after saving in Neovim
    $switchMainOut = & $MothershipBin rift switch main 2>&1 | Out-String
    Write-Host $switchMainOut
    if ($LASTEXITCODE -ne 0) { Write-Fail "mothership rift switch main failed" }

    & $NvimBin --server $Pipe --remote-send "<C-\><C-N>:set autoread backupcopy=yes<CR>:silent! checktime<CR>" | Out-Null
    Start-Sleep -Milliseconds 300

    $diskMainReturned = Get-Content $StackPath -Raw
    Assert-Contains $diskMainReturned "main-nvim-line-123" "Disk stack.txt after switching back to main"
    Assert-NotContains $diskMainReturned "feature-nvim-line-456" "Disk stack.txt on main does not have feature line"

    # Prove Neovim's in-memory buffer on main also reloaded main-nvim-line-123 by writing (:w) from Neovim
    & $NvimBin --server $Pipe --remote-send "<C-\><C-N>:w<CR>" | Out-Null
    Start-Sleep -Milliseconds 300
    $diskMainAfterNvimWrite = Get-Content $StackPath -Raw
    Assert-Contains $diskMainAfterNvimWrite "main-nvim-line-123" "Neovim buffer on main after reload + :w"
    Assert-NotContains $diskMainAfterNvimWrite "feature-nvim-line-456" "Neovim buffer on main does not have feature line"

    $subMainReturned = Get-Content (Join-Path $TestDir "src\main.rs") -Raw
    Assert-Contains $subMainReturned "main-v2" "Disk src/main.rs persisted on main after round-trip"

    Write-Step "7. Switching to 'feature' again to verify feature's Neovim edit also persisted on the server"
    & $MothershipBin rift switch feature | Out-Null
    & $NvimBin --server $Pipe --remote-send "<C-\><C-N>:set autoread backupcopy=yes<CR>:silent! checktime<CR>" | Out-Null
    Start-Sleep -Milliseconds 300

    $diskFeatureReturned = Get-Content $StackPath -Raw
    Assert-Contains $diskFeatureReturned "feature-nvim-line-456" "Disk stack.txt after returning to feature"
    Assert-NotContains $diskFeatureReturned "main-nvim-line-123" "Disk stack.txt on feature does not have main line"

    & $NvimBin --server $Pipe --remote-send "<C-\><C-N>:w<CR>" | Out-Null
    Start-Sleep -Milliseconds 300
    $diskFeatureAfterNvimWrite = Get-Content $StackPath -Raw
    Assert-Contains $diskFeatureAfterNvimWrite "feature-nvim-line-456" "Neovim buffer after returning to feature + :w"

    Write-Step "8. Testing Wormhole creation, conflict resolution, convergence, and switching to converged rift"
    $whCreate = & $MothershipBin wormhole create wh-merge main feature 2>&1 | Out-String
    Write-Host $whCreate
    if ($LASTEXITCODE -ne 0) { Write-Fail "wormhole create failed" }

    $whShow = & $MothershipBin wormhole show wh-merge 2>&1 | Out-String
    Write-Host $whShow
    if ($whShow -match "src/main\.rs[\s\S]*?conflict\s+([0-9a-f]{8})") {
        $srcConflictId = $Matches[1]
        Write-Host "Resolving conflict $srcConflictId in src/main.rs with side main..."
        & $MothershipBin wormhole resolve wh-merge "src/main.rs" $srcConflictId --side main | Out-Null
        if ($LASTEXITCODE -ne 0) { Write-Fail "wormhole resolve src/main.rs failed" }
    }
    if ($whShow -match "stack\.txt[\s\S]*?conflict\s+([0-9a-f]{8})") {
        $stackConflictId = $Matches[1]
        Write-Host "Resolving conflict $stackConflictId in stack.txt with combined custom text..."
        & $MothershipBin wormhole resolve wh-merge stack.txt $stackConflictId --text "walkdir`negui`n`nmain-nvim-line-123`nfeature-nvim-line-456" | Out-Null
        if ($LASTEXITCODE -ne 0) { Write-Fail "wormhole resolve stack.txt failed" }
    }

    $whConverge = & $MothershipBin wormhole converge wh-merge --name merged-rift 2>&1 | Out-String
    Write-Host $whConverge
    if ($LASTEXITCODE -ne 0) { Write-Fail "wormhole converge failed" }

    & $MothershipBin rift switch merged-rift | Out-Null
    & $NvimBin --server $Pipe --remote-send "<C-\><C-N>:set autoread backupcopy=yes<CR>:silent! checktime<CR>" | Out-Null
    Start-Sleep -Milliseconds 200

    $diskMerged = Get-Content $StackPath -Raw
    Assert-Contains $diskMerged "main-nvim-line-123" "Converged rift stack.txt has main line"
    Assert-Contains $diskMerged "feature-nvim-line-456" "Converged rift stack.txt has feature line"

    Write-Step "9. Chatting in the converged rift: send via CLI and read history back"
    $chatText = "chat-e2e-$Stamp"
    $chatSend = & $MothershipBin rift chat send --rift merged-rift $chatText 2>&1 | Out-String
    Write-Host $chatSend
    if ($LASTEXITCODE -ne 0) { Write-Fail "rift chat send failed" }
    Assert-Contains $chatSend "Sent to rift merged-rift" "rift chat send output"

    $chatHistory = & $MothershipBin rift chat history --rift merged-rift --limit 10 2>&1 | Out-String
    Write-Host $chatHistory
    if ($LASTEXITCODE -ne 0) { Write-Fail "rift chat history failed" }
    Assert-Contains $chatHistory $chatText "rift chat history output"
    $subMerged = Get-Content (Join-Path $TestDir "src\main.rs") -Raw
    Assert-Contains $subMerged "main-v2" "Converged rift src/main.rs has main-v2"

    Write-Host "`n========================================================" -ForegroundColor Green
    Write-Host "ALL END-TO-END CLI + GUI NEOVIM + DAEMON + WORMHOLE TESTS PASSED!" -ForegroundColor Green
    Write-Host "========================================================`n" -ForegroundColor Green
}
finally {
    Write-Step "Cleaning up test resources"
    if ($null -ne $NvimProc -and -not $NvimProc.HasExited) {
        try { & $NvimBin --server $Pipe --remote-send "<C-\><C-N>:qa!<CR>" 2>$null | Out-Null } catch {}
        Start-Sleep -Milliseconds 200
        if (-not $NvimProc.HasExited) { Stop-Process -Id $NvimProc.Id -Force -ErrorAction SilentlyContinue }
    }
    $cleanupFailed = $false
    try {
        Set-Location $TestDir -ErrorAction SilentlyContinue
        & $MothershipBin gateway release $TestProject | Out-Null
        & $MothershipBin delete $TestProject --force | Out-Null
        if ($LASTEXITCODE -ne 0) { $cleanupFailed = $true }
    } catch {
        $cleanupFailed = $true
    }
    Set-Location $OrigLocation
    if (Test-Path $TestDir) {
        Remove-Item -Recurse -Force $TestDir -ErrorAction SilentlyContinue
        if (Test-Path $TestDir) { $cleanupFailed = $true }
    }
    if ($cleanupFailed) {
        Write-Warning "Cleanup incomplete: gateway project '$TestProject' or temp dir '$TestDir' may still exist. Remove them manually."
    }
}
Write-Host "[+] Cleanup operations" 