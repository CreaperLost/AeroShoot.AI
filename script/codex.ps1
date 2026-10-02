# ==============================================================================
# AeroShoot.AI - Windows developer and release commands (companion to codex.sh
# and build.sh). Compatible with Windows PowerShell 5.1 and PowerShell 7.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File script/codex.ps1 <mode>
# ==============================================================================
param(
    [Parameter(Position = 0)]
    [ValidateSet("setup", "start", "stop", "run", "build", "help")]
    [string]$Mode = "help"
)

$ErrorActionPreference = "Stop"

$RootDir = Split-Path -Parent $PSScriptRoot
$FrontendDir = Join-Path $RootDir "front-end"
$TauriDir = Join-Path $RootDir "src-tauri"
$Manifest = Join-Path $TauriDir "Cargo.toml"
$DevPort = if ($env:AEROSHOOT_DEV_PORT) { [int]$env:AEROSHOOT_DEV_PORT } else { 1420 }
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $TauriDir "target" }
# Processes this launcher started, as "<pid> <start time ticks>". The start
# time guards against acting on a reused PID.
$RunDir = Join-Path $RootDir ".codex\run"
$DevPidFile = Join-Path $RunDir "dev-server.pid"
$AppPidFile = Join-Path $RunDir "app.pid"

function Show-Usage {
    @"
usage: script/codex.ps1 <setup|start|stop|run|build>

Commands:
  setup   Install the frontend dependencies from package-lock.json
  start   Start the Vite development server in the foreground
  stop    Stop the dev server and desktop app started by start/run
  run     Start Vite if needed, then run the desktop app in debug mode
  build   Run frontend and Rust tests, then build the release app:
          target\release\bundle\windows\AeroShoot.exe and a zip, plus
          the setup.exe and MSI installers when the Tauri CLI is installed
          (cargo install tauri-cli --locked)
"@
}

function Invoke-Native {
    param([string]$Description, [scriptblock]$Command)
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Description failed with exit code $LASTEXITCODE."
    }
}

function Assert-Command {
    param([string]$Name, [string]$Hint)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "'$Name' could not be found in PATH. $Hint"
    }
}

function Assert-Tools {
    param([switch]$Rust)
    Assert-Command "node" "Install Node.js 22 or newer."
    Assert-Command "npm" "Install Node.js 22 or newer."
    if ($Rust) {
        Assert-Command "cargo" "Install Rust from https://rustup.rs (MSVC toolchain)."
        $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
        $hasMsvc = (Test-Path $vswhere) -and
            (& $vswhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
        if (-not $hasMsvc) {
            Write-Warning "Visual Studio C++ build tools were not found; the Rust link step may fail."
        }
    }
}

function Install-FrontendDependencies {
    Push-Location $FrontendDir
    try {
        if (Test-Path "package-lock.json") {
            Invoke-Native "npm ci" { npm ci }
        } else {
            Invoke-Native "npm install" { npm install }
        }
    } finally {
        Pop-Location
    }
}

function Assert-FrontendDependencies {
    if (-not (Test-Path (Join-Path $FrontendDir "node_modules\.bin\vite.cmd"))) {
        throw "Frontend dependencies are not installed. Run the Setup action first."
    }
}

function Test-DevServer {
    $null -ne (Get-NetTCPConnection -LocalPort $DevPort -State Listen -ErrorAction SilentlyContinue)
}

function Wait-DevServer {
    param([bool]$Listening, [int]$Seconds, $Process)
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Test-DevServer) -ne $Listening) {
        if ($Process -and $Process.HasExited) { return $false }
        if ((Get-Date) -gt $deadline) { return $false }
        Start-Sleep -Milliseconds 200
    }
    return $true
}

function Save-Tracked {
    param([string]$File, [System.Diagnostics.Process]$Process)
    New-Item -ItemType Directory -Force -Path $RunDir | Out-Null
    Set-Content -Path $File -Value "$($Process.Id) $($Process.StartTime.ToUniversalTime().Ticks)" -Encoding ASCII
}

# The tracked process if it is still the one this launcher started, else $null.
function Get-Tracked {
    param([string]$File)
    $content = Get-Content $File -Raw -ErrorAction SilentlyContinue
    if (-not $content) { return $null }
    $fields = $content.Trim() -split " "
    if ($fields.Count -ne 2) { return $null }
    $process = Get-Process -Id ([int]$fields[0]) -ErrorAction SilentlyContinue
    if ($process -and $process.StartTime.ToUniversalTime().Ticks -eq [long]$fields[1]) {
        return $process
    }
    return $null
}

