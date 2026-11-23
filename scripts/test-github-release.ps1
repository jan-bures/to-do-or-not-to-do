#Requires -Version 7.0
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'github-release.ps1')

# Functions shadow the native commands. No GitHub operations, even read-only, happen in this test.
$script:main = 'expected-head'
$script:remoteTag = $null
$script:assets = @()
$script:calls = [Collections.Generic.List[string]]::new()
function git {
    $global:LASTEXITCODE = 0
    switch ($args[2]) {
        'rev-parse' { 'expected-head' }
        'status' { }
        'ls-remote' {
            if ($args[-1] -eq 'refs/heads/main') { "$script:main`trefs/heads/main" }
            elseif ($script:remoteTag) { "$script:remoteTag`t$($args[-1])" }
            else { $global:LASTEXITCODE = 2 }
        }
        default { throw 'Unexpected git operation in test.' }
    }
}
function gh {
    $global:LASTEXITCODE = 0
    if ($args[0] -eq 'api') { 'v0.1.0-build.9'; return }
    switch ($args[1]) {
        'view' {
            @{ isDraft = $false; isPrerelease = $false; assets = $script:assets } | ConvertTo-Json
        }
        { $_ -in @('create', 'upload', 'edit') } { $script:calls.Add($args -join ' ') }
        default { throw 'Unexpected GitHub operation in test.' }
    }
}
function Try-Publish {
    $repo = @{ Root = 'test'; Target = 'github.com/owner/project'; Slug = 'owner/project' }
    Publish-GitHubAssets $repo 'expected-head' 'v0.1.0-build.9' '0.1.0-build.9' 'Example notes' `
        @('installer.exe', 'full.nupkg') 'manifest.json'
}
function Assert-Blocked([string]$Message) {
    $script:calls.Clear()
    try { Try-Publish } catch {
        if ($_.Exception.Message -like "*$Message*" -and $script:calls.Count -eq 0) { return }
        throw
    }
    throw 'Expected publishing to stop before making a release change.'
}

Try-Publish
if ($script:calls.Count -ne 1 -or $script:calls[0] -notlike 'release create * --target expected-head installer.exe full.nupkg manifest.json *') {
    throw 'The first platform must create the release at the checked commit with all its assets.'
}
$script:remoteTag = 'expected-head'
$script:assets = @(@{ name = 'other-platform.json' })
$script:calls.Clear()
Try-Publish
if ($script:calls.Count -ne 3 -or $script:calls[0] -notlike 'release upload * installer.exe full.nupkg' -or
    $script:calls[1] -notlike 'release upload * manifest.json' -or $script:calls[2] -notlike 'release edit * --latest') {
    throw 'The second platform must publish its binaries before its manifest, without clobbering.'
}
$script:assets += @{ name = 'full.nupkg' }
Assert-Blocked 'already has a requested asset'
$script:remoteTag = 'wrong-head'
Assert-Blocked 'does not point to the build commit'
$script:main = 'new-main'
Assert-Blocked 'live origin/main'
Write-Host 'GitHub release publication guard checks passed.'
