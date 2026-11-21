<#
.SYNOPSIS
    Supervises todo-server and the cloudflared connector for the per-user Windows pilot.

.DESCRIPTION
    Runs without admin rights. Meant to be started at logon by the scheduled task that
    install-pilot.ps1 registers, inside a headless console so no window appears.

    - Starts todo-server and waits until GET /health answers 200.
    - Only then starts cloudflared, so the tunnel never advertises a dead origin.
    - Restarts either process when it exits, with exponential backoff (5 s up to 5 min) if it keeps
      crashing quickly.
    - Writes process output to dated log files under <DataDir>\logs and prunes logs older than 14 days.
    - A named mutex keeps a single supervisor per user. stop-pilot.ps1 stops it cleanly.

    Nothing here is deployment-specific: the tunnel token is read by cloudflared from a file
    (--token-file), and every path defaults to %LOCALAPPDATA%\todo.
#>
[CmdletBinding()]
param(
    [string]$DataDir = (Join-Path $env:LOCALAPPDATA 'todo'),
    [string]$ServerExe = '',
    [string]$ServerConfig = '',
    [string]$HealthUrl = 'http://127.0.0.1:47320/health',
    [string]$CloudflaredExe = '',
    [string]$TunnelTokenFile = '',
    [switch]$NoTunnel
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Paths not given explicitly live under the data directory.
if (-not $ServerExe) { $ServerExe = Join-Path $DataDir 'bin\todo-server.exe' }
if (-not $ServerConfig) { $ServerConfig = Join-Path $DataDir 'server.toml' }
if (-not $TunnelTokenFile) { $TunnelTokenFile = Join-Path $DataDir 'cloudflared-token' }

$logDir = Join-Path $DataDir 'logs'
$stateFile = Join-Path $DataDir 'pilot.pids.json'
$stopFile = Join-Path $DataDir 'pilot.stop'
New-Item -ItemType Directory -Force -Path $logDir | Out-Null

function Write-PilotLog([string]$Message) {
    $line = '{0:yyyy-MM-ddTHH:mm:ss.fffK} {1}' -f (Get-Date), $Message
    Add-Content -Path (Join-Path $logDir ('pilot-{0:yyyy-MM-dd}.log' -f (Get-Date))) -Value $line
}

$mutex = New-Object System.Threading.Mutex($false, 'Local\todo-pilot-supervisor')
if (-not $mutex.WaitOne(0)) {
    Write-PilotLog 'another supervisor is already running; exiting'
    exit 0
}

if (-not $CloudflaredExe) {
    $cmd = Get-Command cloudflared -ErrorAction SilentlyContinue
    if ($cmd) { $CloudflaredExe = $cmd.Source }
}

function Test-Health {
    try {
        $r = Invoke-WebRequest -Uri $HealthUrl -UseBasicParsing -TimeoutSec 3
        return $r.StatusCode -eq 200
    } catch {
        return $false
    }
}

function New-Child([string]$Name, [string]$Exe, [string[]]$Arguments) {
    [pscustomobject]@{
        Name = $Name; Exe = $Exe; Arguments = $Arguments
        Process = $null; StartedAt = [datetime]::MinValue
        Backoff = 5; NextStart = [datetime]::MinValue
    }
}

function Start-Child($Child) {
    $stamp = '{0:yyyy-MM-dd}' -f (Get-Date)
    $out = Join-Path $logDir "$($Child.Name)-$stamp.out.log"
    $err = Join-Path $logDir "$($Child.Name)-$stamp.err.log"
    $Child.Process = Start-Process -FilePath $Child.Exe -ArgumentList $Child.Arguments `
        -WindowStyle Hidden -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
    $Child.StartedAt = Get-Date
    Write-PilotLog "started $($Child.Name) (pid $($Child.Process.Id))"
}

function Save-State($Children) {
    $pids = @{ supervisor = $PID }
    foreach ($c in $Children) {
        if ($c.Process -and -not $c.Process.HasExited) { $pids[$c.Name] = $c.Process.Id }
    }
    $pids | ConvertTo-Json | Set-Content -Path $stateFile
}

function Stop-Children($Children) {
    # Stop the tunnel first so no traffic reaches a server that is shutting down.
    foreach ($c in @($Children | Sort-Object { if ($_.Name -eq 'cloudflared') { 0 } else { 1 } })) {
        if ($c.Process -and -not $c.Process.HasExited) {
            Write-PilotLog "stopping $($c.Name) (pid $($c.Process.Id))"
            Stop-Process -Id $c.Process.Id -ErrorAction SilentlyContinue
            $c.Process.WaitForExit(10000) | Out-Null
        }
    }
}

if (-not (Test-Path $ServerExe)) {
    Write-PilotLog "todo-server not found at $ServerExe; exiting"
    exit 1
}
# The config path goes through the environment, which the child inherits.
if (Test-Path $ServerConfig) { $env:TODO_SERVER_CONFIG = $ServerConfig }
$server = New-Child 'todo-server' $ServerExe @('serve')
$children = @($server)

$tunnel = $null
if (-not $NoTunnel) {
    if (-not $CloudflaredExe -or -not (Test-Path $CloudflaredExe)) {
        Write-PilotLog 'cloudflared not found; running without the tunnel'
    } elseif (-not (Test-Path $TunnelTokenFile)) {
        Write-PilotLog "tunnel token file not found at $TunnelTokenFile; running without the tunnel"
    } else {
        $tunnel = New-Child 'cloudflared' $CloudflaredExe @(
            'tunnel', '--no-autoupdate', 'run', '--token-file', "`"$TunnelTokenFile`"")
        $children += $tunnel
    }
}

