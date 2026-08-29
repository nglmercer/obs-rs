param(
    [string]$PackageDirectory = $PSScriptRoot,
    [ValidateSet("", "rtmp", "rtmps", "srt", "rist", "whip", "webrtc", "hls")]
    [string[]]$RequiredStreamProtocol = @()
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path -LiteralPath $PackageDirectory).Path
$requiredStreamProtocols = @(
    $RequiredStreamProtocol |
        ForEach-Object { $_.Trim().ToLowerInvariant() } |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        ForEach-Object {
            if ($_ -in @("whip", "webrtc")) { "webrtc" } else { $_ }
        } |
        Select-Object -Unique
)
$sumsPath = Join-Path $root "SHA256SUMS.txt"
if (-not (Test-Path -LiteralPath $sumsPath -PathType Leaf)) {
    throw "SHA256SUMS.txt was not found in $root"
}

$requiredFiles = @(
    "obs-rs-gui.exe",
    "obs-rs.exe",
    "obs-rs-windows-check.exe",
    "obs-rs-capture-windows-helper.exe",
    "install.ps1",
    "uninstall.ps1",
    "run-obs-rs.ps1",
    "verify-package.ps1",
    "acceptance.ps1",
    "WINDOWS-README.md",
    "VERSION.txt",
    "THIRD-PARTY-NOTICES.md"
)
foreach ($relative in $requiredFiles) {
    $requiredPath = Join-Path $root $relative
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
        throw "Required OBS-RS package file is missing: $relative"
    }
}

