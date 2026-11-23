# Build and sign an Android release. Publishing public GitHub assets is strictly opt-in.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $BuildTools,
    [Parameter(Mandatory)][string] $OutputDir,
    [string] $UnsignedApk,
    [switch] $Publish,
    [string] $Notes = 'Signed Android APK for arm64 devices.'
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
. (Join-Path $repo 'scripts/github-release.ps1')
$denylist = $env:TODO_ANDROID_DENYLIST
if ($denylist) {
    if (-not (Test-Path -LiteralPath $denylist -PathType Leaf) -or
        -not (Get-Content -Encoding utf8 -LiteralPath $denylist | Where-Object { $_ -match '^\s*[^\s#]' } | Select-Object -First 1)) {
        throw 'TODO_ANDROID_DENYLIST must point to a readable file with at least one active pattern.'
    }
} elseif ($Publish) {
    throw 'Publishing requires a private denylist (TODO_ANDROID_DENYLIST).'
} else {
    Write-Warning 'No private denylist configured; this dry run only checks built-in privacy patterns.'
}
$expectedOld = $env:TODO_ANDROID_OLD_CERT_SHA256
$lineage = $env:TODO_ANDROID_LINEAGE
if ($Publish -and (-not $lineage -or -not $expectedOld)) {
    throw 'Publishing requires TODO_ANDROID_LINEAGE and TODO_ANDROID_OLD_CERT_SHA256.'
}
if ($expectedOld -and ($expectedOld -notmatch '^[0-9a-fA-F]{64}$' -or -not $lineage)) {
    throw 'TODO_ANDROID_OLD_CERT_SHA256 must be a 64-digit certificate SHA-256 with a lineage configured.'
}
if ($Publish) {
    if ($UnsignedApk) { throw 'Publishing must build the APK from main; do not pass -UnsignedApk.' }
    $repository = Get-PublicReleaseRepository $repo
    $head = git -C $repo rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read the build commit.' }
    Assert-PublicReleaseHead $repository $head
}
foreach ($name in @('TODO_ANDROID_KEYSTORE', 'TODO_ANDROID_KEY_ALIAS', 'TODO_ANDROID_STORE_PASSWORD', 'TODO_ANDROID_KEY_PASSWORD')) {
    if (-not [Environment]::GetEnvironmentVariable($name)) { throw "Set $name outside the repository." }
}
$signer = Join-Path $BuildTools $(if ($IsWindows) { 'apksigner.bat' } else { 'apksigner' })
$aapt = Join-Path $BuildTools $(if ($IsWindows) { 'aapt2.exe' } else { 'aapt2' })
if (-not (Test-Path -LiteralPath $signer -PathType Leaf) -or -not (Test-Path -LiteralPath $aapt -PathType Leaf)) {
    throw 'Pass the Android SDK build-tools directory (37.0.0 or later).'
}
function Read-Lineage([string] $Path) {
    $lines = & $signer lineage --in $Path --print-certs
    if ($LASTEXITCODE -ne 0) { throw 'Could not read signing lineage.' }
    $digests = @($lines | ForEach-Object {
        if ($_ -match '^Signer #([0-9]+) in lineage certificate SHA-256 digest: ([0-9a-fA-F]{64})$') {
            [pscustomobject]@{ Number = [int]$Matches[1]; Digest = $Matches[2].ToLowerInvariant() }
        }
    } | Sort-Object Number)
    if ($digests.Count -lt 2) { throw 'Expected a signing lineage with at least two certificates.' }
    for ($i = 0; $i -lt $digests.Count; $i++) {
        if ($digests[$i].Number -ne $i + 1) { throw 'Unexpected signing lineage order.' }
        $digests[$i].Digest
    }
}
$expectedLineage = @()
if ($lineage) {
    $expectedLineage = @(Read-Lineage $lineage)
    if ($expectedOld -and $expectedLineage[0] -ne $expectedOld) { throw 'The lineage does not start with the expected old certificate.' }
}