# A supervisor that was killed outright (not stopped via stop-pilot.ps1) leaves its children running.
# Clean them up, but only processes that still look like ours.
if (Test-Path $stateFile) {
    try {
        $old = Get-Content -Raw -Path $stateFile | ConvertFrom-Json
        foreach ($name in @('cloudflared', 'todo-server')) {
            $oldPid = $old.$name
            if ($oldPid) {
                $p = Get-Process -Id $oldPid -ErrorAction SilentlyContinue
                if ($p -and $p.ProcessName -eq $name) {
                    Write-PilotLog "stopping leftover $name (pid $oldPid)"
                    Stop-Process -Id $oldPid -ErrorAction SilentlyContinue
                }
            }
        }
    } catch {
        Write-PilotLog "could not read $stateFile; ignoring"
    }
}

Remove-Item -Path $stopFile -ErrorAction SilentlyContinue
Write-PilotLog "supervisor started (pid $PID)"

# Prune old logs once per start.
Get-ChildItem -Path $logDir -Filter '*.log' |
    Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-14) } |
    Remove-Item -ErrorAction SilentlyContinue

try {
    while (-not (Test-Path $stopFile)) {
        foreach ($c in $children) {
            $running = $c.Process -and -not $c.Process.HasExited
            if ($running) { continue }

            if ($c.Process) {
                $uptime = (Get-Date) - $c.StartedAt
                Write-PilotLog ("{0} exited with code {1} after {2:N0} s" -f $c.Name, $c.Process.ExitCode, $uptime.TotalSeconds)
                # Healthy runs reset the backoff; quick crashes double it, up to five minutes.
                if ($uptime.TotalSeconds -ge 60) { $c.Backoff = 5 } else { $c.Backoff = [Math]::Min($c.Backoff * 2, 300) }
                $c.NextStart = (Get-Date).AddSeconds($c.Backoff)
                $c.Process = $null
            }
            if ((Get-Date) -lt $c.NextStart) { continue }

            # The tunnel waits for a healthy origin.
            if ($c -eq $tunnel -and -not (Test-Health)) { continue }
            Start-Child $c
        }
        Save-State $children
        Start-Sleep -Seconds 2
    }
    Write-PilotLog 'stop requested'
} finally {
    Stop-Children $children
    Remove-Item -Path $stateFile, $stopFile -ErrorAction SilentlyContinue
    Write-PilotLog 'supervisor stopped'
    $mutex.ReleaseMutex()
}
