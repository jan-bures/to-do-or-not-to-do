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
    - If <DataDir>\bin\todo-backup.exe and <DataDir>\backup.toml both exist, it also runs encrypted
      backups: `run --kind frequent` every 6 hours and `run --kind daily` once per calendar day. Each run is
      a separate short-lived process that the loop only polls, so a slow or failing backup never holds up
      supervising the server. The last runs are kept in <DataDir>\backup-schedule.json, so a restart does
      not back up again straight away.

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
    [switch]$NoTunnel,
    [string]$BackupExe = '',
    [string]$BackupConfig = '',
    # Minutes between frequent backups. Each run that changed nothing costs no request at all.
    [int]$FrequentBackupMinutes = 360,
    # After a failed run, wait this long before trying the same kind again.
    [int]$BackupRetryMinutes = 30,
    # Give the server time to start (and run migrations) before the first backup.
    [int]$BackupStartDelayMinutes = 5,
    [int]$BackupTimeoutMinutes = 60
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Paths not given explicitly live under the data directory.
if (-not $ServerExe) { $ServerExe = Join-Path $DataDir 'bin\todo-server.exe' }
if (-not $ServerConfig) { $ServerConfig = Join-Path $DataDir 'server.toml' }
if (-not $TunnelTokenFile) { $TunnelTokenFile = Join-Path $DataDir 'cloudflared-token' }
if (-not $BackupExe) { $BackupExe = Join-Path $DataDir 'bin\todo-backup.exe' }
if (-not $BackupConfig) { $BackupConfig = Join-Path $DataDir 'backup.toml' }

$logDir = Join-Path $DataDir 'logs'
$stateFile = Join-Path $DataDir 'pilot.pids.json'
$stopFile = Join-Path $DataDir 'pilot.stop'
$backupScheduleFile = Join-Path $DataDir 'backup-schedule.json'
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
    # Touching Handle now keeps ExitCode readable after the process exits.
    $null = $Child.Process.Handle
    $Child.StartedAt = Get-Date
    Write-PilotLog "started $($Child.Name) (pid $($Child.Process.Id))"
}

function Save-State($Children) {
    $pids = @{ supervisor = $PID }
    foreach ($c in $Children) {
        if ($c.Process -and -not $c.Process.HasExited) { $pids[$c.Name] = $c.Process.Id }
    }
    if ($script:backupJob -and -not $script:backupJob.Process.HasExited) {
        $pids['todo-backup'] = $script:backupJob.Process.Id
    }
    $pids | ConvertTo-Json | Set-Content -Path $stateFile
}

# Reads a property that may be missing. Under StrictMode, $obj.$name throws for a missing property.
function Get-JsonValue($Object, [string]$Name) {
    if ($null -eq $Object) { return $null }
    $p = $Object.PSObject.Properties[$Name]
    if ($p) { return $p.Value }
    return $null
}

# --- Backups -------------------------------------------------------------------------------------------

$script:backupJob = $null
$backupKinds = @('daily', 'frequent')   # daily first when both are due
$backupNotBefore = (Get-Date).ToUniversalTime().AddMinutes($BackupStartDelayMinutes)

