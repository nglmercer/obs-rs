param(
    [string]$PackageDirectory = $PSScriptRoot,
    [string]$OutputDirectory = (Join-Path $PSScriptRoot "acceptance-artifacts"),
    [ValidateRange(1, 28800)]
    [int]$SoakSeconds = 1800,
    [switch]$RequireCamera,
    [switch]$RequireProduction,
    [switch]$RequireProductionHls,
    [string]$ProductionStreamUrl = $env:OBS_RS_PRODUCTION_STREAM_URL
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path -LiteralPath $PackageDirectory).Path
$artifacts = [System.IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $artifacts | Out-Null

$verifier = Join-Path $root "verify-package.ps1"
$launcher = Join-Path $root "run-obs-rs.ps1"
$helper = Join-Path $root "obs-rs-capture-windows-helper.exe"
foreach ($requiredPath in @($verifier, $launcher, $helper)) {
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
        throw "Windows acceptance package is missing $requiredPath"
    }
}

$requiredStreamProtocols = @()
if (-not [string]::IsNullOrWhiteSpace($ProductionStreamUrl)) {
    $productionEndpoint = $ProductionStreamUrl.Trim()
    $delimiter = $productionEndpoint.IndexOf("://", [System.StringComparison]::Ordinal)
    if ($delimiter -lt 1 -or $delimiter + 3 -ge $productionEndpoint.Length) {
        throw "-ProductionStreamUrl must use a non-empty rtmp://, rtmps://, srt://, rist://, whip://, or webrtc:// endpoint"
    }
    $requiredStreamProtocol = $productionEndpoint.Substring(0, $delimiter).ToLowerInvariant()
    if ($requiredStreamProtocol -in @("whip", "webrtc")) {
        $requiredStreamProtocol = "webrtc"
    }
    if ($requiredStreamProtocol -notin @("rtmp", "rtmps", "srt", "rist", "webrtc")) {
        throw "-ProductionStreamUrl uses unsupported protocol $requiredStreamProtocol"
    }
    $requiredStreamProtocols += $requiredStreamProtocol
}
if ($RequireProductionHls) {
    $requiredStreamProtocols += "hls"
}
$requiredStreamProtocols = @($requiredStreamProtocols | Select-Object -Unique)
$verifyArguments = @{
    PackageDirectory = $root
}
if ($requiredStreamProtocols.Count -gt 0) {
    $verifyArguments["RequiredStreamProtocol"] = $requiredStreamProtocols
}
& $verifier @verifyArguments
if ($null -ne $LASTEXITCODE -and $LASTEXITCODE -ne 0) {
    throw "package verification failed with exit code $LASTEXITCODE"
}

function Get-OptionalCimInstances {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ClassName
    )

    @(Get-CimInstance -ClassName $ClassName -ErrorAction SilentlyContinue)
}

$computer = Get-OptionalCimInstances -ClassName "Win32_ComputerSystem" |
    Select-Object -First 1 | Select-Object Manufacturer, Model, SystemType
$operatingSystem = Get-OptionalCimInstances -ClassName "Win32_OperatingSystem" |
    Select-Object -First 1 | Select-Object Caption, Version, BuildNumber, OSArchitecture
$gpu = Get-OptionalCimInstances -ClassName "Win32_VideoController" |
    Select-Object Name, DriverVersion, VideoModeDescription
$monitors = Get-OptionalCimInstances -ClassName "Win32_DesktopMonitor" |
    Select-Object Name, MonitorManufacturer, ScreenWidth, ScreenHeight, PNPDeviceID
$audio = Get-OptionalCimInstances -ClassName "Win32_SoundDevice" |
    Select-Object Name, Manufacturer, Status, PNPDeviceID

$metadata = [ordered]@{
    captured_at_utc = [DateTime]::UtcNow.ToString("o")
    computer = $computer
    operating_system = $operatingSystem
    gpu = @($gpu)
    monitors = @($monitors)
    audio = @($audio)
    soak_seconds = $SoakSeconds
    camera_required = [bool]$RequireCamera
    production_required = [bool]$RequireProduction
    production_hls_required = [bool]$RequireProductionHls
    production_stream_configured = -not [string]::IsNullOrWhiteSpace($ProductionStreamUrl)
}
$metadata | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $artifacts "machine.json") -Encoding utf8

