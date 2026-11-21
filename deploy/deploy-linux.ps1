<#
.SYNOPSIS
    Builds todo-server and todo-backup for Linux in WSL and deploys them to a provisioned Linux VM,
    rolling back there if the new server doesn't come up healthy.

.DESCRIPTION
    Runs on the Windows dev PC under Windows PowerShell 5.1 or PowerShell 7. The VM must have been set
    up with deploy/linux/provision.sh and have /etc/todo/server.toml (see docs/hosting-linux.md).

    1. Builds release x86_64-unknown-linux-gnu binaries inside WSL with deploy/linux/build-release.sh,
       which needs rustup there (a one-time setup, see the runbook) and installs the toolchain pinned
       in rust-toolchain.toml if it is missing. The binaries and a SHA256SUMS file land in
       -ArtifactDir. -SkipBuild deploys what is already there.
    2. Copies them, with deploy/linux/install-release.sh, to a new directory in the remote user's home.
    3. Runs install-release.sh there with sudo. It installs the binaries as a new release directory,
       stops the backups and the server behind a durable deploy marker, copies the databases, switches
       the current release atomically, starts the server and waits for /health on the VM itself, and
       rolls back if it doesn't answer. See that script for the details, and the runbook ("An
       interrupted deploy") for what to do if the connection drops or the VM loses power meanwhile.
    4. Removes the uploaded files, and with -PublicHealthUrl checks /health from outside too.

    The transport is pluggable:
    - gcloud (default): `gcloud compute ssh` and `gcloud compute scp` with --tunnel-through-iap, so the
      VM needs no public SSH port. Give -Project, -Zone and -Instance.
    - ssh: plain OpenSSH `ssh` and `scp` to -SshTarget (user@host, or a Host alias from ~/.ssh/config).

    Nothing about a deployment is built in: every name comes from the parameters.

    Exit codes: those of install-release.sh (0 deployed; 1 nothing changed; 2 rolled back, the previous
    release is current; 3 unfinished, the marker stays), or 1 if the build or the copy failed, or 4 if the deploy
    succeeded but -PublicHealthUrl didn't answer 200.

.PARAMETER Transport
    gcloud (IAP tunnel) or ssh.

.PARAMETER Project
    The Google Cloud project ID (gcloud transport).

.PARAMETER Zone
    The VM's zone (gcloud transport).

.PARAMETER Instance
    The VM's name (gcloud transport).

.PARAMETER GcloudPath
    gcloud.cmd. Defaults to the portable install in %LOCALAPPDATA%\google-cloud-sdk, then to PATH.

.PARAMETER SshTarget
    user@host or an ssh config alias (ssh transport).

.PARAMETER SshOption
    Extra -o options for ssh and scp, for example 'IdentityFile=C:\keys\vm' (ssh transport).

.PARAMETER WslDistro
    The WSL distribution to build in. Defaults to the default distribution.

.PARAMETER ArtifactDir
    Where the Linux binaries are written and read. Defaults to <repo>\target\linux-release.

.PARAMETER NoStart
    Install the binaries and leave the server stopped (for the move from the pilot).

.PARAMETER AllowEmptyDatabase
    Deploy although a release is installed and there is no database to copy first. Without it, such a
    deploy stops before it changes anything, because the database was expected to be there.

.PARAMETER PublicHealthUrl
    Also check this URL from the PC after the deploy, for example https://todo.example.com/health.

.EXAMPLE
    ./deploy/deploy-linux.ps1 -Project <project-id> -Zone <zone> -Instance <instance>

.EXAMPLE
    ./deploy/deploy-linux.ps1 -Transport ssh -SshTarget admin@vm.example.com -SkipBuild
