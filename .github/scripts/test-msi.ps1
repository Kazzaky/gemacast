# Full MSI installer test: custom-folder cycle -> install -> verify -> smoke test ->
# uninstall while running -> verify cleanup.
# Expects env var: MSI_PATH (absolute path to the .msi file)
$ErrorActionPreference = 'Stop'

$msiPath = $env:MSI_PATH
if (-not $msiPath -or -not (Test-Path $msiPath)) {
    Write-Error "MSI_PATH is not set or file does not exist: $msiPath"
    exit 1
}

$regKey = "HKLM:\SOFTWARE\EchaApriliyanto\Gemacast"

# The block the installer excludes from ephemeral allocation. Mirrors
# gemacast-core's Ports:: constants and Product.wxs's netsh custom actions.
$reservedStart = 23555
$reservedEnd = 23559

# Whether netsh reports our exclusion for one protocol. The table prints
# "<start> <end>" per row, so both endpoints on one line is an exact match.
function Test-PortExclusion($proto) {
    $out = netsh int ipv4 show excludedportrange protocol=$proto 2>&1 | Out-String
    return $out -match "\b$reservedStart\s+$reservedEnd\b"
}

# ── Custom Folder Cycle ───────────────────────────────────────────────────
# Gates the directory nesting and the remembered install location. The browse
# dialog retargets INSTALLROOT, so passing it here is the silent equivalent.
# This runs first so the registry is clean for the default install below, which
# makes uninstall's removal of the remembered path load-bearing.
# It cannot gate the dialog wiring itself - that needs a human clicking Change.
Write-Host "=== Installing MSI to a custom folder ==="
$customRoot = "$env:SystemDrive\GemacastCustomTest"
$customDir = "$customRoot\Gemacast"

$proc = Start-Process msiexec -ArgumentList "/i", "`"$msiPath`"", "/qn", "/norestart", "INSTALLROOT=`"$customRoot`"" -Wait -PassThru
if ($proc.ExitCode -ne 0) {
    Write-Error "Custom-folder install failed with exit code $($proc.ExitCode)"
    exit 1
}

$errors = @()

if (-not (Test-Path "$customDir\gemacast-pc.exe")) {
    $errors += "gemacast-pc.exe not found in $customDir"
} else {
    Write-Host "PASS: gemacast-pc.exe installed under the Gemacast subfolder"
}
$legacyFiles = @("adb.exe", "AdbWinApi.dll", "AdbWinUsbApi.dll") |
    Where-Object { Test-Path (Join-Path $customDir $_) }
if ($legacyFiles) {
    $errors += "legacy ADB files were staged: $($legacyFiles -join ', ')"
} else {
    Write-Host "PASS: no legacy ADB files in custom installation"
}

# The reported bug was files landing straight in the folder the user picked.
$flat = Get-ChildItem $customRoot -File -ErrorAction SilentlyContinue
if ($flat) {
    $errors += "files scattered flat in ${customRoot}: $($flat.Name -join ', ')"
} else {
    Write-Host "PASS: nothing scattered flat in the picked folder"
}

