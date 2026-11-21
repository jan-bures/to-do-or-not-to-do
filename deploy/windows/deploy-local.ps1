<#
.SYNOPSIS
    Builds todo-server and todo-backup and deploys them to the Windows pilot, rolling back if the new
    server doesn't come up healthy.

.DESCRIPTION
    Runs under Windows PowerShell 5.1 (or PowerShell 7) and needs no admin rights.

    1. Builds both binaries in release mode with --locked, in -RepoPath.
    2. Stops the pilot with stop-pilot.ps1 and checks that nothing answers the health URL any more.
    3. Copies the database files (todo.sqlite3 and its -wal and -shm) to
       <DataDir>\deploy\pre-<timestamp>\, keeping the newest -KeepSnapshots copies. The server is
       stopped, so the copy is consistent.
    4. Keeps the current binaries as <name>.exe.prev and copies in the new ones.
    5. Migrations need no separate step: todo-server applies any pending ones when it opens the
       database at startup, before it starts listening, so /health answers only once they are done.
    6. Starts the pilot again (install-pilot.ps1 -NoRegister -Start, which refreshes bin\run-pilot.ps1
       but leaves the scheduled task alone) and waits for /health.
    7. If /health doesn't answer 200 within -HealthTimeoutSeconds, it stops the pilot, puts the .prev
       binaries back and starts it again. If the previous server doesn't come up either (for example
       because the new one ran a migration the old one doesn't know), it moves the database files to
       <DataDir>\deploy\failed-<timestamp>\, restores the copy from step 3 and tries once more.

    The health URL comes from `bind` in <DataDir>\server.toml (default 127.0.0.1:47320), and the
    database directory from `data_dir` there (or TODO_SERVER_DATA_DIR), unless given explicitly.

    Exit codes: 0 deployed; 1 nothing was changed (build failed, or the pilot didn't stop); 2 rolled
    back, the previous version is running; 3 rollback failed, the pilot is down.

.PARAMETER DataDir
    The pilot's data directory. Defaults to %LOCALAPPDATA%\todo.

.PARAMETER RepoPath
    The checkout to build. Defaults to the one this script is in.

.PARAMETER ArtifactDir
    Where the built binaries are. Defaults to <target dir>\release.

.PARAMETER SkipBuild
    Deploy the binaries already in -ArtifactDir.

.EXAMPLE
    ./deploy/windows/deploy-local.ps1

.EXAMPLE
    ./deploy/windows/deploy-local.ps1 -DataDir D:\todo-test -NoTunnel
#>
[CmdletBinding()]
param(
    [string]$DataDir = (Join-Path $env:LOCALAPPDATA 'todo'),
    [string]$RepoPath = '',
    [string]$ArtifactDir = '',
    [string]$HealthUrl = '',
    [string]$DatabaseDir = '',
    [int]$HealthTimeoutSeconds = 60,
    [int]$KeepSnapshots = 3,
    [switch]$NoTunnel,
    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$binaries = @('todo-server', 'todo-backup')

if (-not $RepoPath) { $RepoPath = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path }
$DataDir = [IO.Path]::GetFullPath($DataDir)
$binDir = Join-Path $DataDir 'bin'
$deployDir = Join-Path $DataDir 'deploy'
$configFile = Join-Path $DataDir 'server.toml'
$stamp = '{0:yyyyMMdd-HHmmss}' -f (Get-Date)

function Write-Step([string]$Message) { Write-Host "==> $Message" -ForegroundColor Cyan }

# A top-level string setting from server.toml (before the first [table]), or ''.
function Get-TomlString([string]$Key) {
    if (-not (Test-Path $configFile)) { return '' }
    foreach ($line in Get-Content -Path $configFile) {
        if ($line -match '^\s*\[') { break }
        if ($line -match "^\s*$Key\s*=\s*""((?:[^""\\]|\\.)*)""") { return ($Matches[1] -replace '\\\\', '\') }
        if ($line -match "^\s*$Key\s*=\s*'([^']*)'") { return $Matches[1] }
    }
    return ''
}

if (-not $HealthUrl) {
    $bind = Get-TomlString 'bind'
    if (-not $bind) { $bind = '127.0.0.1:47320' }
    $bind = $bind -replace '^0\.0\.0\.0:', '127.0.0.1:' -replace '^\[::\]:', '[::1]:'
    $HealthUrl = "http://$bind/health"
}

if (-not $DatabaseDir) {
    $DatabaseDir = $env:TODO_SERVER_DATA_DIR
    if (-not $DatabaseDir) { $DatabaseDir = Get-TomlString 'data_dir' }
}
if ($DatabaseDir -and -not [IO.Path]::IsPathRooted($DatabaseDir)) {
    Write-Warning "data_dir '$DatabaseDir' is relative, so it depends on the server's working directory. Set an absolute path; skipping the database copy."
    $DatabaseDir = ''
}
if (-not $DatabaseDir) {
    Write-Warning 'No absolute data_dir found; the deploy will not copy the database, and a rollback cannot restore it.'
}

# Mirror -NoTunnel from the registered task when it runs this data directory.
if (-not $NoTunnel) {
    try {
        $task = Get-ScheduledTask -TaskPath '\todo\' -TaskName 'pilot' -ErrorAction Stop
        $args0 = [string]$task.Actions[0].Arguments
        if ($args0 -like "*-DataDir `"$DataDir`"*" -and $args0 -like '*-NoTunnel*') { $NoTunnel = $true }
    } catch { }
}

function Test-Health {
    try {
        $r = Invoke-WebRequest -Uri $HealthUrl -UseBasicParsing -TimeoutSec 3
        return $r.StatusCode -eq 200
    } catch {
        return $false
    }
}

function Wait-Health([int]$Seconds) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        if (Test-Health) { return $true }
        Start-Sleep -Milliseconds 500
    }
    return $false
}

function Stop-Pilot {
    & (Join-Path $PSScriptRoot 'stop-pilot.ps1') -DataDir $DataDir
    # The binary stays locked for a moment after its process exits.
    $exe = Join-Path $binDir 'todo-server.exe'
    $deadline = (Get-Date).AddSeconds(30)
    while ((Test-Path $exe) -and (Get-Date) -lt $deadline) {
        try {
            $f = [IO.File]::Open($exe, 'Open', 'ReadWrite', 'None')
            $f.Dispose()
            break
        } catch {
            Start-Sleep -Milliseconds 250
        }
    }
}

function Start-Pilot {
    $a = @{ DataDir = $DataDir; NoRegister = $true; Start = $true; HealthUrl = $HealthUrl }
    if ($NoTunnel) { $a['NoTunnel'] = $true }
    & (Join-Path $PSScriptRoot 'install-pilot.ps1') @a
}

$dbFiles = @('todo.sqlite3', 'todo.sqlite3-wal', 'todo.sqlite3-shm')

function Save-Database {
    if (-not $DatabaseDir -or -not (Test-Path (Join-Path $DatabaseDir 'todo.sqlite3'))) { return $null }
    $dir = Join-Path $deployDir "pre-$stamp"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    foreach ($f in $dbFiles) {
        $src = Join-Path $DatabaseDir $f
        if (Test-Path $src) { Copy-Item -Path $src -Destination $dir }
    }
    # Keep only the newest copies.
    Get-ChildItem -Path $deployDir -Directory -Filter 'pre-*' | Sort-Object Name -Descending |
        Select-Object -Skip $KeepSnapshots | Remove-Item -Recurse -Force
    return $dir
}

function Restore-Database([string]$From) {
    $aside = Join-Path $deployDir "failed-$stamp"
    New-Item -ItemType Directory -Force -Path $aside | Out-Null
    foreach ($f in $dbFiles) {
        $p = Join-Path $DatabaseDir $f
        if (Test-Path $p) { Move-Item -Path $p -Destination $aside }
    }
    foreach ($f in $dbFiles) {
        $p = Join-Path $From $f
        if (Test-Path $p) { Copy-Item -Path $p -Destination $DatabaseDir }
    }
    Write-Host "Restored the database from $From; the files it replaced are in $aside."
}

# ---- 1. build ------------------------------------------------------------------------------------

if (-not $ArtifactDir) {
    $target = $env:CARGO_TARGET_DIR
    if (-not $target) { $target = Join-Path $RepoPath 'target' }
    $ArtifactDir = Join-Path $target 'release'
}
if (-not $SkipBuild) {
    Write-Step "Building in $RepoPath"
    Push-Location $RepoPath
    try {
        & cargo build --release --locked -p todo-server -p todo-backup
        if ($LASTEXITCODE -ne 0) { Write-Error "cargo build failed with exit code $LASTEXITCODE; nothing was changed." -ErrorAction Continue; exit 1 }
    } finally {
        Pop-Location
    }
}
foreach ($b in $binaries) {
    if (-not (Test-Path (Join-Path $ArtifactDir "$b.exe"))) {
        Write-Error "$b.exe is not in $ArtifactDir; nothing was changed." -ErrorAction Continue
        exit 1
    }
}

# ---- 2. stop -------------------------------------------------------------------------------------

Write-Step "Stopping the pilot in $DataDir"
Stop-Pilot
if (Test-Health) {
    Write-Error "Something still answers $HealthUrl after the pilot stopped; not deploying over it. Starting the pilot again." -ErrorAction Continue
    Start-Pilot
    exit 1
}

# ---- 3. database copy ----------------------------------------------------------------------------

$snapshot = Save-Database
if ($snapshot) { Write-Step "Copied the database to $snapshot" }

# ---- 4. swap -------------------------------------------------------------------------------------

Write-Step "Installing the new binaries in $binDir"
New-Item -ItemType Directory -Force -Path $binDir | Out-Null
$hadPrevious = @{}
foreach ($b in $binaries) {
    $dst = Join-Path $binDir "$b.exe"
    $hadPrevious[$b] = Test-Path $dst
    if ($hadPrevious[$b]) { Copy-Item -Path $dst -Destination "$dst.prev" -Force }
    Copy-Item -Path (Join-Path $ArtifactDir "$b.exe") -Destination $dst -Force
}

# ---- 5-6. start and check ------------------------------------------------------------------------

Write-Step "Starting the pilot; waiting up to $HealthTimeoutSeconds s for $HealthUrl"
Start-Pilot
if (Wait-Health $HealthTimeoutSeconds) {
    Write-Host "Deployed. $HealthUrl answers 200." -ForegroundColor Green
    exit 0
}

# ---- 7. roll back --------------------------------------------------------------------------------

Write-Warning "The new server did not become healthy. Rolling back."
Write-Host "Its log is in $(Join-Path $DataDir 'logs')."
Stop-Pilot
if (-not $hadPrevious['todo-server']) {
    Write-Error 'There is no previous todo-server to roll back to; the pilot is stopped.' -ErrorAction Continue
    exit 3
}
foreach ($b in $binaries) {
    $dst = Join-Path $binDir "$b.exe"
    if ($hadPrevious[$b]) {
        Copy-Item -Path "$dst.prev" -Destination $dst -Force
    } else {
        Remove-Item -Path $dst -Force -ErrorAction SilentlyContinue
    }
}
Start-Pilot
if (Wait-Health $HealthTimeoutSeconds) {
    Write-Warning "Rolled back: the previous version is running."
    exit 2
}

if ($snapshot) {
    Write-Warning 'The previous version did not become healthy either. Restoring the database copy.'
    Stop-Pilot
    Restore-Database $snapshot
    Start-Pilot
    if (Wait-Health $HealthTimeoutSeconds) {
        Write-Warning "Rolled back, with the database from before the deploy: the previous version is running."
        exit 2
    }
}
Write-Error "Rollback failed: nothing answers $HealthUrl. See the logs in $(Join-Path $DataDir 'logs')." -ErrorAction Continue
exit 3
