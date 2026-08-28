$ErrorActionPreference = "Stop"
$installRoot = (Resolve-Path $PSScriptRoot).Path
$versionPath = Join-Path $installRoot "VERSION.txt"
$version = if (Test-Path -LiteralPath $versionPath) {
    (Get-Content -LiteralPath $versionPath -Raw).Trim()
} else {
    "unknown"
}
$uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\OBS-RS"
$uninstallCommand = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$installRoot\uninstall.ps1`""

New-Item -Path $uninstallKey -Force | Out-Null
New-ItemProperty -Path $uninstallKey -Name DisplayName -Value "OBS-RS" -PropertyType String -Force | Out-Null
New-ItemProperty -Path $uninstallKey -Name DisplayVersion -Value $version -PropertyType String -Force | Out-Null
New-ItemProperty -Path $uninstallKey -Name InstallLocation -Value $installRoot -PropertyType String -Force | Out-Null
New-ItemProperty -Path $uninstallKey -Name UninstallString -Value $uninstallCommand -PropertyType String -Force | Out-Null
Write-Output "OBS-RS uninstall entry registered for $installRoot"