# Remembered so the next upgrade does not relocate the install.
$remembered = (Get-ItemProperty -Path $regKey -Name InstallDir -ErrorAction SilentlyContinue).InstallDir
if ($remembered -and $remembered.TrimEnd('\') -eq $customDir) {
    Write-Host "PASS: install location remembered as $remembered"
} else {
    $errors += "remembered InstallDir is '$remembered', expected '$customDir'"
}

Write-Host "`n=== Uninstalling the custom-folder install ==="
$proc = Start-Process msiexec -ArgumentList "/x", "`"$msiPath`"", "/qn", "/norestart" -Wait -PassThru
if ($proc.ExitCode -ne 0) {
    $errors += "custom-folder uninstall failed with exit code $($proc.ExitCode)"
}

$leftover = Get-ChildItem $customDir -Recurse -ErrorAction SilentlyContinue
if ($leftover) {
    $errors += "custom install directory still contains: $($leftover.Name -join ', ')"
} else {
    Write-Host "PASS: custom install directory cleaned up"
}

$remembered = (Get-ItemProperty -Path $regKey -Name InstallDir -ErrorAction SilentlyContinue).InstallDir
if ($remembered) {
    $errors += "remembered InstallDir survived uninstall: $remembered"
} else {
    Write-Host "PASS: remembered install location removed"
}

if ($errors.Count -gt 0) {
    Write-Host "`n=== CUSTOM FOLDER FAILURES ==="
    $errors | ForEach-Object { Write-Host "FAIL: $_" }
    exit 1
}
if ((Test-Path $customRoot) -and ((Split-Path $customRoot -Leaf) -eq "GemacastCustomTest")) {
    Remove-Item $customRoot -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Host "`nAll custom-folder checks passed"

# ── Install ────────────────────────────────────────────────────────────────
Write-Host "`n=== Installing MSI ==="
$proc = Start-Process msiexec -ArgumentList "/i", "`"$msiPath`"", "/qn", "/norestart" -Wait -PassThru
if ($proc.ExitCode -ne 0) {
    Write-Error "MSI install failed with exit code $($proc.ExitCode)"
    exit 1
}
Write-Host "MSI installed successfully"

# ── Verify Installation ───────────────────────────────────────────────────
Write-Host "`n=== Verifying Installation ==="
$installDir = "$env:ProgramFiles\Gemacast"
$errors = @()

# Check main executable
if (-not (Test-Path "$installDir\gemacast-pc.exe")) {
    $errors += "gemacast-pc.exe not found in $installDir"
} else {
    Write-Host "PASS: gemacast-pc.exe exists"
}

# The application must not install the removed ADB runtime.
$legacyFiles = @("adb.exe", "AdbWinApi.dll", "AdbWinUsbApi.dll") |
    Where-Object { Test-Path (Join-Path $installDir $_) }
if ($legacyFiles) {
    $errors += "legacy ADB files were installed: $($legacyFiles -join ', ')"
} else {
    Write-Host "PASS: no legacy ADB files installed"
}

# Check firewall rules
$tcpRule = netsh advfirewall firewall show rule name="Gemacast (TCP)" 2>&1
if ($tcpRule -match "Gemacast") {
    Write-Host "PASS: TCP firewall rule exists"
} else {
    $errors += "TCP firewall rule 'Gemacast (TCP)' not found"
}

$udpRule = netsh advfirewall firewall show rule name="Gemacast (UDP)" 2>&1
if ($udpRule -match "Gemacast") {
    Write-Host "PASS: UDP firewall rule exists"
} else {
    $errors += "UDP firewall rule 'Gemacast (UDP)' not found"
}

# Check port reservation
foreach ($proto in @("tcp", "udp")) {
    if (Test-PortExclusion $proto) {
        Write-Host "PASS: $proto ports $reservedStart-$reservedEnd excluded from ephemeral allocation"
    } else {
        $errors += "$proto ports $reservedStart-$reservedEnd not excluded after install"
    }
}

# Check Start Menu shortcut
$startMenu = "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\Gemacast.lnk"
if (Test-Path $startMenu) {
    Write-Host "PASS: Start Menu shortcut exists"
} else {
    # Try alternative location
    $altMenu = Get-ChildItem "$env:ProgramData\Microsoft\Windows\Start Menu\Programs" -Filter "*Gemacast*" -Recurse -ErrorAction SilentlyContinue
    if ($altMenu) {
        Write-Host "PASS: Start Menu shortcut found at $($altMenu.FullName)"
    } else {
        $errors += "Start Menu shortcut not found"
    }
}

# Smoke test: start the app and deliberately leave it running, so the uninstall
# below has to close it. That is the second reported bug - Restart Manager cannot
# close a tray app or a windowless adb, so without the rescheduled terminate the
# uninstall stops on the "files in use" dialog.
Write-Host "`nSmoke test: starting gemacast-pc.exe..."
$app = Start-Process -FilePath "$installDir\gemacast-pc.exe" -PassThru

try {
    Start-Sleep -Seconds 10

    if ($app.HasExited) {
        $errors += "gemacast-pc.exe exited prematurely with code $($app.ExitCode)"
    } else {
        Write-Host "PASS: gemacast-pc.exe is running (PID $($app.Id))"
    }

    # The desktop process remains running so CloseGemacast must terminate it
    # before MSI can remove its executable and installation directory.

    if ($errors.Count -gt 0) {
        Write-Host "`n=== INSTALLATION FAILURES ==="
        $errors | ForEach-Object { Write-Host "FAIL: $_" }
        exit 1
    }
    Write-Host "`nAll installation checks passed"

    # ── Uninstall, with the app still running ─────────────────────────────
    Write-Host "`n=== Uninstalling MSI (app still running) ==="
    $proc = Start-Process msiexec -ArgumentList "/x", "`"$msiPath`"", "/qn", "/norestart" -Wait -PassThru
    # Exactly 0. 3010 means a reboot was requested, which is what a failed close
    # looks like from out here.
    if ($proc.ExitCode -ne 0) {
        Write-Error "MSI uninstall failed with exit code $($proc.ExitCode)"
        exit 1
    }
    Write-Host "MSI uninstalled successfully"

    # Checked here rather than below, because the cleanup in `finally` would make
    # the assertion pass on its own.
    $survivors = @()
    if (Get-Process gemacast-pc -ErrorAction SilentlyContinue) {
        $survivors += "gemacast-pc"
    } else {
        Write-Host "PASS: no gemacast-pc process survived the uninstall"
    }
    if ($survivors.Count -gt 0) {
        Write-Host "`n=== UNINSTALL FAILURES ==="
        $survivors | ForEach-Object { Write-Host "FAIL: $_ is still running after uninstall" }
        exit 1
    }
}
finally {
    # A failed assertion above must not leave the desktop process behind.
    Get-Process gemacast-pc -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}

