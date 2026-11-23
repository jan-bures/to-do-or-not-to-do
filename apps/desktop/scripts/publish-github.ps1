#Requires -Version 7.0
<#
.SYNOPSIS
    Builds and scans an unsigned Windows release. Use -Publish to upload it to GitHub.
.DESCRIPTION
    Use a new output directory. Reads GitHub Releases to find the previous desktop delta base;
    dry runs never create a release, tag, commit, or asset. Requires gh, Python 3 and OpenSSL 3.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$OutputDir,
    [switch]$Publish,
    [string]$Notes = 'Unsigned Windows x64 installer and update packages.'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'package-version.ps1')
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
. (Join-Path $repo 'scripts/github-release.ps1')

$denylist = $env:TODO_DESKTOP_DENYLIST
if ($Publish -and -not $denylist) { throw 'Publishing requires a private denylist (TODO_DESKTOP_DENYLIST).' }
if ($denylist -and (-not (Test-Path -LiteralPath $denylist -PathType Leaf) -or
    -not (Get-Content -Encoding utf8 -LiteralPath $denylist | Where-Object { $_ -match '^\s*[^\s#]' } | Select-Object -First 1))) {
    throw 'TODO_DESKTOP_DENYLIST must point to a readable file with at least one active pattern.'
}
if (-not $denylist) { Write-Warning 'No private denylist configured; this dry run only checks built-in privacy patterns.' }
$privateKey = $env:TODO_DESKTOP_FEED_KEY
$signer = Join-Path $PSScriptRoot 'sign-update-feed.py'
if ($Publish) {
    if (-not $privateKey) { throw 'Publishing requires an offline Ed25519 key (TODO_DESKTOP_FEED_KEY).' }
    & python -B $signer check-key
    if ($LASTEXITCODE -ne 0) { throw 'The resolved signing key must be outside the worktree.' }
}

$repository = Get-PublicReleaseRepository $repo
$head = git -C $repo rev-parse HEAD
if ($LASTEXITCODE -ne 0) { throw 'Cannot read the build commit.' }
if ($Publish) { Assert-PublicReleaseHead $repository $head }
$height = git -C $repo rev-list --first-parent --count $head
if ($LASTEXITCODE -ne 0) { throw 'Cannot read the git commit count.' }
$rootMetadata = cargo metadata --manifest-path (Join-Path $repo 'Cargo.toml') --locked --no-deps --format-version 1
if ($LASTEXITCODE -ne 0) { throw 'Cannot read the root workspace version.' }
$ffi = ($rootMetadata | ConvertFrom-Json).packages | Where-Object { $_.name -ceq 'todo-ffi' }
$version = Get-DesktopPackageVersion $ffi.version $height # Android uses the root workspace version.
$manifestName = 'releases.win.json'
$proofName = 'desktop-update.sig.json'
$keyOptions = @()
if (-not $Publish -and $env:TODO_DESKTOP_UPDATE_KEYS) { $keyOptions = @('--keys', $env:TODO_DESKTOP_UPDATE_KEYS) }
$setupName = 'todo-desktop-win-Setup.exe'