$oldHelper = $env:OBSR_CAPTURE_HELPER
$oldSoak = $env:OBS_RS_SOAK_SECONDS
$oldEndpoint = $env:OBS_RS_PRODUCTION_STREAM_URL
$oldArtifacts = $env:OBS_RS_ACCEPTANCE_ARTIFACTS
$oldDiscoverer = $env:OBSR_GST_DISCOVERER
$requiredNames = @(
    "OBS_RS_REQUIRE_CAPTURE_HELPER",
    "OBS_RS_REQUIRE_DISCOVERY_STABILITY",
    "OBS_RS_REQUIRE_TARGET_PERSISTENCE",
    "OBS_RS_REQUIRE_DISPLAY",
    "OBS_RS_REQUIRE_DISPLAY_FRAME_RATES",
    "OBS_RS_REQUIRE_WINDOW",
    "OBS_RS_REQUIRE_WINDOW_LIFECYCLE",
    "OBS_RS_REQUIRE_REFERENCE_RECORDING",
    "OBS_RS_REQUIRE_AUDIO_DEVICE_STABILITY",
    "OBS_RS_REQUIRE_MICROPHONE",
    "OBS_RS_REQUIRE_DESKTOP_LOOPBACK",
    "OBS_RS_REQUIRE_MONITOR_OUTPUT",
    "OBS_RS_REQUIRE_AV_SOAK",
    "OBS_RS_REQUIRE_CLEANUP_RESTART"
)
$oldRequired = @{}
foreach ($name in $requiredNames) {
    $oldRequired[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
    [Environment]::SetEnvironmentVariable($name, "1", "Process")
}
if ($RequireCamera) {
    $oldRequired["OBS_RS_REQUIRE_CAMERA"] = [Environment]::GetEnvironmentVariable(
        "OBS_RS_REQUIRE_CAMERA", "Process")
    [Environment]::SetEnvironmentVariable("OBS_RS_REQUIRE_CAMERA", "1", "Process")
}
if (-not [string]::IsNullOrWhiteSpace($ProductionStreamUrl)) {
    $oldRequired["OBS_RS_REQUIRE_PRODUCTION_OUTPUT"] = [Environment]::GetEnvironmentVariable(
        "OBS_RS_REQUIRE_PRODUCTION_OUTPUT", "Process")
    $oldRequired["OBS_RS_REQUIRE_PRODUCTION_RECORDING"] = [Environment]::GetEnvironmentVariable(
        "OBS_RS_REQUIRE_PRODUCTION_RECORDING", "Process")
    $oldRequired["OBS_RS_REQUIRE_PRODUCTION_STREAMING"] = [Environment]::GetEnvironmentVariable(
        "OBS_RS_REQUIRE_PRODUCTION_STREAMING", "Process")
    [Environment]::SetEnvironmentVariable("OBS_RS_REQUIRE_PRODUCTION_OUTPUT", "1", "Process")
    [Environment]::SetEnvironmentVariable("OBS_RS_REQUIRE_PRODUCTION_RECORDING", "1", "Process")
    [Environment]::SetEnvironmentVariable("OBS_RS_REQUIRE_PRODUCTION_STREAMING", "1", "Process")
}
if ($RequireProduction) {
    foreach ($name in @(
        "OBS_RS_REQUIRE_PRODUCTION_OUTPUT",
        "OBS_RS_REQUIRE_PRODUCTION_RECORDING"
    )) {
        if (-not $oldRequired.ContainsKey($name)) {
            $oldRequired[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
        }
        [Environment]::SetEnvironmentVariable($name, "1", "Process")
    }
    $oldRequired["OBS_RS_REQUIRE_MEDIA_SOURCE"] = [Environment]::GetEnvironmentVariable(
        "OBS_RS_REQUIRE_MEDIA_SOURCE", "Process")
    [Environment]::SetEnvironmentVariable("OBS_RS_REQUIRE_MEDIA_SOURCE", "1", "Process")
}
if ($RequireProductionHls) {
    $oldRequired["OBS_RS_REQUIRE_PRODUCTION_HLS"] = [Environment]::GetEnvironmentVariable(
        "OBS_RS_REQUIRE_PRODUCTION_HLS", "Process")
    [Environment]::SetEnvironmentVariable("OBS_RS_REQUIRE_PRODUCTION_HLS", "1", "Process")
}

if ($RequireProduction -or $RequireProductionHls -or
    -not [string]::IsNullOrWhiteSpace($ProductionStreamUrl)) {
    $runtimeMarker = Join-Path $root "GSTREAMER-RUNTIME.txt"
    if (-not (Test-Path -LiteralPath $runtimeMarker -PathType Leaf)) {
        throw "production acceptance requires a package built with -ProductionGStreamer"
    }
}

function Test-ProductionRecordingArtifact {
    param(
        [Parameter(Mandatory = $true)]
        [string]$RecordingPath,
        [Parameter(Mandatory = $true)]
        [string]$ArtifactDirectory
    )

    if (-not (Test-Path -LiteralPath $RecordingPath -PathType Leaf)) {
        throw "production acceptance did not produce a recording artifact: $RecordingPath"
    }
    $discoverer = $env:OBSR_GST_DISCOVERER
    if ([string]::IsNullOrWhiteSpace($discoverer)) {
        $discoverer = Join-Path $root "gstreamer\bin\gst-discoverer-1.0.exe"
    }
    if (-not (Test-Path -LiteralPath $discoverer -PathType Leaf)) {
        throw "production acceptance is missing the bundled recording probe: $discoverer"
    }
    $probePath = Join-Path $ArtifactDirectory "production-recording-discovery.txt"
    $probeOutput = @(& $discoverer -v $RecordingPath 2>&1)
    $probeExitCode = $LASTEXITCODE
    $probeText = $probeOutput | Out-String
    $probeOutput | Set-Content -LiteralPath $probePath -Encoding utf8
    if ($probeExitCode -ne 0) {
        throw "production recording could not be discovered by gst-discoverer (exit $probeExitCode); see $probePath"
    }
    if ($probeText -notmatch '(?im)\bvideo\s*:' -or
        $probeText -notmatch '(?im)\baudio\s*:') {
        throw "production recording discovery did not report both video and audio streams; see $probePath"
    }
    Write-Output "Verified playable production recording with audio/video streams: $RecordingPath"
}

$env:OBSR_CAPTURE_HELPER = $helper
$env:OBS_RS_SOAK_SECONDS = $SoakSeconds.ToString()
$env:OBS_RS_ACCEPTANCE_ARTIFACTS = $artifacts
if (-not [string]::IsNullOrWhiteSpace($ProductionStreamUrl)) {
    $env:OBS_RS_PRODUCTION_STREAM_URL = $ProductionStreamUrl
}

$resultPath = Join-Path $artifacts "windows-check.txt"
$reportPath = Join-Path $artifacts "windows-check.json"
$guiSmokePath = Join-Path $artifacts "gui-smoke.txt"
try {
    & $launcher gui --smoke 2>&1 | Tee-Object -FilePath $guiSmokePath
    if ($LASTEXITCODE -ne 0) {
        throw "packaged GUI smoke test failed with exit code $LASTEXITCODE"
    }

    $checkOutput = @(& $launcher check 2>&1)
    $checkExitCode = $LASTEXITCODE
    $checkOutput | Tee-Object -FilePath $resultPath

    $checkRecords = [System.Collections.Generic.List[object]]::new()
    $unparsedOutput = [System.Collections.Generic.List[string]]::new()
    foreach ($line in $checkOutput) {
        $lineText = [string]$line
        if ($lineText -match '^\s*check=(?<name>\S+)\s+status=(?<status>pass|skip|fail)\s+detail=(?<detail>.*)$') {
            $checkRecords.Add([ordered]@{
                    name = $Matches.name
                    status = $Matches.status
                    detail = $Matches.detail
                })
        } else {
            $unparsedOutput.Add($lineText)
        }
    }

    $counts = [ordered]@{
        pass = @($checkRecords | Where-Object { $_.status -eq "pass" }).Count
        skip = @($checkRecords | Where-Object { $_.status -eq "skip" }).Count
        fail = @($checkRecords | Where-Object { $_.status -eq "fail" }).Count
    }
    $report = [ordered]@{
        schema_version = 1
        captured_at_utc = [DateTime]::UtcNow.ToString("o")
        status = if ($checkExitCode -eq 0) { "pass" } else { "fail" }
        exit_code = $checkExitCode
        counts = $counts
        checks = @($checkRecords)
        unparsed_output = @($unparsedOutput)
    }
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $reportPath -Encoding utf8

    if ($checkExitCode -ne 0) {
        throw "Windows acceptance checks failed with exit code $checkExitCode; see $reportPath"
    }
    if ($RequireProduction -or
        -not [string]::IsNullOrWhiteSpace($ProductionStreamUrl)) {
        Test-ProductionRecordingArtifact `
            -RecordingPath (Join-Path $artifacts "production-recording.mkv") `
            -ArtifactDirectory $artifacts
    }
} finally {
    $env:OBSR_CAPTURE_HELPER = $oldHelper
    $env:OBS_RS_SOAK_SECONDS = $oldSoak
    $env:OBS_RS_PRODUCTION_STREAM_URL = $oldEndpoint
    $env:OBS_RS_ACCEPTANCE_ARTIFACTS = $oldArtifacts
    $env:OBSR_GST_DISCOVERER = $oldDiscoverer
    foreach ($name in $oldRequired.Keys) {
        [Environment]::SetEnvironmentVariable($name, $oldRequired[$name], "Process")
    }
}

Write-Output "Windows acceptance artifacts: $artifacts"