# Remove the PID file only if it still names this process.
function Clear-Tracked {
    param([string]$File, [System.Diagnostics.Process]$Process)
    $content = Get-Content $File -Raw -ErrorAction SilentlyContinue
    if (-not $content) { return }
    $fields = $content.Trim() -split " "
    if ($fields[0] -eq "$($Process.Id)") {
        Remove-Item -Force -ErrorAction SilentlyContinue $File
    }
}

# Every descendant of a PID, deepest first.
function Get-Descendants {
    param([int]$ProcessId, $All)
    foreach ($child in @($All | Where-Object { $_.ParentProcessId -eq $ProcessId })) {
        Get-Descendants -ProcessId $child.ProcessId -All $All
        $child.ProcessId
    }
}

# End a process and everything it started (cmd -> npm -> node, or cargo ->
# aeroshoot). The tree is collected first: children outlive a killed parent.
function Stop-Tree {
    param([System.Diagnostics.Process]$Process)
    $all = @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId)
    $tree = @(Get-Descendants -ProcessId $Process.Id -All $all) + $Process.Id
    foreach ($id in $tree) {
        Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
    }
    foreach ($id in $tree) {
        $member = Get-Process -Id $id -ErrorAction SilentlyContinue
        if ($member) { $member.WaitForExit(10000) | Out-Null }
    }
}

function Start-DevServer {
    param([switch]$Hidden)
    $arguments = @{
        FilePath         = "cmd.exe"
        ArgumentList     = @("/c", "npm run dev")
        WorkingDirectory = $FrontendDir
        PassThru         = $true
    }
    if ($Hidden) { $arguments.WindowStyle = "Hidden" } else { $arguments.NoNewWindow = $true }
    $process = Start-Process @arguments
    # Touch the handle now so ExitCode stays readable after the process exits.
    $null = $process.Handle
    Save-Tracked $DevPidFile $process
    return $process
}

function Invoke-Setup {
    Assert-Tools
    Install-FrontendDependencies
}

function Invoke-Start {
    Assert-Tools
    Assert-FrontendDependencies
    $running = Get-Tracked $DevPidFile
    if ($running) {
        throw "The AeroShoot dev server is already running (pid $($running.Id)). Run Stop first."
    }
    if (Test-DevServer) {
        throw "Port $DevPort is in use by another process; Vite needs it (strictPort)."
    }
    $vite = Start-DevServer
    try {
        $vite.WaitForExit()
        return $vite.ExitCode
    } finally {
        Clear-Tracked $DevPidFile $vite
    }
}

function Invoke-Stop {
    $stopped = $false
    foreach ($entry in @(@{ File = $AppPidFile; Name = "desktop app" }, @{ File = $DevPidFile; Name = "dev server" })) {
        $process = Get-Tracked $entry.File
        if ($process) {
            Stop-Tree $process
            Write-Host "Stopped AeroShoot $($entry.Name) (pid $($process.Id))."
            $stopped = $true
        }
        # Run's own cleanup may remove it concurrently.
        Remove-Item -Force -ErrorAction SilentlyContinue $entry.File
    }
    if (-not $stopped) {
        Write-Host "Nothing started by this launcher is running."
        return
    }
    # Start needs the port back (strictPort).
    if (-not (Wait-DevServer -Listening $false -Seconds 10)) {
        Write-Warning "Port $DevPort is still in use by another process."
    }
}

function Invoke-Run {
    Assert-Tools -Rust
    if (-not (Test-Path (Join-Path $FrontendDir "node_modules"))) {
        Install-FrontendDependencies
    }
    $running = Get-Tracked $AppPidFile
    if ($running) {
        throw "The AeroShoot desktop app is already running (pid $($running.Id)). Run Stop first."
    }
    # Reuse a dev server that is already up (for example from Start).
    $vite = $null
    if (-not (Test-DevServer)) {
        Write-Host "==> Starting Vite on port $DevPort..."
        $vite = Start-DevServer -Hidden
        if (-not (Wait-DevServer -Listening $true -Seconds 60 -Process $vite)) {
            Stop-Tree $vite
            Clear-Tracked $DevPidFile $vite
            throw "Vite did not start listening on port $DevPort."
        }
    }
    try {
        Write-Host "==> Running AeroShoot (debug)..."
        $app = Start-Process -FilePath "cargo" -NoNewWindow -PassThru `
            -ArgumentList @("run", "--manifest-path", "`"$Manifest`"", "--features", "tauri-app")
        $null = $app.Handle
        Save-Tracked $AppPidFile $app
        try {
            $app.WaitForExit()
            return $app.ExitCode
        } finally {
            Clear-Tracked $AppPidFile $app
        }
    } finally {
        if ($vite) {
            if (-not $vite.HasExited) { Stop-Tree $vite }
            Clear-Tracked $DevPidFile $vite
        }
    }
}

