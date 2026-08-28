param(
    [Parameter(Position = 0)]
    [ValidateSet("gui", "app", "check")]
    [string]$EntryPoint = "gui",
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Arguments
)

$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
$runtime = Join-Path $root "gstreamer"
$runtimeBin = Join-Path $runtime "bin"
$runtimePlugins = Join-Path $runtime "lib\gstreamer-1.0"
$runtimeScanner = Join-Path $runtime "libexec\gstreamer-1.0\gst-plugin-scanner.exe"

if (Test-Path -LiteralPath $runtimeBin -PathType Container) {
    $env:PATH = "$runtimeBin;$root;$env:PATH"
    $env:GST_PLUGIN_PATH = $runtimePlugins
    $env:GST_PLUGIN_PATH_1_0 = $runtimePlugins
    if (Test-Path -LiteralPath $runtimeScanner -PathType Leaf) {
        $env:GST_PLUGIN_SCANNER = $runtimeScanner
    }
    $env:GST_REGISTRY = Join-Path $runtime "registry.bin"
}

$executable = switch ($EntryPoint) {
    "gui" { Join-Path $root "obs-rs-gui.exe" }
    "app" { Join-Path $root "obs-rs.exe" }
    "check" { Join-Path $root "obs-rs-windows-check.exe" }
}
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "OBS-RS entry point was not found: $executable"
}

& $executable @Arguments
exit $LASTEXITCODE
