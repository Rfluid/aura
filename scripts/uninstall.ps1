<#
.SYNOPSIS
    Uninstall Aura on Windows.

.DESCRIPTION
    Mirror of the `just uninstall-windows` recipe extracted as a
    standalone script so the README can run it via
    `iex (irm https://raw.githubusercontent.com/Rfluid/aura/main/scripts/uninstall.ps1)`
    without requiring a cloned checkout or `just`.

    Stops aura.exe, removes the Start Menu shortcut (legacy
    Startup-folder shortcut too if present), then removes the install
    directory at %LOCALAPPDATA%\Programs\Aura. Config + state in
    %APPDATA%\aura are preserved.

.NOTES
    Keep in sync with the `uninstall-windows:` recipe in justfile.
    ASCII-only source: avoids PowerShell 5.1 UTF-8-without-BOM parse errors.
#>

$ErrorActionPreference = 'Stop'

# PowerShell 5.1 defaults to TLS 1.0/1.1 on older Windows; force 1.2 in
# case anything below ever needs HTTPS (matches install.ps1).
try {
    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch {}

$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\Aura'
$StartMenu  = Join-Path ([Environment]::GetFolderPath('StartMenu')) 'Programs\Aura.lnk'
$Startup    = Join-Path ([Environment]::GetFolderPath('Startup')) 'Aura.lnk'

# Stop the running tray so the binary isn't locked when we delete it.
Get-Process aura -ErrorAction SilentlyContinue |
    Stop-Process -Force -ErrorAction SilentlyContinue

foreach ($p in @($StartMenu, $Startup)) {
    if (Test-Path $p) {
        Remove-Item $p
    }
}

if (Test-Path $InstallDir) {
    Remove-Item -Recurse -Force $InstallDir
}

Write-Host '✔ Removed binaries and Start Menu shortcut (config + state preserved)'
