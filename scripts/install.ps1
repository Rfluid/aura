<#
.SYNOPSIS
    Install Aura on Windows.

.DESCRIPTION
    Two modes (mirror of install.sh):

      * source   -- run from a cloned repo: build with cargo, then install.
      * release  -- run via `iex (irm ...)`: download the latest GitHub
                   release zip for the host, verify its checksum, install.

    Auto-detects the mode (source when run from a checkout with cargo on
    PATH, release otherwise). Override with $env:AURA_INSTALL_MODE.
    Pin a version with $env:AURA_VERSION = 'v1.2.3'.

    Source mode automatically installs missing build prerequisites via winget:
      - Visual Studio 2022 Build Tools (MSVC linker + headers)
      - ARM64 MSVC component (on ARM64 hosts, added via setup.exe modify)
      - LLVM/clang (ARM64 only; required by the ring crate for assembly)

    Aura is a tray-indicator app: the icon next to the clock is the
    entire UI, so the process needs to be running at sign-in for there
    to be anything to click. Binaries land in
    %LOCALAPPDATA%\Programs\Aura. The installer creates both:

      * a Startup-folder shortcut -- runs aura.exe minimised at sign-in
        (autostart);
      * a Start Menu shortcut -- for discoverability and a manual launch
        if the user has Quit aura.

.NOTES
    Keep in sync with install.sh.
    ASCII-only source: avoids PowerShell 5.1 UTF-8-without-BOM parse errors.
#>

$ErrorActionPreference = 'Stop'

# ---- Constants ---------------------------------------------------------------

$ReleaseBaseUrl    = 'https://github.com/Rfluid/aura/releases'
$InstallDir        = Join-Path $env:LOCALAPPDATA 'Programs\Aura'
$StartupDir        = [Environment]::GetFolderPath('Startup')
$StartupShortcut   = Join-Path $StartupDir 'Aura.lnk'
$StartMenuDir      = Join-Path ([Environment]::GetFolderPath('StartMenu')) 'Programs'
$StartMenuShortcut = Join-Path $StartMenuDir 'Aura.lnk'

# ---- Mode + root detection ---------------------------------------------------
#
# When invoked as `iex (irm ...)`, $MyInvocation.MyCommand.Path is empty --
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

# ---- Helpers (release mode) --------------------------------------------------

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
    $shaFile  = Join-Path $Dir "$Asset.sha256"
    $zipPath  = Join-Path $Dir "$Asset.zip"
    $expected = (Get-Content $shaFile -Raw).Trim().Split()[0].ToLower()
    $actual   = (Get-FileHash -Algorithm SHA256 $zipPath).Hash.ToLower()
    if ($expected -ne $actual) {
        throw "SHA256 mismatch for $Asset.zip (expected $expected, got $actual)"
    }
}

# ---- Build prerequisites (source mode only) ----------------------------------

function Find-VSInstallPath {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) { return $null }
    $path = & $vswhere -latest -products '*' -property installationPath 2>$null
    if ($path) { return $path.Trim() } else { return $null }
}

function Find-MSVCToolsVersion {
    param([string]$VSPath)
    $dir = Join-Path $VSPath 'VC\Tools\MSVC'
    if (-not (Test-Path $dir)) { return $null }
    $entry = Get-ChildItem $dir -ErrorAction SilentlyContinue |
             Sort-Object Name -Descending |
             Select-Object -First 1
    if ($entry) { return $entry.Name } else { return $null }
}

function Find-WindowsSDK {
    $root = $null
    try {
        $root = (Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots' `
                     -ErrorAction Stop).KitsRoot10
    } catch {}
    if (-not $root) {
        try {
            $root = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots' `
                         -ErrorAction Stop).KitsRoot10
        } catch {}
    }
    if (-not $root) { return $null }
    $root = $root.TrimEnd('\')
    $ver  = (Get-ChildItem "$root\include" -ErrorAction SilentlyContinue |
             Sort-Object Name -Descending |
             Select-Object -First 1).Name
    if (-not $ver) { return $null }
    return @{ Root = $root; Version = $ver }
}

function Set-MSVCBuildEnvironment {
    param([string]$VSPath, [string]$MSVCVer, [string]$SDKRoot, [string]$SDKVer)
    # Select tools matching the host architecture so build scripts that run
    # on the host are linked by a compatible linker.
    $procArch  = $env:PROCESSOR_ARCHITECTURE   # AMD64 or ARM64
    $hostDir   = if ($procArch -eq 'ARM64') { 'Hostarm64' } else { 'Hostx64' }
    $targetDir = if ($procArch -eq 'ARM64') { 'arm64'     } else { 'x64'     }
    $msvcBin   = "$VSPath\VC\Tools\MSVC\$MSVCVer\bin\$hostDir\$targetDir"
    $sdkBin    = "$SDKRoot\bin\$SDKVer\$targetDir"
    $env:PATH    = "$msvcBin;$sdkBin;$env:PATH"
    $env:LIB     = "$VSPath\VC\Tools\MSVC\$MSVCVer\lib\$targetDir;" +
                   "$SDKRoot\lib\$SDKVer\um\$targetDir;" +
                   "$SDKRoot\lib\$SDKVer\ucrt\$targetDir"
    $env:INCLUDE = "$VSPath\VC\Tools\MSVC\$MSVCVer\include;" +
                   "$SDKRoot\include\$SDKVer\ucrt;" +
                   "$SDKRoot\include\$SDKVer\um;" +
                   "$SDKRoot\include\$SDKVer\shared"
}

