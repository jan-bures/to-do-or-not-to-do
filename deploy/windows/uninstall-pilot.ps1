<#
.SYNOPSIS
    Stops the todo pilot and removes its scheduled task. Leaves data, logs and secrets in place.
#>
[CmdletBinding()]
param(
    [string]$DataDir = (Join-Path $env:LOCALAPPDATA 'todo')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

& (Join-Path $PSScriptRoot 'stop-pilot.ps1') -DataDir $DataDir

$task = Get-ScheduledTask -TaskName 'pilot' -TaskPath '\todo\' -ErrorAction SilentlyContinue
if ($task) {
    Unregister-ScheduledTask -TaskName 'pilot' -TaskPath '\todo\' -Confirm:$false
    Write-Host 'Removed scheduled task \todo\pilot.'
} else {
    Write-Host 'No scheduled task to remove.'
}
Write-Host "Data, logs and secrets in $DataDir were left in place."
