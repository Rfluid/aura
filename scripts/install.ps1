<#
.SYNOPSIS
    Install Aura on Windows.

.DESCRIPTION
    Two modes (mirror of install.sh):

      * source   — run from a cloned repo: build with cargo, then install.
      * release  — run via `iex (irm ...)`: download the latest GitHub
                   release zip for the host, verify its checksum, install.

    Auto-detects the mode (source when run from a checkout with cargo on
    PATH, release otherwise). Override with $env:AURA_INSTALL_MODE.
    Pin a version with $env:AURA_VERSION = 'v1.2.3'.

    Aura is a tray-indicator app: the icon next to the clock is the
    entire UI, so the process needs to be running at sign-in for there
    to be anything to click. Binaries land in
    %LOCALAPPDATA%\Programs\Aura. The installer creates both:

      * a Startup-folder shortcut — runs aura.exe minimised at sign-in
        (autostart);
      * a Start Menu shortcut — for discoverability and a manual launch
        if the user has Quit aura.

.NOTES
    Keep in sync with install.sh.
#>

$ErrorActionPreference = 'Stop'

# ── Constants ─────────────────────────────────────────────────────────────────

$ReleaseBaseUrl   = 'https://github.com/Rfluid/aura/releases'
$InstallDir       = Join-Path $env:LOCALAPPDATA 'Programs\Aura'
# Startup folder — runs aura.exe at sign-in (autostart).
$StartupDir       = [Environment]::GetFolderPath('Startup')
$StartupShortcut  = Join-Path $StartupDir 'Aura.lnk'
# Per-user Start Menu — for discoverability and pin-to-taskbar.
$StartMenuDir     = Join-Path ([Environment]::GetFolderPath('StartMenu')) 'Programs'
$StartMenuShortcut = Join-Path $StartMenuDir 'Aura.lnk'

# ── Detect mode + root ────────────────────────────────────────────────────────
#
# When invoked as `iex (irm ...)`, $MyInvocation.MyCommand.Path is empty —
# we treat that as the release-download path. When invoked from a checkout,
# the script lives at <root>/scripts/install.ps1 and we can build from source.

$ScriptPath = $MyInvocation.MyCommand.Path
if ($ScriptPath) {
    $ScriptDir = Split-Path -Parent $ScriptPath
    $RepoRoot  = Split-Path -Parent $ScriptDir
} else {
    $ScriptDir = $null
    $RepoRoot  = $null
}

$Mode = $env:AURA_INSTALL_MODE
if (-not $Mode) {
    if ($RepoRoot -and (Test-Path (Join-Path $RepoRoot 'Cargo.toml')) -and (Get-Command cargo -ErrorAction SilentlyContinue)) {
        $Mode = 'source'
    } else {
        $Mode = 'release'
    }
}

# ── Helpers ───────────────────────────────────────────────────────────────────

function Get-HostAssetName {
    param([string]$Version)
    $arch = $env:PROCESSOR_ARCHITECTURE
    switch ($arch) {
        'AMD64' { return "aura-$Version-x86_64-pc-windows-msvc" }
        'ARM64' { return "aura-$Version-aarch64-pc-windows-msvc" }
        default { throw "no published release artifact for PROCESSOR_ARCHITECTURE=$arch" }
    }
}

