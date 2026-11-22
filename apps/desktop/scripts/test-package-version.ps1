#Requires -Version 7.0
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'package-version.ps1')

$earlier = Get-DesktopPackageVersion '0.1.0' '9'
$later = Get-DesktopPackageVersion '0.1.0' '10'
$bump = Get-DesktopPackageVersion '0.1.1' '11'
if ($later.ToString() -cne '0.1.0-build.10' -or -not ($earlier -lt $later -and $later -lt $bump)) {
    throw 'Package versions must preserve their crate version and increase numerically.'
}
foreach ($argsPair in @(@('0.1.0-dev', '10'), @('0.1.0+meta', '10'), @('0.1.0', '01'), @('0.1.0', '0'), @('0.1.0', '4294967296'))) {
    $failed = $false
    try { Get-DesktopPackageVersion @argsPair | Out-Null } catch { $failed = $true }
    if (-not $failed) { throw "Expected invalid version inputs to fail: $argsPair" }
}
Write-Host 'Package version checks passed.'
