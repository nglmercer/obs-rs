param(
    [ValidateSet("debug", "release")]
    [string]$Configuration = "release",
    [string]$OutputDirectory = (Join-Path $PSScriptRoot "dist"),
    [switch]$ProductionGStreamer,
    [string]$GStreamerRuntimeDirectory = ""
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$outputRoot = [System.IO.Path]::GetFullPath($OutputDirectory)
$stagingDirectory = Join-Path $outputRoot "obs-rs"
$helperManifest = Join-Path $repoRoot "packaging\windows\capture-helper\Cargo.toml"

if ($ProductionGStreamer) {
    if ([string]::IsNullOrWhiteSpace($GStreamerRuntimeDirectory)) {
        throw "-GStreamerRuntimeDirectory is required with -ProductionGStreamer"
    }
    $gstreamerRoot = (Resolve-Path -LiteralPath $GStreamerRuntimeDirectory).Path
    $gstreamerBin = Join-Path $gstreamerRoot "bin"
    if (-not (Test-Path -LiteralPath $gstreamerBin -PathType Container)) {
        throw "GStreamer runtime directory must contain a bin directory: $gstreamerBin"
    }
    $gstreamerPlugins = Join-Path $gstreamerRoot "lib\gstreamer-1.0"
    if (-not (Test-Path -LiteralPath $gstreamerPlugins -PathType Container)) {
        throw "GStreamer runtime directory must contain lib\gstreamer-1.0: $gstreamerPlugins"
    }
    $gstreamerCore = Join-Path $gstreamerBin "gstreamer-1.0-0.dll"
    if (-not (Test-Path -LiteralPath $gstreamerCore -PathType Leaf)) {
        throw "GStreamer runtime is missing the core DLL: $gstreamerCore"
    }
    $gstreamerScanner = Join-Path $gstreamerRoot "libexec\gstreamer-1.0\gst-plugin-scanner.exe"
    if (-not (Test-Path -LiteralPath $gstreamerScanner -PathType Leaf)) {
        throw "GStreamer runtime is missing the plugin scanner: $gstreamerScanner"
    }
} else {
    $gstreamerRoot = $null
    $gstreamerBin = $null
    $gstreamerPlugins = $null
}

$metadata = (& cargo metadata --format-version 1 --locked | Out-String | ConvertFrom-Json)
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed with exit code $LASTEXITCODE"
}
$guiPackage = @($metadata.packages | Where-Object { $_.name -eq "obs-rs-gui" }) | Select-Object -First 1
if ($null -eq $guiPackage) {
    throw "could not determine the OBS-RS GUI package version"
}
$version = $guiPackage.version
$archiveName = "obs-rs-windows-$version-x86_64.zip"
$archivePath = Join-Path $outputRoot $archiveName
$checksumPath = "$archivePath.sha256"

New-Item -ItemType Directory -Force -Path $outputRoot | Out-Null
if (Test-Path -LiteralPath $stagingDirectory) {
    Remove-Item -LiteralPath $stagingDirectory -Recurse -Force
}
if (Test-Path -LiteralPath $archivePath) {
    Remove-Item -LiteralPath $archivePath -Force
}
if (Test-Path -LiteralPath $checksumPath) {
    Remove-Item -LiteralPath $checksumPath -Force
}
New-Item -ItemType Directory -Force -Path $stagingDirectory | Out-Null

$profileArguments = if ($Configuration -eq "release") { @("--release") } else { @() }
$featureArguments = if ($ProductionGStreamer) { @("--features", "production-gstreamer") } else { @() }
& cargo build --locked -p obs-rs-gui $profileArguments $featureArguments
if ($LASTEXITCODE -ne 0) {
    throw "GUI build failed with exit code $LASTEXITCODE"
}
& cargo build --locked -p obs-rs-app --bin obs-rs --bin obs-rs-windows-check $profileArguments $featureArguments
if ($LASTEXITCODE -ne 0) {
    throw "application build failed with exit code $LASTEXITCODE"
}
& cargo build --locked --manifest-path $helperManifest $profileArguments
if ($LASTEXITCODE -ne 0) {
    throw "capture helper build failed with exit code $LASTEXITCODE"
}

$profileDirectory = if ($Configuration -eq "release") { "release" } else { "debug" }
$guiBinary = Join-Path $repoRoot "target\$profileDirectory\obs-rs-gui.exe"
$appBinary = Join-Path $repoRoot "target\$profileDirectory\obs-rs.exe"
$checkBinary = Join-Path $repoRoot "target\$profileDirectory\obs-rs-windows-check.exe"
$helperBinary = Join-Path $repoRoot "packaging\windows\capture-helper\target\$profileDirectory\obs-rs-capture-windows-helper.exe"
Copy-Item -LiteralPath $guiBinary -Destination $stagingDirectory
Copy-Item -LiteralPath $appBinary -Destination $stagingDirectory
Copy-Item -LiteralPath $checkBinary -Destination $stagingDirectory
Copy-Item -LiteralPath $helperBinary -Destination $stagingDirectory
Copy-Item -LiteralPath (Join-Path $PSScriptRoot "install.ps1") -Destination $stagingDirectory
Copy-Item -LiteralPath (Join-Path $PSScriptRoot "uninstall.ps1") -Destination $stagingDirectory
Copy-Item -LiteralPath (Join-Path $PSScriptRoot "run-obs-rs.ps1") -Destination $stagingDirectory
Copy-Item -LiteralPath (Join-Path $PSScriptRoot "verify-package.ps1") -Destination $stagingDirectory
Copy-Item -LiteralPath (Join-Path $PSScriptRoot "acceptance.ps1") -Destination $stagingDirectory

