# Stage binaries from a cargo-dist Windows archive for WiX MSI packaging.
# Expects env vars: DIST_EXTRACTED (path to extracted archive root)
$ErrorActionPreference = 'Stop'

$distExtracted = if ($env:DIST_EXTRACTED) { $env:DIST_EXTRACTED } else { "dist_extracted" }
$stagingDir = "wix\msi\staging"
if (Test-Path $stagingDir) {
    Get-ChildItem -LiteralPath $stagingDir -Force | Remove-Item -Recurse -Force
} else {
    New-Item -ItemType Directory -Force -Path $stagingDir | Out-Null
}

# Find and copy the main executable
$exe = Get-ChildItem -Path $distExtracted -Filter "gemacast-pc.exe" -Recurse | Select-Object -First 1
if (-not $exe) {
    Write-Error "gemacast-pc.exe not found in extracted archive"
    exit 1
}
Copy-Item $exe.FullName -Destination $stagingDir
Write-Host "Staged: gemacast-pc.exe"

# Refuse an archive containing legacy ADB runtime binaries.
$legacyFiles = Get-ChildItem -Path $distExtracted -Recurse -File |
    Where-Object { $_.Name -in @("adb.exe", "AdbWinApi.dll", "AdbWinUsbApi.dll") }
if ($legacyFiles) {
    Write-Error "Legacy ADB files found in the cargo-dist archive: $($legacyFiles.FullName -join ', ')"
    exit 1
}

Write-Host "`nStaging complete. Contents:"
Get-ChildItem $stagingDir | Format-Table Name, Length