function Resolve-LatestVersion {
    # GitHub's /releases/latest redirects to /releases/tag/<version>; the
    # final URL segment is the tag. -MaximumRedirection 0 + catching the
    # redirect target avoids depending on the API.
    try {
        $r = Invoke-WebRequest -Uri "$ReleaseBaseUrl/latest" `
            -MaximumRedirection 0 -ErrorAction Stop
    } catch {
        $r = $_.Exception.Response
    }
    $loc = if ($r.Headers.Location) { $r.Headers.Location } else { $r.Headers['Location'] }
    if (-not $loc) { throw "could not resolve latest release URL" }
    $locStr = "$loc"
    return ($locStr -split '/')[-1]
}

function Test-Sha256 {
    param([string]$Asset, [string]$Dir)
    $shaFile = Join-Path $Dir "$Asset.sha256"
    $zipPath = Join-Path $Dir "$Asset.zip"
    $expected = (Get-Content $shaFile -Raw).Trim().Split()[0].ToLower()
    $actual   = (Get-FileHash -Algorithm SHA256 $zipPath).Hash.ToLower()
    if ($expected -ne $actual) {
        throw "SHA256 mismatch for $Asset.zip (expected $expected, got $actual)"
    }
}

# ── Stage binaries ────────────────────────────────────────────────────────────

if ($Mode -eq 'source') {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Error "cargo not found. Install Rust from https://rustup.rs"
        exit 1
    }
    Write-Host "▸ Building Aura (release)…"
    Push-Location $RepoRoot
    try {
        cargo build --release --workspace
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    } finally {
        Pop-Location
    }
    $StageDir = Join-Path $RepoRoot 'target\release'
}
else {
    $Version = $env:AURA_VERSION
    if (-not $Version) {
        Write-Host "▸ Resolving latest release…"
        $Version = Resolve-LatestVersion
        if (-not $Version) {
            Write-Error "failed to determine the latest GitHub release version"
            exit 1
        }
    }

    $Asset = Get-HostAssetName -Version $Version
    Write-Host "▸ Installing $Version ($Asset)"

    $DlDir = Join-Path ([IO.Path]::GetTempPath()) ("aura-install-" + [Guid]::NewGuid())
    New-Item -ItemType Directory -Force -Path $DlDir | Out-Null
    try {
        Invoke-WebRequest -Uri "$ReleaseBaseUrl/download/$Version/$Asset.zip" `
            -OutFile (Join-Path $DlDir "$Asset.zip")
        Invoke-WebRequest -Uri "$ReleaseBaseUrl/download/$Version/$Asset.sha256" `
            -OutFile (Join-Path $DlDir "$Asset.sha256")
        Test-Sha256 -Asset $Asset -Dir $DlDir

        Expand-Archive -Path (Join-Path $DlDir "$Asset.zip") -DestinationPath $DlDir -Force
        # Windows zips currently flatten contents (no parent dir); fall back
        # to a nested layout if a future CI change starts wrapping them.
        $StageDir = if (Test-Path (Join-Path $DlDir 'aura.exe')) {
            $DlDir
        } else {
            Join-Path $DlDir $Asset
        }
    } catch {
        Remove-Item -Recurse -Force $DlDir -ErrorAction SilentlyContinue
        throw
    }
    # Defer cleanup until after install.
    $CleanupDir = $DlDir
}

# ── Install binaries ──────────────────────────────────────────────────────────

# Stop any running instance so we can overwrite the .exe.
Get-Process aura -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item -Force (Join-Path $StageDir 'aura.exe')            (Join-Path $InstallDir 'aura.exe')
Copy-Item -Force (Join-Path $StageDir 'aura-plugin-rtk.exe') (Join-Path $InstallDir 'aura-plugin-rtk.exe')
Write-Host "▸ Installed binaries to $InstallDir"

# ── Detect agents and seed/merge config ──────────────────────────────────────
# Runs before autostart so the app picks up the populated config on its first
# launch. Failure is non-fatal — AppConfig::load() writes a default config on
# first launch as a fallback.

Write-Host "▸ Configuring agents…"
& (Join-Path $InstallDir 'aura.exe') setup-config
if ($LASTEXITCODE -ne 0) {
    Write-Warning "'aura setup-config' failed; defaults will be written on first launch"
}

# ── Shortcuts: Startup (autostart) + Start Menu (discoverability) ────────────

function New-AuraShortcut {
    param([string]$Path)
    $wsh = New-Object -ComObject WScript.Shell
    $lnk = $wsh.CreateShortcut($Path)
    $lnk.TargetPath       = Join-Path $InstallDir 'aura.exe'
    $lnk.WorkingDirectory = $InstallDir
    $lnk.WindowStyle      = 7  # Minimized — tray-icon only.
    $lnk.Description      = 'Aura — Agent Usage Reporter'
    $lnk.Save()
}

New-Item -ItemType Directory -Force -Path $StartMenuDir | Out-Null
New-AuraShortcut -Path $StartupShortcut
Write-Host "▸ Installed Startup shortcut to $StartupShortcut"
New-AuraShortcut -Path $StartMenuShortcut
Write-Host "▸ Installed Start Menu shortcut to $StartMenuShortcut"

# Release mode: start aura immediately to mirror systemd `enable --now` /
# launchctl `kickstart`. Source mode users have presumably built and can
# launch via Start Menu when ready.
if ($Mode -eq 'release') {
    Start-Process -WindowStyle Hidden (Join-Path $InstallDir 'aura.exe')
    Write-Host "▸ Aura started — tray icon should appear next to the clock"
}

# ── Cleanup ───────────────────────────────────────────────────────────────────

if ($Mode -eq 'release' -and $CleanupDir) {
    Remove-Item -Recurse -Force $CleanupDir -ErrorAction SilentlyContinue
}

# ── PATH hint ─────────────────────────────────────────────────────────────────

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not ($userPath -split ';' | Where-Object { $_ -eq $InstallDir })) {
    Write-Host ""
    Write-Host "note: $InstallDir is not on your user PATH. Add it with:"
    Write-Host "    [Environment]::SetEnvironmentVariable('Path', `"`$([Environment]::GetEnvironmentVariable('Path', 'User'));$InstallDir`", 'User')"
    Write-Host "Restart your terminal afterwards."
}

Write-Host ""
Write-Host "✔ Aura installed."

# ── Next-step hints ───────────────────────────────────────────────────────────
Write-Host ""
Write-Host "Next steps:"
Write-Host "  → Tray icon: should appear at the right end of the taskbar (near the clock)."
Write-Host "  → If it landed in the '^' overflow group, drag it into the always-visible area."
Write-Host "  → Right-click the tray icon for Show / Quit. Left-click toggles the modal."
Write-Host "  → If Windows SmartScreen blocked the unsigned binary on first run, click 'More info' → 'Run anyway'."
