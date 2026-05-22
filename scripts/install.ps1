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

    Binaries land in %LOCALAPPDATA%\Programs\Aura. Autostart is wired via
    a Startup-folder shortcut so Aura launches at sign-in.

.NOTES
    Keep in sync with install.sh.
#>

$ErrorActionPreference = 'Stop'

# ── Constants ─────────────────────────────────────────────────────────────────

$ReleaseBaseUrl = 'https://github.com/Rfluid/aura/releases'
$InstallDir     = Join-Path $env:LOCALAPPDATA 'Programs\Aura'
$StartupDir     = [Environment]::GetFolderPath('Startup')
$ShortcutPath   = Join-Path $StartupDir 'Aura.lnk'

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

# ── Autostart shortcut ────────────────────────────────────────────────────────

$wsh = New-Object -ComObject WScript.Shell
$lnk = $wsh.CreateShortcut($ShortcutPath)
$lnk.TargetPath       = Join-Path $InstallDir 'aura.exe'
$lnk.WorkingDirectory = $InstallDir
$lnk.WindowStyle      = 7  # Minimized — tray-icon only.
$lnk.Description      = 'Aura — Agent Usage Reporter'
$lnk.Save()
Write-Host "▸ Installed Startup shortcut to $ShortcutPath"

# Release mode: kick off the app immediately to match install.sh behavior
# (systemd `enable --now` / launchctl `kickstart`).
if ($Mode -eq 'release') {
    Start-Process -WindowStyle Hidden (Join-Path $InstallDir 'aura.exe')
    Write-Host "▸ Aura started"
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