function Invoke-Validate {
    Write-Host "==> [1/5] Validating: frontend tests..."
    Push-Location $FrontendDir
    try {
        Invoke-Native "npm test" { npm test }
    } finally {
        Pop-Location
    }
    Write-Host "==> [2/5] Validating: Rust unit and integration tests..."
    Invoke-Native "cargo test" {
        cargo test --manifest-path $Manifest --no-default-features --lib --tests
    }
}

function Invoke-Build {
    Write-Host "==> [0/5] Checking build prerequisites..."
    Assert-Tools -Rust
    Write-Host "    Node:   $(node -v)"
    Write-Host "    npm:    v$(npm -v)"
    Write-Host "    Rust:   $(rustc --version)"
    if (-not (Test-Path (Join-Path $FrontendDir "node_modules"))) {
        Install-FrontendDependencies
    }

    Invoke-Validate

    $config = Get-Content (Join-Path $TauriDir "tauri.conf.json") -Raw | ConvertFrom-Json
    $version = $config.version
    $arch = switch ($env:PROCESSOR_ARCHITECTURE) {
        "ARM64" { "arm64" }
        default { "x64" }
    }
    $bundleDir = Join-Path $TargetDir "release\bundle\windows"

    Write-Host "==> [3/5] Building frontend production bundle..."
    foreach ($path in @(
            (Join-Path $FrontendDir "dist"),
            (Join-Path $FrontendDir "node_modules\.vite"),
            $bundleDir,
            (Join-Path $TargetDir "release\bundle\nsis"),
            (Join-Path $TargetDir "release\bundle\msi"))) {
        if (Test-Path $path) {
            Remove-Item -Recurse -Force $path
            Write-Host "    Removed $path"
        }
    }
    Push-Location $FrontendDir
    try {
        Invoke-Native "npm run build" { npm run build }
    } finally {
        Pop-Location
    }

    # Cargo embeds the frontend (custom-protocol). The Tauri CLI, when
    # installed, runs the same release build and also wraps it in installers.
    if (Get-Command "cargo-tauri" -ErrorAction SilentlyContinue) {
        Write-Host "==> [4/5] Compiling desktop application and installers..."
        Push-Location $TauriDir
        try {
            Invoke-Native "cargo tauri build" {
                cargo tauri build --features tauri-app --bundles nsis,msi --ci
            }
        } finally {
            Pop-Location
        }
    } else {
        Write-Host "==> [4/5] Compiling desktop application..."
        Write-Host "    Tauri CLI not found; skipping installers (cargo install tauri-cli --locked)."
        Invoke-Native "cargo build" {
            cargo build --release --manifest-path $Manifest --features tauri-app,custom-protocol
        }
    }

    Write-Host "==> [5/5] Packaging..."
    New-Item -ItemType Directory -Force -Path $bundleDir | Out-Null
    $exe = Join-Path $bundleDir "AeroShoot.exe"
    Copy-Item (Join-Path $TargetDir "release\aeroshoot.exe") $exe
    $zip = Join-Path $bundleDir "AeroShoot_${version}_${arch}.zip"
    Compress-Archive -Path $exe -DestinationPath $zip -Force

    Write-Host ""
    Write-Host "Build complete. Requires the Microsoft Edge WebView2 Runtime (included with Windows 11)."
    Write-Host "    App: $exe"
    Write-Host "    Zip: $zip"
    $installers = @(
        Get-ChildItem (Join-Path $TargetDir "release\bundle\nsis\*-setup.exe") -ErrorAction SilentlyContinue
        Get-ChildItem (Join-Path $TargetDir "release\bundle\msi\*.msi") -ErrorAction SilentlyContinue
    )
    foreach ($installer in $installers) {
        Write-Host "    Installer: $($installer.FullName)"
    }
}

$exitCode = 0
try {
    switch ($Mode) {
        "setup" { Invoke-Setup }
        "start" { $exitCode = Invoke-Start }
        "stop" { Invoke-Stop }
        "run" { $exitCode = Invoke-Run }
        "build" { Invoke-Build }
        default { Show-Usage }
    }
} catch {
    Write-Host "Error: $($_.Exception.Message)" -ForegroundColor Red
    $exitCode = 1
}
exit $exitCode
