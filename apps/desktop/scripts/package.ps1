#Requires -Version 7.0
<#
.SYNOPSIS
    Builds and packages the Windows x64 desktop client. Does not install or publish it.
.DESCRIPTION
    Requires the pinned Rust toolchain, Windows MSVC x64 build tools and the .NET 8 SDK/runtime.
    The Velopack vpk tool is installed without admin rights in the workspace target directory.
    Keep the previous output in ReleaseDir to build a delta against its latest full package.
.PARAMETER AllowUnmerged
    Skip the origin/main ancestry check for test builds. Do not publish those packages.
#>
[CmdletBinding()]
param([string]$ReleaseDir = '', [switch]$AllowUnmerged)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'package-version.ps1')

$app = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$repo = (Resolve-Path (Join-Path $app '../..')).Path
if (-not $ReleaseDir) { $ReleaseDir = Join-Path $app 'target/desktop-release' }
$ReleaseDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($ReleaseDir)
$target = 'x86_64-pc-windows-msvc'
$vpkVersion = '1.2.161' # Same as the Rust updater.
$toolDir = Join-Path $app "target/desktop-tools/vpk-$vpkVersion"
$vpk = Join-Path $toolDir 'vpk.exe'
$channel = 'win'
$appId = 'todo-desktop'
$index = Join-Path $ReleaseDir "releases.$channel.json"

Push-Location $app
try {
    $shallow = git rev-parse --is-shallow-repository
    if ($LASTEXITCODE -ne 0 -or $shallow -ne 'false') { throw 'Packaging needs a full Git history.' }
    $dirty = git status --porcelain --untracked-files=no
    if ($LASTEXITCODE -ne 0 -or $dirty) { throw 'Commit changes to tracked files before packaging.' }
    if (-not $AllowUnmerged) {
        git merge-base --is-ancestor HEAD origin/main
        if ($LASTEXITCODE -ne 0) {
            throw 'Packaging a release requires a commit on origin/main. Run git fetch origin, or use -AllowUnmerged for a test build only.'
        }
    }
    $height = git rev-list --first-parent --count HEAD
    if ($LASTEXITCODE -ne 0) { throw "Could not determine Git commit count ($LASTEXITCODE)." }
    $metadataJson = cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed ($LASTEXITCODE)." }
    $metadata = $metadataJson | ConvertFrom-Json
    $crate = $metadata.packages | Where-Object { $_.name -ceq $appId }
    $version = Get-DesktopPackageVersion $crate.version $height

    $hasPrevious = Test-Path -LiteralPath $index -PathType Leaf
    if ($hasPrevious) {
        $previous = Get-Content -LiteralPath $index -Raw | ConvertFrom-Json
        foreach ($asset in $previous.Assets) {
            if ($version -le [semver]$asset.Version) {
                throw "Version $version must be newer than the existing release $($asset.Version). Do not overwrite a published version."
            }
        }
    } elseif (Test-Path -LiteralPath $ReleaseDir) {
        if (Get-ChildItem -LiteralPath $ReleaseDir -Filter '*.nupkg') {
            throw 'Release directory already has packages but no feed. Restore the feed or choose an empty directory.'
        }
    }

    cargo build --release --locked --target $target --bin $appId
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)." }
    if (-not (Test-Path -LiteralPath $vpk -PathType Leaf)) {
        dotnet tool install vpk --tool-path $toolDir --version $vpkVersion
        if ($LASTEXITCODE -ne 0) { throw "Installing vpk failed ($LASTEXITCODE). Install the .NET 8 SDK." }
    }

    $staging = Join-Path $app "target/desktop-stage-$([guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Path $staging | Out-Null
    try {
        # vpk edits its input; never point it at Cargo's build directory.
        Copy-Item -LiteralPath (Join-Path $metadata.target_directory "$target/release/$appId.exe") -Destination $staging
        & $vpk pack --packId $appId --packVersion $version.ToString() --packTitle 'To-do or not to do' `
            --packDir $staging --outputDir $ReleaseDir --mainExe "$appId.exe" --channel $channel `
            --runtime win-x64 --delta BestSpeed `
            --icon (Join-Path $repo 'assets/brand/generated/todo.ico')
        if ($LASTEXITCODE -ne 0) { throw "vpk pack failed ($LASTEXITCODE)." }
    } finally {
        Remove-Item -LiteralPath $staging -Recurse -Force
    }

    $feed = Get-Content -LiteralPath $index -Raw | ConvertFrom-Json
    $kinds = @('full')
    if ($hasPrevious) { $kinds += 'delta' }
    foreach ($kind in $kinds) {
        $name = "$appId-$version-$kind.nupkg"
        $asset = $feed.Assets | Where-Object { $_.FileName -ceq $name -and $_.Version -ceq $version.ToString() }
        if (-not $asset -or $asset.SHA256 -ne (Get-FileHash -LiteralPath (Join-Path $ReleaseDir $name)).Hash) {
            throw "Feed did not match package $name. Do not publish this output."
        }
    }
    $setup = Join-Path $ReleaseDir "$appId-$channel-Setup.exe"
    if (-not (Test-Path -LiteralPath $setup -PathType Leaf)) { throw 'vpk did not produce Setup.exe.' }
    Write-Host "Packaged $version (unsigned, not published): $ReleaseDir"
} finally {
    Pop-Location
}