if ($ProductionGStreamer) {
    # Keep the native DLLs beside the entry points so Windows can resolve them
    # even when obs-rs-gui.exe is started directly. Plugins and the scanner
    # stay below gstreamer/ and the launcher sets their search paths.
    $gstreamerDirectory = Join-Path $stagingDirectory "gstreamer"
    $runtimeBinDestination = Join-Path $gstreamerDirectory "bin"
    New-Item -ItemType Directory -Force -Path $runtimeBinDestination | Out-Null
    Get-ChildItem -LiteralPath $gstreamerBin -Filter "*.dll" -File | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $stagingDirectory
        Copy-Item -LiteralPath $_.FullName -Destination $runtimeBinDestination
    }
    $pluginDestination = Join-Path $gstreamerDirectory "lib\gstreamer-1.0"
    New-Item -ItemType Directory -Force -Path $pluginDestination | Out-Null
    Get-ChildItem -LiteralPath $gstreamerPlugins -Force |
        Copy-Item -Destination $pluginDestination -Recurse

    $scanner = Join-Path $gstreamerRoot "libexec\gstreamer-1.0\gst-plugin-scanner.exe"
    $scannerDestination = Join-Path $gstreamerDirectory "libexec\gstreamer-1.0"
    New-Item -ItemType Directory -Force -Path $scannerDestination | Out-Null
    Copy-Item -LiteralPath $scanner -Destination $scannerDestination
    $share = Join-Path $gstreamerRoot "share\gstreamer-1.0"
    if (Test-Path -LiteralPath $share -PathType Container) {
        $shareDestination = Join-Path $gstreamerDirectory "share\gstreamer-1.0"
        New-Item -ItemType Directory -Force -Path $shareDestination | Out-Null
        Get-ChildItem -LiteralPath $share -Force |
            Copy-Item -Destination $shareDestination -Recurse
    }
    @(
        "GStreamer runtime: $gstreamerRoot",
        "Native output feature: production-gstreamer",
        "Launch with run-obs-rs.ps1 so PATH, GST_PLUGIN_PATH, and the plugin scanner are configured.",
        "The runtime and Cargo development package must come from the same GStreamer release."
    ) | Set-Content -LiteralPath (Join-Path $stagingDirectory "GSTREAMER-RUNTIME.txt") -Encoding utf8
}

$readmePath = Join-Path $repoRoot "packaging\windows\README.md"
Copy-Item -LiteralPath $readmePath -Destination (Join-Path $stagingDirectory "WINDOWS-README.md")
$version | Set-Content -LiteralPath (Join-Path $stagingDirectory "VERSION.txt") -Encoding utf8

$noticeLines = @(
    "# OBS-RS dependency notices",
    "",
    "This bundle was built from OBS-RS $version. The entries below are the resolved Cargo packages included by the workspace; license fields are taken from Cargo metadata.",
    ""
)
$noticeLines += @($metadata.packages |
    Where-Object { $null -ne $_.source } |
    Sort-Object name, version |
    ForEach-Object {
        $license = if ([string]::IsNullOrWhiteSpace($_.license)) { "license metadata unavailable" } else { $_.license }
        "- $($_.name) $($_.version) — $license — $($_.source)"
    })
$noticeLines | Set-Content -LiteralPath (Join-Path $stagingDirectory "THIRD-PARTY-NOTICES.md") -Encoding utf8

$payloadChecksums = @(Get-ChildItem -LiteralPath $stagingDirectory -File -Recurse |
    Sort-Object FullName |
    ForEach-Object {
        $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash.ToLowerInvariant()
        $relative = $_.FullName.Substring($stagingDirectory.Length).TrimStart("\", "/")
        "$hash  $($relative.Replace("\", "/"))"
    })
$payloadChecksums | Set-Content -LiteralPath (Join-Path $stagingDirectory "SHA256SUMS.txt") -Encoding ascii
Compress-Archive -Path $stagingDirectory -DestinationPath $archivePath -CompressionLevel Optimal
$archiveHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $archivePath).Hash.ToLowerInvariant()
"$archiveHash  $archiveName" | Set-Content -LiteralPath $checksumPath -Encoding ascii
Write-Output "Created $archivePath"
Write-Output "SHA256 $archiveHash"
