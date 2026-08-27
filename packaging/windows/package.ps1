param(
    [ValidateSet("debug", "release")]
    [string]$Configuration = "release",
    [string]$OutputDirectory = (Join-Path $PSScriptRoot "dist")
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$outputRoot = [System.IO.Path]::GetFullPath($OutputDirectory)
$stagingDirectory = Join-Path $outputRoot "obs-rs"
$helperManifest = Join-Path $repoRoot "packaging\windows\capture-helper\Cargo.toml"

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
& cargo build --locked -p obs-rs-gui $profileArguments
if ($LASTEXITCODE -ne 0) {
    throw "GUI build failed with exit code $LASTEXITCODE"
}
& cargo build --locked -p obs-rs-app --bin obs-rs --bin obs-rs-windows-check $profileArguments
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

$payloadChecksums = @(Get-ChildItem -LiteralPath $stagingDirectory -File |
    Sort-Object Name |
    ForEach-Object {
        $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash.ToLowerInvariant()
        "$hash  $($_.Name)"
    })
$payloadChecksums | Set-Content -LiteralPath (Join-Path $stagingDirectory "SHA256SUMS.txt") -Encoding ascii
Compress-Archive -Path $stagingDirectory -DestinationPath $archivePath -CompressionLevel Optimal
$archiveHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $archivePath).Hash.ToLowerInvariant()
"$archiveHash  $archiveName" | Set-Content -LiteralPath $checksumPath -Encoding ascii
Write-Output "Created $archivePath"
Write-Output "SHA256 $archiveHash"
