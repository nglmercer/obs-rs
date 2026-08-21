param(
    [ValidateSet("debug", "release")]
    [string]$Configuration = "release",
    [string]$OutputDirectory = (Join-Path $PSScriptRoot "dist")
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$outputRoot = [System.IO.Path]::GetFullPath($OutputDirectory)
$stagingDirectory = Join-Path $outputRoot "obs-rs"
$archivePath = Join-Path $outputRoot "obs-rs-windows.zip"
$helperManifest = Join-Path $repoRoot "packaging\windows\capture-helper\Cargo.toml"

New-Item -ItemType Directory -Force -Path $outputRoot | Out-Null
if (Test-Path -LiteralPath $stagingDirectory) {
    Remove-Item -LiteralPath $stagingDirectory -Recurse -Force
}
if (Test-Path -LiteralPath $archivePath) {
    Remove-Item -LiteralPath $archivePath -Force
}
New-Item -ItemType Directory -Force -Path $stagingDirectory | Out-Null

$cargoProfile = if ($Configuration -eq "release") { "--release" } else { "" }
& cargo build -p obs-rs-gui $cargoProfile
if ($LASTEXITCODE -ne 0) {
    throw "GUI build failed with exit code $LASTEXITCODE"
}
& cargo build --manifest-path $helperManifest $cargoProfile
if ($LASTEXITCODE -ne 0) {
    throw "capture helper build failed with exit code $LASTEXITCODE"
}

$profileDirectory = if ($Configuration -eq "release") { "release" } else { "debug" }
$guiBinary = Join-Path $repoRoot "target\$profileDirectory\obs-rs-gui.exe"
$helperBinary = Join-Path $repoRoot "packaging\windows\capture-helper\target\$profileDirectory\obs-rs-capture-windows-helper.exe"
Copy-Item -LiteralPath $guiBinary -Destination $stagingDirectory
Copy-Item -LiteralPath $helperBinary -Destination $stagingDirectory
Copy-Item -LiteralPath (Join-Path $PSScriptRoot "install.ps1") -Destination $stagingDirectory
Copy-Item -LiteralPath (Join-Path $PSScriptRoot "uninstall.ps1") -Destination $stagingDirectory

$readmePath = Join-Path $repoRoot "packaging\windows\README.md"
Copy-Item -LiteralPath $readmePath -Destination (Join-Path $stagingDirectory "WINDOWS-README.md")
Compress-Archive -Path $stagingDirectory -DestinationPath $archivePath -CompressionLevel Optimal
Write-Output "Created $archivePath"