# ── Verify Uninstall ──────────────────────────────────────────────────────
Write-Host "`n=== Verifying Uninstall Cleanup ==="
$errors = @()

# Installation directory should be removed
if (Test-Path $installDir) {
    $remaining = Get-ChildItem $installDir -Recurse
    if ($remaining.Count -gt 0) {
        $errors += "Installation directory still contains files: $($remaining.Name -join ', ')"
    } else {
        Write-Host "PASS: Installation directory is empty (will be removed)"
    }
} else {
    Write-Host "PASS: Installation directory removed"
}

# Firewall rules should be cleaned up
$tcpRule = netsh advfirewall firewall show rule name="Gemacast (TCP)" 2>&1
if ($tcpRule -match "No rules match") {
    Write-Host "PASS: TCP firewall rule removed"
} else {
    $errors += "TCP firewall rule still exists after uninstall"
}

$udpRule = netsh advfirewall firewall show rule name="Gemacast (UDP)" 2>&1
if ($udpRule -match "No rules match") {
    Write-Host "PASS: UDP firewall rule removed"
} else {
    $errors += "UDP firewall rule still exists after uninstall"
}

# The exclusion must not outlive the product - it would keep the block off-limits
# to every other program on the box for nothing.
foreach ($proto in @("tcp", "udp")) {
    if (Test-PortExclusion $proto) {
        $errors += "$proto ports $reservedStart-$reservedEnd still excluded after uninstall"
    } else {
        Write-Host "PASS: $proto port exclusion removed"
    }
}

if ($errors.Count -gt 0) {
    Write-Host "`n=== UNINSTALL FAILURES ==="
    $errors | ForEach-Object { Write-Host "FAIL: $_" }
    exit 1
}
Write-Host "`nAll uninstall checks passed"
# Explicit, because the last thing above is netsh reporting "No rules match" with
# exit code 1. Without this the script inherits it and a clean run fails the job.
exit 0