#>
[CmdletBinding()]
param(
    [ValidateSet('gcloud', 'ssh')][string]$Transport = 'gcloud',
    [string]$Project = '',
    [string]$Zone = '',
    [string]$Instance = '',
    [string]$GcloudPath = '',
    [string]$SshTarget = '',
    [string[]]$SshOption = @(),
    [string]$WslDistro = '',
    [string]$RepoPath = '',
    [string]$ArtifactDir = '',
    [switch]$SkipBuild,
    [switch]$NoStart,
    [switch]$AllowEmptyDatabase,
    [int]$HealthTimeoutSeconds = 60,
    [int]$KeepSnapshots = 3,
    [string]$PublicHealthUrl = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $RepoPath) { $RepoPath = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path }
if (-not $ArtifactDir) { $ArtifactDir = Join-Path $RepoPath 'target\linux-release' }
$linuxDir = Join-Path $RepoPath 'deploy\linux'
$installer = Join-Path $linuxDir 'install-release.sh'
$files = @('todo-server', 'todo-backup', 'SHA256SUMS')

# ---- transport ----------------------------------------------------------------------------------

switch ($Transport) {
    'gcloud' {
        foreach ($p in @('Project', 'Zone', 'Instance')) {
            if (-not (Get-Variable -Name $p -ValueOnly)) { throw "-Transport gcloud needs -$p." }
        }
        if (-not $GcloudPath) {
            $portable = Join-Path $env:LOCALAPPDATA 'google-cloud-sdk\bin\gcloud.cmd'
            if (Test-Path $portable) {
                $GcloudPath = $portable
            } else {
                $cmd = Get-Command gcloud -ErrorAction SilentlyContinue
                if (-not $cmd) { throw "gcloud not found. Pass -GcloudPath." }
                $GcloudPath = $cmd.Source
            }
        }
        if (-not (Test-Path $GcloudPath)) { throw "gcloud not found at $GcloudPath." }
        $gcloudCommon = @('--project', $Project, '--zone', $Zone, '--tunnel-through-iap', '--quiet')
    }
    'ssh' {
        if (-not $SshTarget) { throw '-Transport ssh needs -SshTarget (user@host or an ssh config alias).' }
        $sshOpts = @('-o', 'BatchMode=yes')
        foreach ($o in $SshOption) { $sshOpts += @('-o', $o) }
    }
}

# Runs one command line in the remote user's login shell, in their home directory. Returns the exit code.
# The output goes to the console, not into the return value.
function Invoke-Remote([string]$Command) {
    if ($Transport -eq 'gcloud') {
        & $GcloudPath compute ssh $Instance @gcloudCommon --command $Command | Out-Host
    } else {
        & ssh @sshOpts $SshTarget $Command | Out-Host
    }
    return $LASTEXITCODE
}

# Copies files from the current directory into an existing directory, relative to the remote user's
# home. Names stay relative: both gcloud and scp would read `C:\...` as a host named C.
function Copy-ToRemote([string[]]$Names, [string]$RemoteDir) {
    if ($Transport -eq 'gcloud') {
        & $GcloudPath compute scp @gcloudCommon @Names "${Instance}:$RemoteDir/" | Out-Host
    } else {
        & scp @sshOpts @Names "${SshTarget}:$RemoteDir/" | Out-Host
    }
    if ($LASTEXITCODE -ne 0) { throw "copying to the VM failed (exit code $LASTEXITCODE)." }
}

# ---- 1. build -----------------------------------------------------------------------------------

