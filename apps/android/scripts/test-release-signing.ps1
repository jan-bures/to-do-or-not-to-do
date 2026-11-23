# CI exercise of the real release signing and privacy gate. Never publishes.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $BuildTools,
    [Parameter(Mandatory)][string] $UnsignedApk,
    [Parameter(Mandatory)][string] $TempRoot
)
$ErrorActionPreference = 'Stop'
$test = {
    $scratch = Join-Path $TempRoot ('android-ota-' + [Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Path $scratch
    try {
        $signer = Join-Path $BuildTools $(if ($IsWindows) { 'apksigner.bat' } else { 'apksigner' })
        $old = Join-Path $scratch 'old.p12'
        $oldCert = Join-Path $scratch 'old.der'
        $env:TODO_ANDROID_KEYSTORE = Join-Path $scratch 'new.p12'
        $env:TODO_ANDROID_KEY_ALIAS = 'ci-release'
        $env:TODO_ANDROID_OLD_PASSWORD = [Guid]::NewGuid().ToString('N')
        $env:TODO_ANDROID_STORE_PASSWORD = [Guid]::NewGuid().ToString('N')
        $env:TODO_ANDROID_KEY_PASSWORD = $env:TODO_ANDROID_STORE_PASSWORD
        $env:TODO_ANDROID_LINEAGE = Join-Path $scratch 'rotation.lineage'
        $env:TODO_ANDROID_DENYLIST = Join-Path $scratch 'denylist.txt'
        Set-Content -LiteralPath $env:TODO_ANDROID_DENYLIST -Value 'fixture-private-value-not-in-apk'

        keytool -genkeypair -keystore $old -storetype PKCS12 -alias ci-old `
            -keyalg RSA -keysize 2048 -validity 1 -dname 'CN=Throwaway Android Old' -storepass:env TODO_ANDROID_OLD_PASSWORD
        if ($LASTEXITCODE -ne 0) { throw 'Old key creation failed.' }
        keytool -genkeypair -keystore $env:TODO_ANDROID_KEYSTORE -storetype PKCS12 -alias $env:TODO_ANDROID_KEY_ALIAS `
            -keyalg RSA -keysize 2048 -validity 1 -dname 'CN=Throwaway Android CI' -storepass:env TODO_ANDROID_STORE_PASSWORD
        if ($LASTEXITCODE -ne 0) { throw 'New key creation failed.' }
        keytool -exportcert -keystore $old -alias ci-old -storepass:env TODO_ANDROID_OLD_PASSWORD -file $oldCert
        if ($LASTEXITCODE -ne 0) { throw 'Old certificate export failed.' }

        & $signer rotate --out $env:TODO_ANDROID_LINEAGE `
            --old-signer --ks $old --ks-key-alias ci-old --ks-pass env:TODO_ANDROID_OLD_PASSWORD `
            --set-installed-data true --set-rollback false `
            --new-signer --ks $env:TODO_ANDROID_KEYSTORE --ks-key-alias $env:TODO_ANDROID_KEY_ALIAS `
            --ks-pass env:TODO_ANDROID_STORE_PASSWORD
        if ($LASTEXITCODE -ne 0) { throw 'Throwaway rotation failed.' }

        $releaseScript = Join-Path $PSScriptRoot 'release-apk.ps1'
        $env:TODO_ANDROID_OLD_CERT_SHA256 = '0' * 64
        $wrongCertRejected = $false
        try {
            & $releaseScript -BuildTools $BuildTools -UnsignedApk $UnsignedApk -OutputDir (Join-Path $scratch 'invalid')
        } catch {
            if ($_.Exception.Message -ne 'The lineage does not start with the expected old certificate.') { throw }
            $wrongCertRejected = $true
        }
        if (-not $wrongCertRejected) { throw 'Accepted the wrong old certificate.' }

        $env:TODO_ANDROID_OLD_CERT_SHA256 = (Get-FileHash -LiteralPath $oldCert).Hash
        & $releaseScript -BuildTools $BuildTools -UnsignedApk $UnsignedApk -OutputDir (Join-Path $scratch 'output')
    } finally { Remove-Item -LiteralPath $scratch -Recurse -Force }
}
$previous = @{}
foreach ($name in @('KEYSTORE', 'KEY_ALIAS', 'OLD_PASSWORD', 'STORE_PASSWORD', 'KEY_PASSWORD', 'LINEAGE', 'DENYLIST', 'OLD_CERT_SHA256')) {
    $key = "TODO_ANDROID_$name"
    $previous[$key] = [Environment]::GetEnvironmentVariable($key)
}
try { & $test } finally {
    foreach ($key in $previous.Keys) { [Environment]::SetEnvironmentVariable($key, $previous[$key]) }
}
