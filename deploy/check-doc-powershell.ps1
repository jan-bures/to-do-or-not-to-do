<#
.SYNOPSIS
    Parses every PowerShell code block in the given Markdown files and fails on any syntax error, so the
    runbooks' commands work when they are pasted as written.

.EXAMPLE
    ./deploy/check-doc-powershell.ps1 docs/hosting-linux.md
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory, ValueFromRemainingArguments)][string[]]$Path
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$blocks = 0
$errorCount = 0
foreach ($file in $Path) {
    $lines = @(Get-Content -LiteralPath $file)
    $block = $null
    $firstLine = 0
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $trimmed = $lines[$i].Trim()
        if ($null -eq $block) {
            if ($trimmed -eq '```powershell') {
                $block = New-Object System.Collections.Generic.List[string]
                $firstLine = $i + 2
            }
        } elseif ($trimmed -eq '```') {
            $blocks++
            $parseErrors = $null
            [System.Management.Automation.Language.Parser]::ParseInput(($block -join "`n"), [ref]$null, [ref]$parseErrors) | Out-Null
            foreach ($e in $parseErrors) {
                Write-Host "${file}:$($firstLine + $e.Extent.StartLineNumber - 1): $($e.Message)"
                $errorCount++
            }
            $block = $null
        } else {
            $block.Add($lines[$i])
        }
    }
    if ($null -ne $block) {
        Write-Host "${file}:${firstLine}: a PowerShell block is never closed"
        $errorCount++
    }
}
Write-Host "$blocks PowerShell blocks parsed, $errorCount errors"
if ($errorCount -gt 0) { exit 1 }