$checked = 0
$rootPrefix = $root.TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
$listedFiles = New-Object 'System.Collections.Generic.HashSet[string]' `
    ([System.StringComparer]::OrdinalIgnoreCase)
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
    if (-not $file.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Checksum path escapes the package directory: $relative"
    }
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
        throw "Checksum target is missing: $relative"
    }
    $canonicalRelative = $file.Substring($rootPrefix.Length).Replace("\", "/")
    if (-not $listedFiles.Add($canonicalRelative)) {
        throw "Checksum target is listed more than once: $canonicalRelative"
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash
    if ($actual -ine $parts[0]) {
        throw "Checksum mismatch: $relative"
    }
    $checked++
}
$payloadFiles = @(Get-ChildItem -LiteralPath $root -File -Recurse -Force |
    Where-Object { $_.FullName -ne $sumsPath } |
    ForEach-Object {
        $_.FullName.Substring($rootPrefix.Length).Replace("\", "/")
    })
$unlistedFiles = @($payloadFiles | Where-Object { -not $listedFiles.Contains($_) })
if ($unlistedFiles.Count -gt 0) {
    $preview = ($unlistedFiles | Select-Object -First 10) -join ", "
    $suffix = if ($unlistedFiles.Count -gt 10) { ", ..." } else { "" }
    throw "Package payload files are missing from SHA256SUMS.txt: $preview$suffix"
}

$runtimeMarker = Join-Path $root "GSTREAMER-RUNTIME.txt"
if ($requiredStreamProtocols.Count -gt 0 -and
    -not (Test-Path -LiteralPath $runtimeMarker -PathType Leaf)) {
    throw "requested streaming protocol gates require a packaged GStreamer runtime marker"
}
if (Test-Path -LiteralPath $runtimeMarker -PathType Leaf) {
    $markerVersionLine = Get-Content -LiteralPath $runtimeMarker |
        Where-Object { $_ -match '^GStreamer version:\s*(?<version>\S+)\s*$' } |
        Select-Object -First 1
    if ($null -eq $markerVersionLine) {
        throw "GStreamer runtime marker does not contain a version"
    }
    $markerVersionMatch = [System.Text.RegularExpressions.Regex]::Match(
        $markerVersionLine,
        '^GStreamer version:\s*(?<version>\S+)\s*$')
    $runtimeVersion = $markerVersionMatch.Groups["version"].Value
    $streamGateLine = Get-Content -LiteralPath $runtimeMarker |
        Where-Object { $_ -match '^Streaming protocol gates?:\s*(?<protocols>\S+)\s*$' } |
        Select-Object -First 1
    $streamGates = if ($null -eq $streamGateLine) {
        @("none")
    } else {
        $gateText = ([System.Text.RegularExpressions.Regex]::Match(
                $streamGateLine,
                '^Streaming protocol gates?:\s*(?<protocols>\S+)\s*$')).Groups["protocols"].Value
        @($gateText -split ',' |
            ForEach-Object { $_.Trim().ToLowerInvariant() } |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
            ForEach-Object {
                if ($_ -in @("whip", "webrtc")) { "webrtc" } else { $_ }
            } |
            Select-Object -Unique)
    }
    if ($streamGates.Count -eq 0) {
        $streamGates = @("none")
    }
    $missingRequestedGates = @($requiredStreamProtocols | Where-Object {
        $_ -notin $streamGates
    })
    if ($missingRequestedGates.Count -gt 0) {
        throw "package streaming protocol gates do not match the requested acceptance protocols: gates=$($streamGates -join ',') missing=$($missingRequestedGates -join ',')"
    }
    $runtimeFiles = @(
        @{ Path = (Join-Path $root "gstreamer\bin\gstreamer-1.0-0.dll"); Type = "Leaf" },
        @{ Path = (Join-Path $root "gstreamer\bin\gst-inspect-1.0.exe"); Type = "Leaf" },
        @{ Path = (Join-Path $root "gstreamer\bin\gst-discoverer-1.0.exe"); Type = "Leaf" },
        @{ Path = (Join-Path $root "gstreamer\lib\gstreamer-1.0"); Type = "Container" },
        @{ Path = (Join-Path $root "gstreamer\libexec\gstreamer-1.0\gst-plugin-scanner.exe"); Type = "Leaf" }
    )
    foreach ($runtimeFile in $runtimeFiles) {
        if (-not (Test-Path -LiteralPath $runtimeFile.Path -PathType $runtimeFile.Type)) {
            throw "GStreamer runtime marker is present but the runtime is incomplete: $($runtimeFile.Path)"
        }
    }
    $runtime = Join-Path $root "gstreamer"
    $runtimeBin = Join-Path $runtime "bin"
    $runtimeInspect = Join-Path $runtimeBin "gst-inspect-1.0.exe"
    $runtimePlugins = Join-Path $runtime "lib\gstreamer-1.0"
    $runtimeScanner = Join-Path $runtime "libexec\gstreamer-1.0\gst-plugin-scanner.exe"
    $oldPath = $env:PATH
    $oldPluginPath = $env:GST_PLUGIN_PATH
    $oldPluginPath10 = $env:GST_PLUGIN_PATH_1_0
    $oldScanner = $env:GST_PLUGIN_SCANNER
    try {
        $env:PATH = "$runtimeBin;$root;$oldPath"
        $env:GST_PLUGIN_PATH = $runtimePlugins
        $env:GST_PLUGIN_PATH_1_0 = $runtimePlugins
        $env:GST_PLUGIN_SCANNER = $runtimeScanner
        $probeOutput = (& $runtimeInspect --version 2>&1 | Out-String).Trim()
        if ($LASTEXITCODE -ne 0) {
            throw "bundled GStreamer capability probe failed to start"
        }
        $probeMatch = [System.Text.RegularExpressions.Regex]::Match(
            $probeOutput,
            '(?im)\bversion\s+(?<version>\d+\.\d+(?:\.\d+)?)')
        if (-not $probeMatch.Success -or
            $probeMatch.Groups["version"].Value -ne $runtimeVersion) {
            throw "bundled GStreamer version does not match GSTREAMER-RUNTIME.txt"
        }
        & $runtimeInspect --exists appsrc 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "bundled GStreamer runtime cannot load the appsrc element"
        }
        function Test-BundledGStreamerElement {
            param(
                [Parameter(Mandatory = $true)]
                [string]$Element
            )

            & $runtimeInspect --exists $Element 2>&1 | Out-Null
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
            -not (Test-BundledGStreamerElement -Element $_)
        })
        if ($missingElements.Count -gt 0) {
            throw "bundled GStreamer runtime is missing production recording elements: $($missingElements -join ', ')"
        }
        $h264Encoders = @("vah264enc", "vaapih264enc", "nvh264enc", "openh264enc")
        $availableH264Encoders = @($h264Encoders | Where-Object {
            Test-BundledGStreamerElement -Element $_
        })
        if ($availableH264Encoders.Count -eq 0) {
            throw "bundled GStreamer runtime does not provide an approved H.264 encoder"
        }
        foreach ($streamGate in @($streamGates | Where-Object { $_ -ne "none" })) {
            $requiredStreamingElements = switch ($streamGate) {
                "rtmp" { @("flvmux") ; break }
                "rtmps" { @("flvmux") ; break }
                "srt" { @("mpegtsmux", "srtsink") ; break }
                "rist" { @("mpegtsmux", "rtpmp2tpay", "ristsink") ; break }
                "webrtc" { @("vp8enc", "opusenc", "webrtcbin", "whipclientsink") ; break }
                "hls" { @("hlssink2") ; break }
                default { throw "GStreamer runtime marker contains an unknown streaming protocol gate: $streamGate" }
            }
            $missingStreamingElements = @($requiredStreamingElements | Where-Object {
                -not (Test-BundledGStreamerElement -Element $_)
            })
            if ($streamGate -in @("rtmp", "rtmps")) {
                $hasRtmpSink = @(@("rtmp2sink", "rtmpsink") | Where-Object {
                    Test-BundledGStreamerElement -Element $_
                }).Count -gt 0
                if (-not $hasRtmpSink) {
                    $missingStreamingElements += "rtmp2sink or rtmpsink"
                }
            }
            if ($missingStreamingElements.Count -gt 0) {
                throw "bundled GStreamer runtime is missing production $streamGate streaming elements: $($missingStreamingElements -join ', ')"
            }
        }
    } finally {
        $env:PATH = $oldPath
        $env:GST_PLUGIN_PATH = $oldPluginPath
        $env:GST_PLUGIN_PATH_1_0 = $oldPluginPath10
        $env:GST_PLUGIN_SCANNER = $oldScanner
    }
    Write-Output "Verified bundled GStreamer runtime inputs"
}

$helper = Join-Path $root "obs-rs-capture-windows-helper.exe"
$helperOutput = (& $helper --protocol OBSRWIN1 --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "Windows capture helper version probe failed"
}
$helperMatch = [System.Text.RegularExpressions.Regex]::Match(
    $helperOutput,
    '(?m)^OBSRWIN1\tVERSION\t(?<version>\d+\.\d+(?:\.\d+)?)\s*$')
if (-not $helperMatch.Success) {
    throw "Windows capture helper returned an invalid OBSRWIN1 version reply: $helperOutput"
}
$packageVersion = (Get-Content -LiteralPath (Join-Path $root "VERSION.txt") -Raw).Trim()
$helperMajor = $helperMatch.Groups["version"].Value.Split('.')[0]
$packageMajor = $packageVersion.Split('.')[0]
if ($helperMajor -ne $packageMajor) {
    throw "Windows capture helper major version does not match package version: helper=$($helperMatch.Groups["version"].Value) package=$packageVersion"
}
Write-Output "Verified Windows capture helper protocol OBSRWIN1 version $($helperMatch.Groups["version"].Value)"
Write-Output "Verified $checked OBS-RS package files"
