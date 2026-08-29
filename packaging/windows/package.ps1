param(
    [ValidateSet("debug", "dev-fast", "release")]
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
$gstreamerVersion = $null

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
    $gstreamerInspect = Join-Path $gstreamerBin "gst-inspect-1.0.exe"
    if (-not (Test-Path -LiteralPath $gstreamerInspect -PathType Leaf)) {
        throw "GStreamer runtime is missing the capability probe tool: $gstreamerInspect"
    }
    $gstreamerScanner = Join-Path $gstreamerRoot "libexec\gstreamer-1.0\gst-plugin-scanner.exe"
    if (-not (Test-Path -LiteralPath $gstreamerScanner -PathType Leaf)) {
        throw "GStreamer runtime is missing the plugin scanner: $gstreamerScanner"
    }
    $probeOutput = (& $gstreamerInspect --version 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "GStreamer capability probe could not start: $gstreamerInspect"
    }
    $probeMatch = [System.Text.RegularExpressions.Regex]::Match(
        $probeOutput,
        '(?im)\bversion\s+(?<version>\d+\.\d+(?:\.\d+)?)')
    if (-not $probeMatch.Success) {
        throw "GStreamer capability probe returned no parseable version: $probeOutput"
    }
    $gstreamerVersion = $probeMatch.Groups["version"].Value
    function Test-GStreamerElement {
        param(
            [Parameter(Mandatory = $true)]
            [string]$Element
        )

        & $gstreamerInspect --exists $Element 2>&1 | Out-Null
        return $LASTEXITCODE -eq 0
    }
    $requiredElements = @(
        "appsrc",
        "queue",
        "videoconvert",
        "audioconvert",
        "audioresample",
        "avenc_aac",
        "h264parse",
        "matroskamux",
        "mp4mux",
        "filesrc",
        "matroskademux",
        "aacparse",
        "filesink"
    )
    $missingElements = @($requiredElements | Where-Object {
        -not (Test-GStreamerElement -Element $_)
    })
    if ($missingElements.Count -gt 0) {
        throw "GStreamer runtime is missing production recording elements: $($missingElements -join ', ')"
    }
    $h264Encoders = @("vah264enc", "vaapih264enc", "nvh264enc", "openh264enc")
    $availableH264Encoders = @($h264Encoders | Where-Object {
        Test-GStreamerElement -Element $_
    })
    if ($availableH264Encoders.Count -eq 0) {
        throw "GStreamer runtime does not provide an approved H.264 encoder"
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

$profileArguments = switch ($Configuration) {
    "release" { @("--release"); break }
    "dev-fast" { @("--profile", "dev-fast"); break }
    default { @() }
}
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

$profileDirectory = $Configuration
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
    Copy-Item -LiteralPath $gstreamerInspect -Destination $stagingDirectory
    Copy-Item -LiteralPath $gstreamerInspect -Destination $runtimeBinDestination
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
        "GStreamer version: $gstreamerVersion",
        "Native output feature: production-gstreamer",
        "Capability probe: gst-inspect-1.0.exe",
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
        "- $($_.name) $($_.version) - $license - $($_.source)"
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