# An Android-only Latest is not a delta base. Find the greatest published desktop version,
# irrespective of which platform published most recently. `gh` is used only by publishers.
$tags = & gh api --hostname github.com --paginate "repos/$($repository.Slug)/releases?per_page=100" `
    --jq '.[] | select((.draft or .prerelease) | not) | select(any(.assets[]; .name == "releases.win.json")) | .tag_name'
if ($LASTEXITCODE -ne 0) { throw 'Cannot list GitHub releases for the desktop delta base.' }
$previous = $null
foreach ($tag in $tags) {
    if ($tag -notmatch '^v((?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)-build\.[1-9][0-9]*)$') { continue }
    $candidate = [semver]$Matches[1]
    if ($candidate -ge $version) { throw 'The build must be newer than the last published desktop release.' }
    if ($null -eq $previous -or $candidate -gt $previous) { $previous = $candidate }
}

$OutputDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDir)
if (Test-Path -LiteralPath $OutputDir) { throw 'Use a new output directory for each release.' }
$null = New-Item -ItemType Directory -Path $OutputDir
$manifest = Join-Path $OutputDir $manifestName
$proof = Join-Path $OutputDir $proofName
$packages = @((Join-Path $OutputDir $setupName), (Join-Path $OutputDir "todo-desktop-$version-full.nupkg"))
if ($null -ne $previous) {
    $baseName = "todo-desktop-$previous-full.nupkg"
    $baseFile = Join-Path $OutputDir $baseName
    & gh release download "v$previous" --repo $repository.Target --dir $OutputDir `
        --pattern $manifestName --pattern $proofName --pattern $baseName
    if ($LASTEXITCODE -ne 0) { throw 'Cannot download the previous signed desktop feed and full package.' }
    & python -B $signer verify "v$previous" $manifest $proof @keyOptions
    if ($LASTEXITCODE -ne 0) { throw 'The previous desktop feed is not signed by a trusted key.' }
    Remove-Item -LiteralPath $proof
    $feed = Get-Content -Raw -LiteralPath $manifest | ConvertFrom-Json
    $asset = @($feed.Assets | Where-Object { $_.Version -ceq "$previous" -and $_.FileName -ceq $baseName -and
        $_.Type -ceq 'Full' -and $_.PackageId -ceq 'todo-desktop' })
    if ($asset.Count -ne 1 -or $asset[0].Size -ne (Get-Item -LiteralPath $baseFile).Length -or
        $asset[0].SHA256 -cnotmatch '^[0-9a-fA-F]{64}$' -or $asset[0].SHA256 -ne (Get-FileHash -LiteralPath $baseFile).Hash) {
        throw 'The previous full package does not match its manifest.'
    }
    # Seed vpk with only the immediate desktop base. The retained old feed entry still resolves to
    # its original tag, not this release. Older installs can download the new full directly.
    @{ Assets = $asset } | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8NoBOM -LiteralPath $manifest
    $packages += Join-Path $OutputDir "todo-desktop-$version-delta.nupkg"
}

$feedOverride = $env:TODO_DESKTOP_UPDATE_FEED
$keysOverride = $env:TODO_DESKTOP_UPDATE_KEYS
try {
    $env:TODO_DESKTOP_FEED_KEY = $null # The Rust build and packaging tools do not need the key path.
    if ($Publish) {
        $env:TODO_DESKTOP_UPDATE_FEED = $repository.Url
        $env:TODO_DESKTOP_UPDATE_KEYS = $null # Official packages always pin the checked-in public keys.
    }
    & (Join-Path $PSScriptRoot 'package.ps1') -ReleaseDir $OutputDir -AllowUnmerged:(-not $Publish)
} finally {
    $env:TODO_DESKTOP_UPDATE_FEED = $feedOverride
    $env:TODO_DESKTOP_UPDATE_KEYS = $keysOverride
    $env:TODO_DESKTOP_FEED_KEY = $privateKey
}
foreach ($package in $packages) {
    if (-not (Test-Path -LiteralPath $package -PathType Leaf)) {
        throw 'Packaged version does not match the shared Android/desktop release version.'
    }
}
if ($null -ne $previous) { Remove-Item -LiteralPath $baseFile }
if ($privateKey) {
    $testKey = @()
    if (-not $Publish -and $keyOptions.Count) { $testKey = @('--allow-test-key-in-checkout') }
    & python -B $signer sign "v$version" $manifest $proof --setup $packages[0] @keyOptions @testKey
    if ($LASTEXITCODE -ne 0) { throw 'Cannot sign the desktop update feed.' }
    $packages += $proof # Upload the proof before the well-known manifest becomes visible.
}
# Never publish vpk convenience/legacy outputs; only the explicit asset list is uploaded.
$unused = @('todo-desktop-win-Portable.zip', 'RELEASES', 'assets.win.json')
foreach ($name in $unused) {
    $path = Join-Path $OutputDir $name
    if (Test-Path -LiteralPath $path -PathType Leaf) { Remove-Item -LiteralPath $path }
}
$scannerArgs = @($OutputDir, '--notes-stdin')
if ($Publish) { $scannerArgs += '--require-denylist' }
$Notes | & python -B (Join-Path $PSScriptRoot 'scan-release-desktop.py') @scannerArgs
if ($LASTEXITCODE -ne 0) { throw 'Desktop release privacy scan failed.' }

if ($Publish) {
    Publish-GitHubAssets $repository $head "v$version" "$version" $Notes $packages $manifest
} else {
    Write-Output "Dry run: v$version. Nothing published."
}
