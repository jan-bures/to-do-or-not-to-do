# Velopack/NuGet and the Rust updater compare dotted numeric prerelease identifiers numerically.
function Get-DesktopPackageVersion([string]$CrateVersion, [string]$FirstParentCount) {
    if ($CrateVersion -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
        throw 'The desktop crate version must be a stable major.minor.patch for packaging.'
    }
    $height = 0u
    if ($FirstParentCount -cnotmatch '^[1-9][0-9]*$' -or
        -not [uint32]::TryParse($FirstParentCount, [ref]$height)) {
        throw 'Expected a positive Git first-parent commit count.'
    }
    return [semver]"$CrateVersion-build.$height"
}