if (-not $SkipBuild) {
    $wsl = Join-Path $env:WINDIR 'System32\wsl.exe'
    if (-not (Test-Path $wsl)) { throw 'WSL is not installed. Install it (wsl --install), or build elsewhere and pass -SkipBuild.' }
    $distro = @()
    if ($WslDistro) { $distro = @('-d', $WslDistro) }
    New-Item -ItemType Directory -Force -Path $ArtifactDir | Out-Null
    # wslpath turns the Windows paths into /mnt/... paths inside the distribution.
    $wslRepo = (& $wsl @distro -- wslpath -a -u ($RepoPath -replace '\\', '/')).Trim()
    $wslOut = (& $wsl @distro -- wslpath -a -u ($ArtifactDir -replace '\\', '/')).Trim()
    if (-not $wslRepo -or -not $wslOut) { throw 'wslpath failed; is the WSL distribution running?' }
    Write-Host "Building in WSL: $wslRepo -> $wslOut" -ForegroundColor Cyan
    & $wsl @distro -- bash -l "$wslRepo/deploy/linux/build-release.sh" $wslOut
    if ($LASTEXITCODE -ne 0) {
        Write-Host 'The build failed; nothing was deployed.' -ForegroundColor Red
        exit 1
    }
}
foreach ($f in $files) {
    if (-not (Test-Path (Join-Path $ArtifactDir $f))) { throw "$f is missing from $ArtifactDir. Build first (drop -SkipBuild)." }
}

# ---- 2. copy ------------------------------------------------------------------------------------

$remoteDir = 'todo-deploy-{0:yyyyMMddTHHmmss}' -f (Get-Date).ToUniversalTime()
Write-Host "Copying the release to ~/$remoteDir on the VM" -ForegroundColor Cyan
# The installer travels with the binaries, so the VM always runs the one from this checkout.
Copy-Item -Path $installer -Destination (Join-Path $ArtifactDir 'install-release.sh') -Force
Push-Location $ArtifactDir
try {
    if ((Invoke-Remote "mkdir -m 700 $remoteDir") -ne 0) { throw 'could not create the remote directory.' }
    Copy-ToRemote ($files + @('install-release.sh')) $remoteDir
} catch {
    Write-Host "Copying failed: $_ Nothing was deployed." -ForegroundColor Red
    Invoke-Remote "rm -rf $remoteDir" | Out-Null
    exit 1
} finally {
    Pop-Location
}

# ---- 3. install ---------------------------------------------------------------------------------

$installArgs = "--health-timeout $HealthTimeoutSeconds --keep $KeepSnapshots"
if ($NoStart) { $installArgs += ' --no-start' }
if ($AllowEmptyDatabase) { $installArgs += ' --allow-empty-database' }
# The upload is removed whatever happens; the script's exit code is passed back.
$remote = "sudo bash $remoteDir/install-release.sh $installArgs $remoteDir; rc=`$?; rm -rf $remoteDir; exit `$rc"
Write-Host 'Installing on the VM' -ForegroundColor Cyan
$code = Invoke-Remote $remote

switch ($code) {
    0 { if ($NoStart) { Write-Host 'Installed; the server stays stopped.' -ForegroundColor Green } else { Write-Host 'Deployed.' -ForegroundColor Green } }
    1 { Write-Host 'Nothing changed on the VM.' -ForegroundColor Yellow; exit 1 }
    2 { Write-Host 'The new release failed its health check and was rolled back; the previous release runs.' -ForegroundColor Yellow; exit 2 }
    3 { Write-Host 'The deploy is unfinished and its marker stays (on the VM: sudo /usr/local/lib/todo/install-release --status). See the runbook, "An interrupted deploy".' -ForegroundColor Red; exit 3 }
    default { Write-Host "The install ended with exit code $code (the connection may have dropped). On the VM, run: sudo /usr/local/lib/todo/install-release --status" -ForegroundColor Red; exit 1 }
}

# ---- 4. check from outside ----------------------------------------------------------------------

if ($PublicHealthUrl -and -not $NoStart) {
    try {
        $r = Invoke-WebRequest -Uri $PublicHealthUrl -UseBasicParsing -TimeoutSec 15 -MaximumRedirection 0
        if ($r.StatusCode -ne 200) { throw "status $($r.StatusCode)" }
        Write-Host "$PublicHealthUrl answers 200." -ForegroundColor Green
    } catch {
        Write-Host "$PublicHealthUrl did not answer 200: $_" -ForegroundColor Red
        exit 4
    }
}
exit 0