if (-not $UnsignedApk) {
    $signingVars = @{}
    $argsForGradle = @(':app:assembleRelease')
    if ($Publish) { $argsForGradle += "-Ptodo.updateFeed=$($repository.Url)" }
    Push-Location (Join-Path $PSScriptRoot '..')
    try {
        foreach ($name in @('TODO_ANDROID_KEYSTORE', 'TODO_ANDROID_KEY_ALIAS', 'TODO_ANDROID_STORE_PASSWORD', 'TODO_ANDROID_KEY_PASSWORD', 'TODO_ANDROID_LINEAGE', 'TODO_ANDROID_OLD_CERT_SHA256', 'TODO_ANDROID_DENYLIST')) {
            $signingVars[$name] = [Environment]::GetEnvironmentVariable($name)
            [Environment]::SetEnvironmentVariable($name, $null)
        }
        if ((git rev-parse --is-shallow-repository) -ne 'false') { throw 'A full Git history is required for release numbering.' }
        if ($IsWindows) { & .\gradlew.bat @argsForGradle } else { & ./gradlew @argsForGradle }
        if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
        $UnsignedApk = (Resolve-Path 'app/build/outputs/apk/release/app-release-unsigned.apk').Path
    } finally {
        foreach ($name in $signingVars.Keys) { [Environment]::SetEnvironmentVariable($name, $signingVars[$name]) }
        Pop-Location
    }
}
$badging = & $aapt dump badging $UnsignedApk
if ($LASTEXITCODE -ne 0) { throw 'Could not read the APK metadata.' }
$package = $badging | Where-Object { $_ -like 'package:*' } | Select-Object -First 1
if ($package -notmatch "^package: name='app\.todoornot\.android' versionCode='([1-9][0-9]*)' versionName='((?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)-build\.\1)'") {
    throw 'Unexpected release package name or version metadata.'
}
$code = [int]$Matches[1]
$version = $Matches[2]
if (-not ($badging -match "^(?:minSdkVersion|sdkVersion):'29'$") -or ($badging -notcontains "targetSdkVersion:'37'")) { throw 'Unexpected SDK versions.' }
if ($Publish) {
    $height = git -C $repo rev-list --first-parent --count $head
    if ($LASTEXITCODE -ne 0 -or $code -ne [int]$height) { throw 'The APK version does not match the publishing commit.' }
}
if (Test-Path -LiteralPath $OutputDir) { throw 'Use a new output directory for each release.' }
$null = New-Item -ItemType Directory -Path $OutputDir
$file = "todo-android-$version.apk"
$apk = Join-Path $OutputDir $file
$rotation = @()
if ($lineage) {
    # Apply rotation on every supported OS (API 29+) rather than the default of API 33+.
    # v3-only signing needs only the final key, even on the first rotated release.
    $rotation = @('--lineage', $lineage, '--rotation-min-sdk-version', '28')
}
try {
    & $signer sign --out $apk --ks $env:TODO_ANDROID_KEYSTORE --ks-key-alias $env:TODO_ANDROID_KEY_ALIAS `
        --ks-pass env:TODO_ANDROID_STORE_PASSWORD --key-pass env:TODO_ANDROID_KEY_PASSWORD `
        --v1-signing-enabled false --v2-signing-enabled false --v3-signing-enabled true --v4-signing-enabled false `
        @rotation $UnsignedApk
    if ($LASTEXITCODE -ne 0) { throw 'APK signing failed.' }
    $verification = & $signer verify --print-certs -v $apk
    if ($LASTEXITCODE -ne 0 -or $verification -notcontains 'Verified using v3 scheme (APK Signature Scheme v3): true') {
        throw 'APK verification failed.'
    }
    # Reading each ZIP entry catches paths in Rust read-only data and DEX strings, not just debug symbols.
    $python = if ($IsWindows) { 'python' } else { 'python3' }
    # The release notes become public too, so scan them with the certificate output.
    (@($verification) + @($Notes)) | & $python -B (Join-Path $PSScriptRoot 'scan-release-apk.py') $apk --certificate-stdin
    if ($LASTEXITCODE -ne 0) { throw 'APK or release notes contain local or denied data, or could not be scanned.' }
    $verification | Write-Output
    $certs = @($verification | ForEach-Object {
        if ($_ -match '^(?:Signer #[0-9]+|V3\.0 Signer:) certificate SHA-256 digest: ([0-9a-fA-F]{64})$') { $Matches[1].ToLowerInvariant() }
    } | Select-Object -Unique)
    if ($certs.Count -ne 1) { throw 'Expected one current APK v3 signer.' }
    if ($lineage) {
        if ($expectedLineage[-1] -ne $certs[0]) { throw 'The lineage must end with the manifest signer.' }
        $embedded = @(Read-Lineage $apk)
        if (($embedded -join ',') -ne ($expectedLineage -join ',')) {
            throw 'The APK does not carry the expected v3 signing lineage, including the old certificate.'
        }
        if ($expectedOld -and $embedded -notcontains $expectedOld) { throw 'The APK lineage is missing the expected old certificate.' }
    }
    $size = (Get-Item -LiteralPath $apk).Length
    if ($size -gt 64MB) { throw 'The APK exceeds the updater limit of 64 MiB.' }
    $manifest = [ordered]@{
        versionCode = $code
        versionName = $version
        size = $size
        sha256 = (Get-FileHash -LiteralPath $apk).Hash.ToLowerInvariant()
        fileName = $file
        minSdk = 29
        signingCertSha256 = $certs
    }
    $index = Join-Path $OutputDir 'android-update.json'
    $manifest | ConvertTo-Json | Set-Content -Encoding utf8NoBOM -LiteralPath $index
} catch {
    Remove-Item -LiteralPath $OutputDir -Recurse -Force
    throw
}
if ($Publish) {
    Publish-GitHubAssets $repository $head "v$version" $version $Notes @($apk) $index
} else {
    Write-Output "Dry run: $version ($($manifest.size) bytes). Nothing published."
}
