<#
.SYNOPSIS
Renders the screenshot gallery: every shell fixture in light and dark, at 100 % and 150 %.

.DESCRIPTION
Builds the desktop app in release mode (unless -NoBuild), then runs `todo-desktop --screenshot-all` once
and checks that every image has exactly the expected pixel size. The PNGs land in
apps/desktop/target/gallery (git ignores target/). Compare them against the chosen mockups before calling a UI
change done.

.EXAMPLE
pwsh apps/desktop/scripts/gallery.ps1
pwsh apps/desktop/scripts/gallery.ps1 -NoBuild -OutDir C:\temp\gallery
#>
param(
    [string]$OutDir,
    [switch]$NoBuild,
    [string[]]$Fixtures = @('my-day', 'work', 'collapsed-collection', 'long-sidebar', 'long-names', 'long-names-narrow', 'narrow-detail', 'long-task-list', 'caption-hover', 'caption-close-hover', 'empty', 'finished', 'finished-short', 'finished-nothing-tomorrow', 'quick-add', 'pairing', 'pairing-code', 'open-failed', 'open-failed-confirm', 'completing-fill', 'completing-hold', 'completing-collapse-50', 'sun-travel-50', 'reopening-collapse-50', 'dusk-50', 'dusk-75'),
    [string[]]$Themes = @('light', 'dark'),
    [string[]]$Scales = @('1', '1.5')
)

$ErrorActionPreference = 'Stop'
$app = Resolve-Path (Join-Path $PSScriptRoot '..')
if (-not $OutDir) { $OutDir = Join-Path $app 'target/gallery' }
$OutDir = (New-Item -ItemType Directory -Force -Path $OutDir).FullName

if (-not $NoBuild) {
    Push-Location $app
    try {
        cargo build --release --locked
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
    } finally { Pop-Location }
}
$exe = Join-Path $app 'target/release/todo-desktop.exe'
if (-not (Test-Path $exe)) { throw "missing $exe; build first" }

# The expected logical size of each fixture's window; the captured PNG must be exactly this times the scale.
function Get-ExpectedSize([string]$fixture, [double]$scale) {
    $logical = switch ($fixture) {
        'quick-add' { @(640, 196) }
        { $_ -in 'long-names-narrow', 'narrow-detail' } { @(960, 900) }
        'finished-short' { @(1440, 700) }
        default { @(1440, 900) }
    }
    return @([int][math]::Round($logical[0] * $scale), [int][math]::Round($logical[1] * $scale))
}

# Width and height from the PNG's IHDR chunk (big-endian, at bytes 16 and 20), or $null when the file isn't a
# complete PNG: the signature at the start and the IEND chunk (with its fixed CRC) at the very end.
function Get-PngSize([string]$path) {
    $b = [System.IO.File]::ReadAllBytes($path)
    $signature = [byte[]](0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A)
    $iend = [byte[]](0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82)
    if ($b.Length -lt 45) { return $null }
    if (Compare-Object $b[0..7] $signature -SyncWindow 0) { return $null }
    if (Compare-Object $b[($b.Length - 12)..($b.Length - 1)] $iend -SyncWindow 0) { return $null }
    $be = { param($o) ([int]$b[$o] -shl 24) -bor ([int]$b[$o + 1] -shl 16) -bor ([int]$b[$o + 2] -shl 8) -bor [int]$b[$o + 3] }
    return @((& $be 16), (& $be 20))
}

# Sample ownership, not just the HWND: the user is free to switch between other apps during a run.
if (-not ('GalleryForeground' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class GalleryForeground {
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
    public static uint Owner() {
        uint pid;
        GetWindowThreadProcessId(GetForegroundWindow(), out pid);
        return pid;
    }
}
'@
}
$start = [Diagnostics.ProcessStartInfo]::new($exe)
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
foreach ($arg in @('--screenshot-all', $OutDir, '--overwrite', '--fixture', ($Fixtures -join ','),
                   '--theme', ($Themes -join ','), '--scale', ($Scales -join ','))) {
    $start.ArgumentList.Add($arg)
}
$clock = [Diagnostics.Stopwatch]::StartNew()
$before = [GalleryForeground]::Owner()
$changed = $false
$tookFocus = $false
$p = [Diagnostics.Process]::Start($start)
try {
    do {
        $owner = [GalleryForeground]::Owner()
        if ($owner -ne $before) { $changed = $true }
        if ($owner -eq $p.Id) { $tookFocus = $true }
    } until ($p.WaitForExit(10))
    if ([GalleryForeground]::Owner() -ne $before) { $changed = $true }
    if ($tookFocus) { throw 'The screenshot process took the foreground' }
    if ($p.ExitCode -ne 0) { throw "Screenshot process failed ($($p.ExitCode))" }
} finally { $p.Dispose() }

$failed = 0
foreach ($fixture in $Fixtures) {
    foreach ($theme in $Themes) {
        foreach ($scale in $Scales) {
            $pct = [int]([double]::Parse($scale, [Globalization.CultureInfo]::InvariantCulture) * 100)
            $out = Join-Path $OutDir "$fixture-$theme-$pct.png"
            $want = Get-ExpectedSize $fixture ([double]::Parse($scale, [Globalization.CultureInfo]::InvariantCulture))
            $got = Get-PngSize $out
            if ($null -eq $got) {
                Write-Warning "$fixture $theme ${pct}%: $out is not a complete PNG"
                $failed++
                continue
            }
            if ($got[0] -ne $want[0] -or $got[1] -ne $want[1]) {
                Write-Warning "$fixture $theme ${pct}%: image is $($got[0])x$($got[1]), expected $($want[0])x$($want[1])"
                $failed++
            }
        }
    }
}
$focus = if ($changed) { 'other apps switched foreground' } else { 'foreground owner unchanged' }
Write-Host ('Gallery in {0} ({1:N2} s including validation; {2})' -f $OutDir, $clock.Elapsed.TotalSeconds, $focus)
if ($failed -gt 0) { throw "$failed screenshot(s) failed" }
