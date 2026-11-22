<#
.SYNOPSIS
    Registers the per-user logon task that runs the todo pilot (todo-server plus cloudflared).

.DESCRIPTION
    Needs no admin rights. It registers an interactive, limited-rights scheduled task for the
    current user at \todo\pilot, triggered at logon. The task runs run-pilot.ps1 in a headless
    console (conhost --headless), so no window appears.

    The script copies run-pilot.ps1 to <DataDir>\bin so the task doesn't depend on this checkout.
    Pass -Start to start the pilot right away instead of waiting for the next logon.

    -NoRegister skips the scheduled task: with -Start it only refreshes bin\run-pilot.ps1 and starts
    the supervisor. deploy-local.ps1 uses it, so a deploy never rewrites the task, and a test deploy
    to another data directory never points the real task at it. -HealthUrl is passed on to the
    supervisor, for a server that doesn't listen on the default port.

.EXAMPLE
    ./install-pilot.ps1 -Start

.EXAMPLE
    ./install-pilot.ps1 -DataDir D:\todo-test -NoRegister -NoTunnel -Start -HealthUrl http://127.0.0.1:47391/health
#>
[CmdletBinding()]
param(
    [string]$DataDir = (Join-Path $env:LOCALAPPDATA 'todo'),
    [switch]$NoTunnel,
    [switch]$Start,
    [switch]$NoRegister,
    [string]$HealthUrl = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# run-pilot.ps1 refuses such a directory: Start-Process can't redirect its logs there.
if ($DataDir -match '[\[\]]') { throw "The data directory must not contain [ or ]: $DataDir" }

$binDir = Join-Path $DataDir 'bin'
New-Item -ItemType Directory -Force -Path $binDir | Out-Null
$runner = Join-Path $binDir 'run-pilot.ps1'
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'run-pilot.ps1') -Destination $runner -Force

$serverExe = Join-Path $binDir 'todo-server.exe'
if (-not (Test-Path -LiteralPath $serverExe)) {
    Write-Warning "todo-server.exe is not in $binDir yet. Copy it there before the pilot starts."
}

# Windows PowerShell 5.1 is always present at this path. PowerShell 7 installed from the Microsoft Store
# is an app execution alias, which doesn't start from a scheduled task, so it isn't used here.
$shell = Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\powershell.exe'

$runnerArgs = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$runner`" -DataDir `"$DataDir`""
if ($NoTunnel) { $runnerArgs += ' -NoTunnel' }
if ($HealthUrl) { $runnerArgs += " -HealthUrl `"$HealthUrl`"" }

if ($NoRegister) {
    Write-Host "Refreshed $runner. The scheduled task was left as it is."
} else {
    $user = "$env:USERDOMAIN\$env:USERNAME"
    $action = New-ScheduledTaskAction -Execute "$env:WINDIR\System32\conhost.exe" `
        -Argument "--headless `"$shell`" $runnerArgs"
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
    # Interactive and limited: runs as the signed-in user, needs no stored password and no elevation.
    $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -StartWhenAvailable -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew `
        -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1)

    Register-ScheduledTask -TaskName 'pilot' -TaskPath '\todo\' -Action $action -Trigger $trigger `
        -Principal $principal -Settings $settings -Force -ErrorAction Stop | Out-Null
    Write-Host "Registered scheduled task \todo\pilot for $user."
}

if ($Start) {
    # Start in this session directly. Start-ScheduledTask can pick another, disconnected session of the
    # same user, where an interactive task fails with 0xC0000142. The logon trigger always uses the
    # session that is logging on.
    Start-Process -FilePath "$env:WINDIR\System32\conhost.exe" -ArgumentList "--headless `"$shell`" $runnerArgs"
    Write-Host "Started. Logs: $(Join-Path $DataDir 'logs')"
}
