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
    try {
        $file = [System.IO.Path]::GetFullPath(
            (Join-Path $root ($relative.Replace("/", "\"))))
    } catch {
        throw "Checksum path is invalid: $relative"
    }
    $rootPrefix = $root.TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
    if (-not $file.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Checksum path escapes the package directory: $relative"
    }
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
        throw "Checksum target is missing: $relative"
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash
    if ($actual -ine $parts[0]) {
        throw "Checksum mismatch: $relative"
    }
    $checked++
}

$runtimeMarker = Join-Path $root "GSTREAMER-RUNTIME.txt"
if (Test-Path -LiteralPath $runtimeMarker -PathType Leaf) {
    $runtimeFiles = @(
        @{ Path = (Join-Path $root "gstreamer\bin\gstreamer-1.0-0.dll"); Type = "Leaf" },
        @{ Path = (Join-Path $root "gstreamer\bin\gst-inspect-1.0.exe"); Type = "Leaf" },
        @{ Path = (Join-Path $root "gstreamer\lib\gstreamer-1.0"); Type = "Container" },
        @{ Path = (Join-Path $root "gstreamer\libexec\gstreamer-1.0\gst-plugin-scanner.exe"); Type = "Leaf" }
    )
    foreach ($runtimeFile in $runtimeFiles) {
        if (-not (Test-Path -LiteralPath $runtimeFile.Path -PathType $runtimeFile.Type)) {
            throw "GStreamer runtime marker is present but the runtime is incomplete: $($runtimeFile.Path)"
        }
    }
    Write-Output "Verified bundled GStreamer runtime inputs"
}
Write-Output "Verified $checked OBS-RS package files"
