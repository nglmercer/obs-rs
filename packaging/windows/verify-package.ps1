param(
    [string]$PackageDirectory = $PSScriptRoot
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path -LiteralPath $PackageDirectory).Path
$sumsPath = Join-Path $root "SHA256SUMS.txt"
if (-not (Test-Path -LiteralPath $sumsPath -PathType Leaf)) {
    throw "SHA256SUMS.txt was not found in $root"
}

$checked = 0
foreach ($line in Get-Content -LiteralPath $sumsPath) {
    if ([string]::IsNullOrWhiteSpace($line)) {
        continue
    }
    $parts = $line -split "\s+", 2
    if ($parts.Count -ne 2 -or $parts[0] -notmatch '^[0-9a-fA-F]{64}$') {
        throw "Malformed checksum line: $line"
    }
    $relative = $parts[1].Trim()
    if ([System.IO.Path]::IsPathRooted($relative)) {
        throw "Checksum path must be relative: $relative"
    }
    $file = Join-Path $root ($relative.Replace("/", "\"))
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
        throw "Checksum target is missing: $relative"
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash
    if ($actual -ine $parts[0]) {
        throw "Checksum mismatch: $relative"
    }
    $checked++
}

Write-Output "Verified $checked OBS-RS package files"
