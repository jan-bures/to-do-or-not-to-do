<#
.SYNOPSIS
Renders the screenshot gallery: every shell fixture in light and dark, at 100 % and 150 %.

.DESCRIPTION
Builds the desktop app in release mode (unless -NoBuild), makes sure the bundled fonts are available, then runs
`todo-desktop --screenshot` once per fixture, theme and scale. The PNGs land in apps/desktop/target/gallery
(git ignores target/). Compare them against the chosen mockups before calling a UI change done.

.EXAMPLE
pwsh apps/desktop/scripts/gallery.ps1
pwsh apps/desktop/scripts/gallery.ps1 -NoBuild -OutDir C:\temp\gallery
#>
param(
    [string]$OutDir,
    [switch]$NoBuild,
    [string[]]$Fixtures = @('my-day', 'work', 'empty', 'quick-add'),
    [string[]]$Themes = @('light', 'dark'),
    [string[]]$Scales = @('1', '1.5')
)

$ErrorActionPreference = 'Stop'
$app = Resolve-Path (Join-Path $PSScriptRoot '..')
if (-not $OutDir) { $OutDir = Join-Path $app 'target/gallery' }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

if (-not $NoBuild) {
    Push-Location $app
    try {
        cargo build --release --locked
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
    } finally { Pop-Location }
}
$exe = Join-Path $app 'target/release/todo-desktop.exe'
if (-not (Test-Path $exe)) { throw "missing $exe; build first" }

# Fonts: the shared assets/fonts directory once it exists, otherwise the stopgap download in target/fonts.
$repo = Resolve-Path (Join-Path $app '../..')
$shared = Join-Path $repo 'assets/fonts'
$stopgap = Join-Path $app 'target/fonts'
$haveShared = (Test-Path $shared) -and (Get-ChildItem $shared -Include *.ttf, *.otf -Recurse -ErrorAction SilentlyContinue)
$haveStopgap = (Test-Path $stopgap) -and (Get-ChildItem $stopgap -Filter *.ttf -ErrorAction SilentlyContinue)
if (-not $haveShared -and -not $haveStopgap) {
    Write-Host 'Fetching fonts into target/fonts...'
    python (Join-Path $PSScriptRoot 'fetch_fonts.py') $stopgap
    if ($LASTEXITCODE -ne 0) { throw 'fetching fonts failed' }
}

$failed = 0
foreach ($fixture in $Fixtures) {
    foreach ($theme in $Themes) {
        foreach ($scale in $Scales) {
            $pct = [int]([double]::Parse($scale, [Globalization.CultureInfo]::InvariantCulture) * 100)
            $out = Join-Path $OutDir "$fixture-$theme-$pct.png"
            $argList = @('--screenshot', "`"$out`"", '--fixture', $fixture, '--theme', $theme, '--scale', $scale)
            $p = Start-Process -FilePath $exe -ArgumentList $argList -Wait -PassThru -NoNewWindow
            if ($p.ExitCode -ne 0) {
                Write-Warning "$fixture $theme ${pct}%: exit code $($p.ExitCode)"
                $failed++
            }
        }
    }
}
Write-Host "Gallery in $OutDir"
if ($failed -gt 0) { throw "$failed screenshot(s) failed" }
