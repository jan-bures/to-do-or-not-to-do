# Shared publication guards for the Android and Windows release scripts. No local tags are created.
function Get-PublicReleaseRepository([string]$Root) {
    $line = Get-Content -LiteralPath (Join-Path $Root 'Cargo.toml') |
        Where-Object { $_ -match '^repository = "https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+"$' }
    if ($line -cnotmatch '^repository = "https://github\.com/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)"$') {
        throw 'No public GitHub repository URL in the root Cargo.toml.'
    }
    $slug = $Matches[1]
    $url = "https://github.com/$slug"
    $origin = git -C $Root remote get-url origin
    if ($LASTEXITCODE -ne 0 -or ($origin -replace '\.git$', '') -notin @($url, "git@github.com:$slug", "ssh://git@github.com/$slug")) {
        throw 'Origin must be the repository named in Cargo.toml.'
    }
    @{ Root = $Root; Slug = $slug; Url = $url; Target = "github.com/$slug" }
}

function Assert-PublicReleaseHead($Repository, [string]$Head) {
    $now = git -C $Repository.Root rev-parse HEAD
    if ($LASTEXITCODE -ne 0 -or $now -cne $Head) { throw 'HEAD changed during the build.' }
    $remoteMain = git -C $Repository.Root ls-remote --exit-code --refs origin refs/heads/main
    if ($LASTEXITCODE -ne 0 -or ($remoteMain -split '\s+')[0] -cne $Head) {
        throw 'Publishing requires HEAD to equal live origin/main.'
    }
    $dirty = git -C $Repository.Root status --porcelain
    if ($LASTEXITCODE -ne 0 -or $dirty) { throw 'Publishing requires a clean worktree.' }
}

function Publish-GitHubAssets($Repository, [string]$Head, [string]$Tag, [string]$Version,
                             [string]$Notes, [string[]]$Packages, [string]$Manifest) {
    Assert-PublicReleaseHead $Repository $Head
    $remoteTag = git -C $Repository.Root ls-remote --exit-code --refs origin "refs/tags/$Tag"
    $tagResult = $LASTEXITCODE
    if ($tagResult -eq 2) {
        # The remote API creates the lightweight tag at the checked commit.
        & gh release create $Tag --repo $Repository.Target --target $Head @Packages $Manifest `
            --title "To-do or not to do $Version" --notes $Notes --latest
        if ($LASTEXITCODE -ne 0) { throw 'GitHub did not create the release.' }
    } elseif ($tagResult -eq 0) {
        if (($remoteTag -split '\s+')[0] -cne $Head) { throw 'The release tag does not point to the build commit.' }
        $json = & gh release view $Tag --repo $Repository.Target --json assets,isDraft,isPrerelease
        if ($LASTEXITCODE -ne 0) { throw 'The release tag exists without a readable GitHub release.' }
        $release = $json | ConvertFrom-Json
        if ($release.isDraft -or $release.isPrerelease) { throw 'Cannot add assets to a draft or prerelease.' }
        $names = @($release.assets | ForEach-Object { $_.name })
        foreach ($file in @($Packages) + @($Manifest)) {
            if ([IO.Path]::GetFileName($file) -in $names) { throw 'The release already has a requested asset. Nothing overwritten.' }
        }
        # Upload the manifest last so clients cannot see an offer before all its packages exist.
        & gh release upload $Tag --repo $Repository.Target @Packages
        if ($LASTEXITCODE -ne 0) { throw 'GitHub asset upload failed. Check for partial uploads before retrying.' }
        & gh release upload $Tag --repo $Repository.Target $Manifest
        if ($LASTEXITCODE -ne 0) { throw 'GitHub manifest upload failed. Inspect the release before retrying.' }
        & gh release edit $Tag --repo $Repository.Target --latest
        if ($LASTEXITCODE -ne 0) { throw 'Assets uploaded but GitHub did not mark the release Latest.' }
    } else {
        throw 'Could not check the remote release tag.'
    }
    $latestTag = & gh api --hostname github.com "repos/$($Repository.Slug)/releases/latest" --jq '.tag_name'
    if ($LASTEXITCODE -ne 0 -or $latestTag -cne $Tag) {
        Write-Warning 'RELEASE PUBLISHED BUT NOT CONFIRMED AS LATEST. Check GitHub before retrying.'
    }
}