function ConvertTo-UtcTime($Value) {
    if ($null -eq $Value -or $Value -eq '') { return [datetime]::MinValue }
    # Windows PowerShell keeps ISO strings as strings; PowerShell 7 turns them into DateTime already.
    if ($Value -is [datetime]) { return $Value.ToUniversalTime() }
    try {
        return [datetime]::Parse([string]$Value, [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind).ToUniversalTime()
    } catch {
        return [datetime]::MinValue
    }
}

function Read-BackupSchedule {
    $schedule = @{}
    $json = $null
    if (Test-Path $backupScheduleFile) {
        try {
            $json = Get-Content -Raw -Path $backupScheduleFile | ConvertFrom-Json
        } catch {
            Write-PilotLog "could not read $backupScheduleFile; treating every backup as due"
        }
    }
    foreach ($kind in $backupKinds) {
        foreach ($field in @('LastSuccess', 'LastAttempt')) {
            $schedule["$kind$field"] = ConvertTo-UtcTime (Get-JsonValue $json "$kind$field")
        }
    }
    return $schedule
}

function Save-BackupSchedule($Schedule) {
    $out = [ordered]@{}
    foreach ($key in ($Schedule.Keys | Sort-Object)) {
        $t = $Schedule[$key]
        if ($t -ne [datetime]::MinValue) { $out[$key] = $t.ToString('o') } else { $out[$key] = $null }
    }
    $tmp = "$backupScheduleFile.tmp"
    $out | ConvertTo-Json | Set-Content -Path $tmp
    Move-Item -Path $tmp -Destination $backupScheduleFile -Force
}

function Test-BackupDue([string]$Kind, $Schedule, [datetime]$NowUtc) {
    $lastSuccess = $Schedule["${Kind}LastSuccess"]
    $lastAttempt = $Schedule["${Kind}LastAttempt"]
    # A run that failed waits before it's retried, so a broken config doesn't spin.
    if ($lastAttempt -gt $lastSuccess -and ($NowUtc - $lastAttempt).TotalMinutes -lt $BackupRetryMinutes) {
        return $false
    }
    if ($Kind -eq 'daily') {
        if ($lastSuccess -eq [datetime]::MinValue) { return $true }
        return $lastSuccess.ToLocalTime().Date -lt $NowUtc.ToLocalTime().Date
    }
    return ($NowUtc - $lastSuccess).TotalMinutes -ge $FrequentBackupMinutes
}

function Start-BackupJob([string]$Kind, $Schedule, [datetime]$NowUtc) {
    $stamp = '{0:yyyyMMdd-HHmmss}' -f (Get-Date)
    $out = Join-Path $logDir "backup-$Kind-$stamp.out.tmp"
    $err = Join-Path $logDir "backup-$Kind-$stamp.err.tmp"
    $Schedule["${Kind}LastAttempt"] = $NowUtc
    Save-BackupSchedule $Schedule
    $p = Start-Process -FilePath $BackupExe -ArgumentList @(
        '--config', "`"$BackupConfig`"", 'run', '--kind', $Kind) `
        -WindowStyle Hidden -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
    # Touching Handle now keeps ExitCode readable after the process exits.
    $null = $p.Handle
    $script:backupJob = [pscustomobject]@{
        Kind = $Kind; Process = $p; StartedAt = $NowUtc; Out = $out; Err = $err
        KilledAt = $null; LastKillAttempt = $null
    }
    Write-PilotLog "started backup $Kind (pid $($p.Id))"
}

# Appends a finished run's output to the day's backup log and removes the temporary files.
function Complete-BackupJob($Job, [string]$Result) {
    $log = Join-Path $logDir ('backup-{0:yyyy-MM-dd}.log' -f (Get-Date))
    $lines = @('{0:yyyy-MM-ddTHH:mm:ss.fffK} backup {1}: {2}' -f (Get-Date), $Job.Kind, $Result)
    foreach ($f in @($Job.Out, $Job.Err)) {
        if (Test-Path $f) {
            $lines += @(Get-Content -Path $f | ForEach-Object { "  $_" })
            Remove-Item -Path $f -ErrorAction SilentlyContinue
        }
    }
    Add-Content -Path $log -Value $lines
}

function Invoke-BackupScheduler {
    $nowUtc = (Get-Date).ToUniversalTime()
    $job = $script:backupJob
    if ($job) {
        # This runs on every supervisor tick, so it never waits: it looks at the job and returns.
        if (-not $job.Process.HasExited) {
            if (($nowUtc - $job.StartedAt).TotalMinutes -lt $BackupTimeoutMinutes) { return }
            # Too slow. Kill it through the handle kept since it started (immune to PID reuse), then keep
            # tracking it until it has really exited, so a second backup never starts next to a live one.
            # Retry the kill at most once a minute.
            if ($null -eq $job.LastKillAttempt -or ($nowUtc - $job.LastKillAttempt).TotalSeconds -ge 60) {
                if ($null -eq $job.KilledAt) {
                    Write-PilotLog "backup $($job.Kind) ran longer than $BackupTimeoutMinutes min; killing it"
                    $job.KilledAt = $nowUtc
                } else {
                    Write-PilotLog "backup $($job.Kind) (pid $($job.Process.Id)) is still running after a kill; retrying"
                }
                $job.LastKillAttempt = $nowUtc
                try { $job.Process.Kill() } catch { Write-PilotLog "could not kill backup $($job.Kind): $_" }
            }
            return
        }
        $code = $job.Process.ExitCode
        Write-PilotLog "backup $($job.Kind) exited with code $code"
        if ($null -ne $job.KilledAt) {
            # Whatever the exit code says, a killed run is not a success.
            Complete-BackupJob $job "timed out after $BackupTimeoutMinutes min and was killed"
        } elseif ($code -eq 0) {
            $schedule = Read-BackupSchedule
            # Count from the start, so the cadence doesn't drift by the run time.
            $schedule["$($job.Kind)LastSuccess"] = $job.StartedAt
            Save-BackupSchedule $schedule
            Complete-BackupJob $job 'ok'
        } elseif ($code -eq 3) {
            Complete-BackupJob $job 'refused: monthly operations budget reached'
        } else {
            Complete-BackupJob $job "failed with exit code $code"
        }
        $script:backupJob = $null
        return
    }
    if ($nowUtc -lt $backupNotBefore) { return }
    $schedule = Read-BackupSchedule
    foreach ($kind in $backupKinds) {
        if (Test-BackupDue $kind $schedule $nowUtc) {
            Start-BackupJob $kind $schedule $nowUtc
            return
        }
    }
}

function Stop-BackupJob {
    $job = $script:backupJob
    if ($job -and -not $job.Process.HasExited) {
        # Safe to interrupt: nothing is recorded until an upload completes, and the next run cleans up.
        # Only on shutdown, outside the supervision loop, is a short wait acceptable.
        Write-PilotLog "stopping backup $($job.Kind) (pid $($job.Process.Id))"
        try { $job.Process.Kill() } catch { Write-PilotLog "could not kill backup $($job.Kind): $_" }
        if ($job.Process.WaitForExit(10000)) {
            Complete-BackupJob $job 'stopped with the supervisor'
        } else {
            Write-PilotLog "backup $($job.Kind) (pid $($job.Process.Id)) did not exit; the next start cleans up after it"
        }
    }
    $script:backupJob = $null
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
        foreach ($name in @('cloudflared', 'todo-server', 'todo-backup')) {
            $oldPid = Get-JsonValue $old $name
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

$backupEnabled = (Test-Path $BackupExe) -and (Test-Path $BackupConfig)
if ($backupEnabled) {
    Write-PilotLog ("backups enabled: frequent every {0} min, daily once a day, first check after {1} min" -f `
        $FrequentBackupMinutes, $BackupStartDelayMinutes)
} else {
    Write-PilotLog 'backups disabled (needs bin\todo-backup.exe and backup.toml in the data directory)'
}

Remove-Item -Path $stopFile -ErrorAction SilentlyContinue
Write-PilotLog "supervisor started (pid $PID)"

# Prune old logs once per start.
Get-ChildItem -Path $logDir -Filter '*.log' |
    Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-14) } |
    Remove-Item -ErrorAction SilentlyContinue
# Output of backup runs that were cut off by a killed supervisor.
Get-ChildItem -Path $logDir -Filter 'backup-*.tmp' |
    Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-1) } |
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
        if ($backupEnabled) {
            # A scheduling problem must never stop the supervisor from looking after the server.
            try { Invoke-BackupScheduler } catch { Write-PilotLog "backup scheduler error: $_" }
        }
        Save-State $children
        Start-Sleep -Seconds 2
    }
    Write-PilotLog 'stop requested'
} finally {
    Stop-BackupJob
    Stop-Children $children
    Remove-Item -Path $stateFile, $stopFile -ErrorAction SilentlyContinue
    Write-PilotLog 'supervisor stopped'
    $mutex.ReleaseMutex()
}
