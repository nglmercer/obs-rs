param(
    [string]$PackageDirectory = $PSScriptRoot,
    [string]$OutputDirectory = (Join-Path $PSScriptRoot "acceptance-artifacts"),
    [ValidateRange(1, 28800)]
    [int]$SoakSeconds = 1800,
    [switch]$RequireCamera,
    [switch]$RequireProduction,
    [string]$ProductionStreamUrl = ""
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

& $verifier -PackageDirectory $root
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
    production_stream_configured = -not [string]::IsNullOrWhiteSpace($ProductionStreamUrl)
}
$metadata | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $artifacts "machine.json") -Encoding utf8

$oldHelper = $env:OBSR_CAPTURE_HELPER
$oldSoak = $env:OBS_RS_SOAK_SECONDS
$oldEndpoint = $env:OBS_RS_PRODUCTION_STREAM_URL
$oldArtifacts = $env:OBS_RS_ACCEPTANCE_ARTIFACTS
$requiredNames = @(
    "OBS_RS_REQUIRE_CAPTURE_HELPER",
    "OBS_RS_REQUIRE_DISCOVERY_STABILITY",
    "OBS_RS_REQUIRE_TARGET_PERSISTENCE",
    "OBS_RS_REQUIRE_DISPLAY",
    "OBS_RS_REQUIRE_DISPLAY_FRAME_RATES",
    "OBS_RS_REQUIRE_WINDOW",
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
}

$env:OBSR_CAPTURE_HELPER = $helper
$env:OBS_RS_SOAK_SECONDS = $SoakSeconds.ToString()
$env:OBS_RS_ACCEPTANCE_ARTIFACTS = $artifacts
if (-not [string]::IsNullOrWhiteSpace($ProductionStreamUrl)) {
    $env:OBS_RS_PRODUCTION_STREAM_URL = $ProductionStreamUrl
}

$resultPath = Join-Path $artifacts "windows-check.txt"
$guiSmokePath = Join-Path $artifacts "gui-smoke.txt"
try {
    & $launcher gui --smoke 2>&1 | Tee-Object -FilePath $guiSmokePath
    if ($LASTEXITCODE -ne 0) {
        throw "packaged GUI smoke test failed with exit code $LASTEXITCODE"
    }
    & $launcher check 2>&1 | Tee-Object -FilePath $resultPath
    if ($LASTEXITCODE -ne 0) {
        throw "Windows acceptance checks failed with exit code $LASTEXITCODE"
    }
} finally {
    $env:OBSR_CAPTURE_HELPER = $oldHelper
    $env:OBS_RS_SOAK_SECONDS = $oldSoak
    $env:OBS_RS_PRODUCTION_STREAM_URL = $oldEndpoint
    $env:OBS_RS_ACCEPTANCE_ARTIFACTS = $oldArtifacts
    foreach ($name in $oldRequired.Keys) {
        [Environment]::SetEnvironmentVariable($name, $oldRequired[$name], "Process")
    }
}

Write-Output "Windows acceptance artifacts: $artifacts"
