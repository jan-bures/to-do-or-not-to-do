<#
.SYNOPSIS
    Stops the running todo pilot: cloudflared first, then todo-server, then the supervisor.

.DESCRIPTION
    Asks the supervisor to stop by writing <DataDir>\pilot.stop and waits for it to finish.
    If the supervisor doesn't stop within the timeout, the script stops the processes recorded in
    pilot.pids.json, but only if they still have the expected names. A backup worker that outlived the
    supervisor is stopped only if its PID, name and start time all still match the record.
    The scheduled task stays registered, so the pilot starts again at the next logon.
#>
[CmdletBinding()]
param(
    [string]$DataDir = (Join-Path $env:LOCALAPPDATA 'todo'),
    [int]$TimeoutSeconds = 30
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$stateFile = Join-Path $DataDir 'pilot.pids.json'
$stopFile = Join-Path $DataDir 'pilot.stop'

# Reads a property that may be missing. Under StrictMode, $obj.$name throws for a missing property.
function Get-JsonValue($Object, [string]$Name) {
    if ($null -eq $Object) { return $null }
    $p = $Object.PSObject.Properties[$Name]
    if ($p) { return $p.Value }
    return $null
}

# The state file, $null if there is none, or 'unreadable' (for example while it is being rewritten).
function Read-PilotState {
    if (-not (Test-Path -LiteralPath $stateFile)) { return $null }
    try { return Get-Content -Raw -LiteralPath $stateFile | ConvertFrom-Json } catch { return 'unreadable' }
}

# The supervisor is running while the state file names it. On shutdown it removes the file, or leaves one
# without a supervisor entry that only records a backup worker which didn't exit.
function Test-SupervisorRecorded {
    $state = Read-PilotState
    if ($null -eq $state) { return $false }
    if ($state -is [string]) { return $true }
    return $null -ne (Get-JsonValue $state 'supervisor')
}

# The running backup worker a { pid, startTime } record describes, or $null unless PID, name and start time
# all still match (a reused PID is never touched).
function Find-RecordedBackup($Record) {
    $procId = Get-JsonValue $Record 'pid'
    $startText = Get-JsonValue $Record 'startTime'
    if (-not $procId -or -not $startText) { return $null }
    try {
        $start = if ($startText -is [datetime]) { $startText.ToUniversalTime() } else {
            [datetime]::Parse([string]$startText, [Globalization.CultureInfo]::InvariantCulture,
                [Globalization.DateTimeStyles]::RoundtripKind).ToUniversalTime()
        }
    } catch { return $null }
    $p = Get-Process -Id $procId -ErrorAction SilentlyContinue
    if (-not $p -or $p.ProcessName -ne 'todo-backup') { return $null }
    try { $actual = $p.StartTime.ToUniversalTime() } catch { return $null }
    if ([Math]::Abs(($actual - $start).TotalSeconds) -ge 1) { return $null }
    return $p
}

if (-not (Test-Path -LiteralPath $stateFile)) {
    Write-Host 'The pilot is not running.'
    exit 0
}

if (Test-SupervisorRecorded) {
    Set-Content -LiteralPath $stopFile -Value 'stop'
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Test-SupervisorRecorded) -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 500
    }
}

$state = Read-PilotState
if ($state -is [string]) {
    Write-Warning "Could not read $stateFile; leaving it in place."
} elseif ($null -ne $state) {
    if ($null -ne (Get-JsonValue $state 'supervisor')) {
        Write-Warning 'The supervisor did not stop in time; stopping the recorded processes directly.'
    }
    $expected = [ordered]@{ cloudflared = 'cloudflared'; 'todo-server' = 'todo-server'; supervisor = $null }
    foreach ($key in $expected.Keys) {
        $procId = Get-JsonValue $state $key
        if (-not $procId) { continue }
        $p = Get-Process -Id $procId -ErrorAction SilentlyContinue
        if (-not $p) { continue }
        $name = $expected[$key]
        if ($name -and $p.ProcessName -ne $name) { continue }
        if (-not $name -and $p.ProcessName -notin @('pwsh', 'powershell')) { continue }
        Stop-Process -Id $procId -ErrorAction SilentlyContinue
    }
    $record = Get-JsonValue $state 'todo-backup'
    $backup = if ($record) { Find-RecordedBackup $record } else { $null }
    if ($backup) {
        Write-Host "Stopping the backup worker (pid $($backup.Id))."
        try { $backup.Kill() } catch { }
    }
    if ($backup -and -not $backup.WaitForExit(10000)) {
        # Keep only its record, so the next supervisor start stops it.
        Write-Warning "The backup worker (pid $($backup.Id)) is still running; the next start stops it."
        @{ 'todo-backup' = $record } | ConvertTo-Json | Set-Content -LiteralPath $stateFile
    } else {
        Remove-Item -LiteralPath $stateFile -ErrorAction SilentlyContinue
    }
}
Remove-Item -LiteralPath $stopFile -ErrorAction SilentlyContinue

Write-Host 'The pilot is stopped.'
