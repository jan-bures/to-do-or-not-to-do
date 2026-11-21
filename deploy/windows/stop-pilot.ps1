<#
.SYNOPSIS
    Stops the running todo pilot: cloudflared first, then todo-server, then the supervisor.

.DESCRIPTION
    Asks the supervisor to stop by writing <DataDir>\pilot.stop and waits for it to finish.
    If the supervisor doesn't stop within the timeout, or isn't running, the script stops the
    processes recorded in pilot.pids.json, but only if they still have the expected names.
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

if (-not (Test-Path $stateFile)) {
    Write-Host 'The pilot is not running.'
    exit 0
}

Set-Content -Path $stopFile -Value 'stop'
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
while ((Test-Path $stateFile) -and (Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 500
}

if (Test-Path $stateFile) {
    Write-Warning 'The supervisor did not stop in time; stopping the recorded processes directly.'
    $state = Get-Content -Raw -Path $stateFile | ConvertFrom-Json
    $expected = [ordered]@{ cloudflared = 'cloudflared'; 'todo-server' = 'todo-server'; supervisor = $null }
    foreach ($key in $expected.Keys) {
        $procId = $state.$key
        if (-not $procId) { continue }
        $p = Get-Process -Id $procId -ErrorAction SilentlyContinue
        if (-not $p) { continue }
        $name = $expected[$key]
        if ($name -and $p.ProcessName -ne $name) { continue }
        if (-not $name -and $p.ProcessName -notin @('pwsh', 'powershell')) { continue }
        Stop-Process -Id $procId -ErrorAction SilentlyContinue
    }
    Remove-Item -Path $stateFile, $stopFile -ErrorAction SilentlyContinue
}

Write-Host 'The pilot is stopped.'