function Install-VCBuildTools {
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
        throw "winget not found. Install Visual Studio Build Tools manually: https://visualstudio.microsoft.com/downloads/"
    }
    Write-Host "> Installing Visual Studio Build Tools (this may take several minutes)..."
    # Base VCTools workload only. The ARM64 target component is added
    # separately via setup.exe modify because winget-bootstrapped installs
    # do not reliably propagate the ARM64 payload.
    winget install Microsoft.VisualStudio.2022.BuildTools `
        --accept-source-agreements --accept-package-agreements `
        --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --norestart"
    if ($LASTEXITCODE -ne 0) { throw "VS Build Tools installation failed (exit $LASTEXITCODE)" }
}

function Add-VSARM64Component {
    param([string]$VSInstallPath)
    # Use the VS Installer directly (setup.exe modify) rather than winget --force,
    # because winget re-runs the bootstrapper which does not reliably deliver
    # the arm64 target payload on ARM64 hosts.
    $setup = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\setup.exe"
    if (-not (Test-Path $setup)) {
        throw "VS Installer not found at $setup -- cannot add ARM64 component"
    }
    Write-Host "> Adding ARM64 MSVC build tools (VC.Tools.ARM64)..."
    & $setup modify `
        --installPath $VSInstallPath `
        --add Microsoft.VisualStudio.Component.VC.Tools.ARM64 `
        --quiet --norestart
    if ($LASTEXITCODE -ne 0) { throw "Failed to add ARM64 MSVC component (exit $LASTEXITCODE)" }
}

function Install-LLVMClang {
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
        throw "winget not found. Install LLVM manually from https://releases.llvm.org/"
    }
    Write-Host "> Installing LLVM/clang (required by the ring crate on ARM64)..."
    winget install LLVM.LLVM --accept-source-agreements --accept-package-agreements
    if ($LASTEXITCODE -ne 0) { throw "LLVM installation failed (exit $LASTEXITCODE)" }
}

function Invoke-BuildPrerequisites {
    $isARM64 = ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64')

    # 1. Ensure VS Build Tools (MSVC linker + headers) is installed.
    $vsPath  = Find-VSInstallPath
    $msvcVer = if ($vsPath) { Find-MSVCToolsVersion -VSPath $vsPath } else { $null }
    if (-not $vsPath -or -not $msvcVer) {
        Install-VCBuildTools
        $vsPath  = Find-VSInstallPath
        $msvcVer = if ($vsPath) { Find-MSVCToolsVersion -VSPath $vsPath } else { $null }
        if (-not $vsPath -or -not $msvcVer) {
            throw "VS Build Tools not found after installation. Please install manually."
        }
    }

    # 2. On ARM64: ensure the arm64 target linker is present.
    #    The base VCTools workload only ships x64/x86 targets; the ARM64
    #    target (Hostarm64\arm64\link.exe + lib\arm64\) must be added via
    #    setup.exe modify rather than winget --force.
    if ($isARM64) {
        $arm64Link = "$vsPath\VC\Tools\MSVC\$msvcVer\bin\Hostarm64\arm64\link.exe"
        if (-not (Test-Path $arm64Link)) {
            Add-VSARM64Component -VSInstallPath $vsPath
            $msvcVer   = Find-MSVCToolsVersion -VSPath $vsPath
            $arm64Link = "$vsPath\VC\Tools\MSVC\$msvcVer\bin\Hostarm64\arm64\link.exe"
            if (-not (Test-Path $arm64Link)) {
                throw "ARM64 linker not found after adding component. Please install manually."
            }
        }
    }

    # 3. On ARM64: ensure clang is available.
    #    The ring crate compiles architecture-specific assembly using clang;
    #    MSVC cl.exe is not accepted for this step.
    if ($isARM64) {
        $llvmBin  = 'C:\Program Files\LLVM\bin'
        $clangExe = Join-Path $llvmBin 'clang.exe'
        if (-not (Test-Path $clangExe) -and -not (Get-Command clang -ErrorAction SilentlyContinue)) {
            Install-LLVMClang
        }
        # Prepend LLVM bin to PATH for this process regardless of how clang was installed.
        if (Test-Path $llvmBin) { $env:PATH = "$llvmBin;$env:PATH" }
    }

    # 4. Configure PATH / LIB / INCLUDE for the MSVC build environment.
    $sdk = Find-WindowsSDK
    if (-not $sdk) {
        throw "Windows SDK not found. Run Windows Update or install from https://developer.microsoft.com/windows/downloads/windows-sdk/"
    }
    Set-MSVCBuildEnvironment `
        -VSPath   $vsPath `
        -MSVCVer  $msvcVer `
        -SDKRoot  $sdk.Root `
        -SDKVer   $sdk.Version
    Write-Host "> Build environment ready (MSVC $msvcVer, SDK $($sdk.Version))"
}

# ---- Stage binaries ----------------------------------------------------------

if ($Mode -eq 'source') {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Error "cargo not found. Install Rust from https://rustup.rs"
        exit 1
    }

    Invoke-BuildPrerequisites

    Write-Host "> Building Aura (release)..."
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
        Write-Host "> Resolving latest release..."
        $Version = Resolve-LatestVersion
        if (-not $Version) {
            Write-Error "failed to determine the latest GitHub release version"
            exit 1
        }
    }

    $Asset = Get-HostAssetName -Version $Version
    Write-Host "> Installing $Version ($Asset)"

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

# ---- Install binaries --------------------------------------------------------

# Stop any running instance so we can overwrite the .exe.
Get-Process aura -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item -Force (Join-Path $StageDir 'aura.exe')            (Join-Path $InstallDir 'aura.exe')
Copy-Item -Force (Join-Path $StageDir 'aura-plugin-rtk.exe') (Join-Path $InstallDir 'aura-plugin-rtk.exe')
Write-Host "> Installed binaries to $InstallDir"

# Brand icon for Start Menu / Startup shortcuts. Source mode picks it up from
# the repo; release archives ship it alongside the .exe (see release.yml).
$IconStagePath = if ($Mode -eq 'source') {
    Join-Path $RepoRoot 'packaging\windows\aura.ico'
} else {
    Join-Path $StageDir 'aura.ico'
}
$InstalledIcon = Join-Path $InstallDir 'aura.ico'
if (Test-Path $IconStagePath) {
    Copy-Item -Force $IconStagePath $InstalledIcon
} else {
    Write-Warning "aura.ico not found at $IconStagePath; shortcuts will use the generic icon"
    $InstalledIcon = $null
}

# ---- Detect agents and seed/merge config -------------------------------------
# Runs before autostart so the app picks up the populated config on its first
# launch. Failure is non-fatal -- AppConfig::load() writes a default config on
# first launch as a fallback.

Write-Host "> Configuring agents..."
& (Join-Path $InstallDir 'aura.exe') setup-config
if ($LASTEXITCODE -ne 0) {
    Write-Warning "'aura setup-config' failed; defaults will be written on first launch"
}

# ---- Shortcuts: Startup (autostart) + Start Menu (discoverability) ----------

function New-AuraShortcut {
    param([string]$Path)
    $wsh = New-Object -ComObject WScript.Shell
    $lnk = $wsh.CreateShortcut($Path)
    $lnk.TargetPath       = Join-Path $InstallDir 'aura.exe'
    $lnk.WorkingDirectory = $InstallDir
    $lnk.WindowStyle      = 7  # Minimized - tray-icon only.
    $lnk.Description      = 'Aura - Agent Usage Reporter'
    if ($InstalledIcon) {
        $lnk.IconLocation = "$InstalledIcon,0"
    }
    $lnk.Save()
}

New-Item -ItemType Directory -Force -Path $StartMenuDir | Out-Null
New-AuraShortcut -Path $StartupShortcut
Write-Host "> Installed Startup shortcut to $StartupShortcut"
New-AuraShortcut -Path $StartMenuShortcut
Write-Host "> Installed Start Menu shortcut to $StartMenuShortcut"

# Release mode: start aura immediately to mirror systemd `enable --now` /
# launchctl `kickstart`. Source mode users have presumably built and can
# launch via Start Menu when ready.
if ($Mode -eq 'release') {
    Start-Process -WindowStyle Hidden (Join-Path $InstallDir 'aura.exe')
    Write-Host "> Aura started - tray icon should appear next to the clock"
}

# ---- Cleanup -----------------------------------------------------------------

if ($Mode -eq 'release' -and $CleanupDir) {
    Remove-Item -Recurse -Force $CleanupDir -ErrorAction SilentlyContinue
}

# ---- PATH hint ---------------------------------------------------------------

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not ($userPath -split ';' | Where-Object { $_ -eq $InstallDir })) {
    Write-Host ""
    Write-Host "note: $InstallDir is not on your user PATH. Add it with:"
    Write-Host "    [Environment]::SetEnvironmentVariable('Path', `"`$([Environment]::GetEnvironmentVariable('Path', 'User'));$InstallDir`", 'User')"
    Write-Host "Restart your terminal afterwards."
}

Write-Host ""
Write-Host "Aura installed."

# ---- Next-step hints ---------------------------------------------------------
Write-Host ""
Write-Host "Next steps:"
Write-Host "  > Tray icon: should appear at the right end of the taskbar (near the clock)."
Write-Host "  > If it landed in the '^' overflow group, drag it into the always-visible area."
Write-Host "  > Right-click the tray icon for Show / Quit. Left-click toggles the modal."
Write-Host "  > If Windows SmartScreen blocked the unsigned binary on first run, click 'More info' -> 'Run anyway'."
