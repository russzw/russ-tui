#requires -Version 5.1
<#
.SYNOPSIS
    Installs russ-tui and the tools needed to build and play it on Windows.

.DESCRIPTION
    The installer can run from a cloned repository or directly from the web:
        irm https://raw.githubusercontent.com/russzw/russ-tui/main/install.ps1 | iex

    It checks for Rust and mpv, installs missing dependencies with winget (or
    rustup-init for Rust), builds a release binary, installs it under
    %LOCALAPPDATA%\russ-tui, and creates .cmd/.ps1 launchers.

    No provider URLs or media files are added by this script. State stays in
    %APPDATA%\russ-tui\state.json and is not touched by the installer.

.PARAMETER InstallDir
    Install directory. Defaults to %LOCALAPPDATA%\russ-tui.

.PARAMETER SkipRust
    Do not install Rust. The script still requires cargo to be available.

.PARAMETER SkipMpv
    Do not install, detect, or configure mpv. Use this if mpv is already
    configured through RUSS_TUI_MPV or PATH.

.PARAMETER SkipBuildTools
    Do not install Visual Studio C++ Build Tools. The build may fail if the
    MSVC toolchain is active and the C++ workload is missing.

.PARAMETER NoPath
    Do not add the install directory to the current user's PATH.

.PARAMETER Launch
    Start russ-tui after installation.

.PARAMETER SourceDir
    Build from this source checkout instead of the current directory or the
    GitHub source archive.

.PARAMETER MpvPath
    Use this mpv.exe instead of discovering or installing mpv.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\install.ps1 -Launch

.EXAMPLE
    .\install.ps1 -InstallDir "$env:TEMP\russ-tui-test" -SkipMpv -NoPath
#>
[CmdletBinding()]
param(
    [string]$InstallDir = "",
    [switch]$SkipRust,
    [switch]$SkipMpv,
    [switch]$SkipBuildTools,
    [switch]$NoPath,
    [switch]$Launch,
    [string]$SourceDir = "",
    [string]$MpvPath = ""
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$RepoUrl = "https://github.com/russzw/russ-tui"
$ArchiveUrl = "$RepoUrl/archive/refs/heads/main.zip"
$script:DownloadTemp = $null

function Write-Step {
    param([string]$Message)
    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Write-Ok {
    param([string]$Message)
    Write-Host "    $Message" -ForegroundColor Green
}

function Write-Warn {
    param([string]$Message)
    Write-Host "    $Message" -ForegroundColor Yellow
}

function Test-ProjectSource {
    param([string]$Path)

    if (-not $Path) {
        return $false
    }

    $cargoToml = Join-Path $Path "Cargo.toml"
    $mainRs = Join-Path $Path "src\main.rs"
    if (-not (Test-Path -LiteralPath $cargoToml -PathType Leaf)) {
        return $false
    }
    if (-not (Test-Path -LiteralPath $mainRs -PathType Leaf)) {
        return $false
    }

    $content = Get-Content -LiteralPath $cargoToml -Raw
    return ($content -match '(?m)^\s*name\s*=\s*"russ-tui"\s*$')
}

function Resolve-ProjectSource {
    param([string]$RequestedSource)

    if ($RequestedSource) {
        $full = [IO.Path]::GetFullPath($RequestedSource)
        if (-not (Test-ProjectSource $full)) {
            throw "SourceDir does not look like a russ-tui checkout: $full"
        }
        return $full
    }

    $candidates = @()
    if ($PSScriptRoot) {
        $candidates += $PSScriptRoot
    }
    $candidates += (Get-Location).Path

    foreach ($candidate in $candidates) {
        if (Test-ProjectSource $candidate) {
            return [IO.Path]::GetFullPath($candidate)
        }
    }

    Write-Step "Downloading the russ-tui source"
    $tempBase = $env:TEMP
    if (-not $tempBase) {
        $tempBase = [IO.Path]::GetTempPath()
    }
    $temp = Join-Path $tempBase ("russ-tui-install-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $temp -Force | Out-Null
    $script:DownloadTemp = $temp

    $zip = Join-Path $temp "russ-tui-main.zip"
    try {
        Invoke-WebRequest -Uri $ArchiveUrl -OutFile $zip -UseBasicParsing
        Expand-Archive -LiteralPath $zip -DestinationPath $temp -Force
    }
    catch {
        throw "Could not download the source archive from $ArchiveUrl. Check your connection and try again. Details: $($_.Exception.Message)"
    }

    $downloaded = Get-ChildItem -LiteralPath $temp -Directory |
        Where-Object { Test-ProjectSource $_.FullName } |
        Select-Object -First 1

    if (-not $downloaded) {
        throw "The downloaded archive did not contain a russ-tui source tree."
    }

    return $downloaded.FullName
}

function Add-DirectoryToProcessPath {
    param([string]$Directory)

    $parts = @($env:Path -split ";" | Where-Object { $_ })
    foreach ($part in $parts) {
        if ($part.TrimEnd("\") -ieq $Directory.TrimEnd("\")) {
            return
        }
    }
    $env:Path = ($parts + $Directory) -join ";"
}

function Add-DirectoryToUserPath {
    param([string]$Directory)

    $normalized = $Directory.TrimEnd("\")
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($null -eq $userPath) {
        $userPath = ""
    }

    $parts = @($userPath -split ";" | Where-Object { $_ -and $_.Trim() })
    foreach ($part in $parts) {
        if ($part.TrimEnd("\") -ieq $normalized) {
            Write-Ok "Install directory is already on your user PATH."
            Add-DirectoryToProcessPath $normalized
            return
        }
    }

    $newPath = (($parts + $normalized) -join ";")
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    Add-DirectoryToProcessPath $normalized
    Write-Ok "Added the install directory to your user PATH (new terminals only)."
}

function Get-CargoPath {
    $cargoHome = $env:CARGO_HOME
    if (-not $cargoHome) {
        $cargoHome = Join-Path $env:USERPROFILE ".cargo"
    }
    $cargoBin = Join-Path $cargoHome "bin"

    Add-DirectoryToProcessPath $cargoBin

    $candidate = Join-Path $cargoBin "cargo.exe"
    if (Test-Path -LiteralPath $candidate -PathType Leaf) {
        return $candidate
    }

    $command = Get-Command cargo.exe -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    return $null
}

function Get-RustupPath {
    $cargoHome = $env:CARGO_HOME
    if (-not $cargoHome) {
        $cargoHome = Join-Path $env:USERPROFILE ".cargo"
    }
    return (Join-Path $cargoHome "bin\rustup.exe")
}

function Install-WingetPackage {
    param(
        [string]$Id,
        [string[]]$AdditionalArgs = @(),
        [switch]$NoSilent
    )

    if (-not (Get-Command winget.exe -ErrorAction SilentlyContinue)) {
        return $false
    }

    $wingetArgs = @(
        "install",
        "--id", $Id,
        "-e",
        "--accept-package-agreements",
        "--accept-source-agreements"
    )
    if (-not $NoSilent) {
        $wingetArgs += "--silent"
    }
    $wingetArgs += $AdditionalArgs

    Write-Host "    winget install $Id"
    & winget.exe @wingetArgs
    if ($LASTEXITCODE -eq 0) {
        return $true
    }

    Write-Warn "winget returned exit code $LASTEXITCODE for $Id."
    return $false
}

function Ensure-Rust {
    param([switch]$SkipInstall)

    Write-Step "Checking Rust"
    $cargo = Get-CargoPath
    if ($cargo) {
        Write-Ok "Found cargo: $cargo"
        return $cargo
    }

    if ($SkipInstall) {
        throw "Rust is not installed, and -SkipRust was supplied. Install Rust from https://rustup.rs and try again."
    }

    Write-Warn "Rust was not found. Installing the stable toolchain."
    $installed = Install-WingetPackage -Id "Rustlang.Rustup"
    if (-not $installed) {
        Write-Warn "winget could not install Rust; using the official rustup-init fallback."
    }

    $rustup = Get-RustupPath
    if (Test-Path -LiteralPath $rustup -PathType Leaf) {
        & $rustup toolchain install stable --profile minimal
        if ($LASTEXITCODE -eq 0) {
            & $rustup default stable
        }
    }

    $cargo = Get-CargoPath
    if (-not $cargo) {
        Write-Warn "Rustup is not available yet; downloading rustup-init."
        $tempBase = $env:TEMP
        if (-not $tempBase) {
            $tempBase = [IO.Path]::GetTempPath()
        }
        $rustupInit = Join-Path $tempBase ("rustup-init-" + [guid]::NewGuid().ToString("N") + ".exe")
        try {
            Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $rustupInit -UseBasicParsing
            & $rustupInit -y --default-toolchain stable --profile minimal
            if ($LASTEXITCODE -ne 0) {
                throw "rustup-init exited with code $LASTEXITCODE"
            }
        }
        finally {
            Remove-Item -LiteralPath $rustupInit -Force -ErrorAction SilentlyContinue
        }
        $cargo = Get-CargoPath
    }

    if (-not $cargo) {
        throw "Rust installation did not produce cargo.exe. Install Rust manually from https://rustup.rs and rerun this script."
    }

    Write-Ok "Rust is ready: $cargo"
    return $cargo
}

function Get-RustHost {
    param([string]$CargoPath)

    $cargoDir = Split-Path -Parent $CargoPath
    $rustc = Join-Path $cargoDir "rustc.exe"
    if (-not (Test-Path -LiteralPath $rustc -PathType Leaf)) {
        $command = Get-Command rustc.exe -ErrorAction SilentlyContinue
        if ($command) {
            $rustc = $command.Source
        }
    }
    if (-not (Test-Path -LiteralPath $rustc -PathType Leaf)) {
        return ""
    }

    $version = & $rustc -vV 2>$null
    foreach ($line in $version) {
        if ($line -match '^host:\s*(.+)$') {
            return $Matches[1].Trim()
        }
    }
    return ""
}

function Test-VcBuildTools {
    $cl = Get-Command cl.exe -ErrorAction SilentlyContinue
    if ($cl) {
        return $true
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path -LiteralPath $vswhere -PathType Leaf) {
        $found = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
        if ($LASTEXITCODE -eq 0 -and $found) {
            return $true
        }
    }

    return $false
}

function Ensure-BuildTools {
    param(
        [string]$CargoPath,
        [switch]$SkipInstall
    )

    Write-Step "Checking the C++ build toolchain"
    $hostTriple = Get-RustHost $CargoPath
    if ($hostTriple) {
        Write-Ok "Rust host: $hostTriple"
    }
    else {
        Write-Warn "Could not detect the Rust host; assuming the Windows MSVC target."
    }

    $needsMsvc = (-not $hostTriple) -or ($hostTriple -match "msvc")
    if (-not $needsMsvc) {
        Write-Ok "The active Rust toolchain does not require Visual Studio C++ Build Tools."
        return
    }

    if (Test-VcBuildTools) {
        Write-Ok "Visual Studio C++ build tools are available."
        return
    }

    if ($SkipInstall) {
        Write-Warn "MSVC build tools were not found, and -SkipBuildTools was supplied. The build may fail."
        return
    }

    Write-Warn "Visual Studio C++ Build Tools were not found. Installing them; this can take several minutes."
    $override = "--wait --quiet --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
    $installed = Install-WingetPackage -Id "Microsoft.VisualStudio.2022.BuildTools" -AdditionalArgs @("--override", $override) -NoSilent
    if (-not $installed) {
        throw "Could not install Visual Studio C++ Build Tools. Install the 'Desktop development with C++' workload from https://visualstudio.microsoft.com/visual-cpp-build-tools/ and rerun this script."
    }

    if (-not (Test-VcBuildTools)) {
        throw "The build tools installer finished, but the C++ workload is still missing. Open the Visual Studio Installer and add 'Desktop development with C++'."
    }

    Write-Ok "Visual Studio C++ build tools are ready."
}

function Find-Mpv {
    if ($env:RUSS_TUI_MPV) {
        $configured = $env:RUSS_TUI_MPV.Trim()
        if ($configured -and (Test-Path -LiteralPath $configured -PathType Leaf)) {
            $resolved = (Resolve-Path -LiteralPath $configured).Path
            if ($resolved -like "*.com") {
                $exe = [IO.Path]::ChangeExtension($resolved, ".exe")
                if (Test-Path -LiteralPath $exe -PathType Leaf) {
                    return $exe
                }
            }
            return $resolved
        }
    }

    $command = Get-Command mpv.exe -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $candidates = @(
        (Join-Path $env:ProgramFiles "MPV Player\mpv.exe"),
        (Join-Path ${env:ProgramFiles(x86)} "MPV Player\mpv.exe"),
        (Join-Path $env:LOCALAPPDATA "Programs\mpv\mpv.exe"),
        (Join-Path $env:LOCALAPPDATA "Microsoft\WinGet\Links\mpv.exe")
    )
    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            return $candidate
        }
    }

    $wingetPackages = Join-Path $env:LOCALAPPDATA "Microsoft\WinGet\Packages"
    if (Test-Path -LiteralPath $wingetPackages -PathType Container) {
        $found = Get-ChildItem -LiteralPath $wingetPackages -Filter "mpv.exe" -File -Recurse -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($found) {
            return $found.FullName
        }
    }

    return $null
}

function Ensure-Mpv {
    param(
        [string]$RequestedPath,
        [switch]$SkipInstall
    )

    if ($SkipInstall) {
        Write-Step "Skipping mpv setup"
        Write-Warn "Make sure mpv is on PATH or RUSS_TUI_MPV points to mpv.exe before playing a stream."
        return $null
    }

    Write-Step "Checking mpv"
    if ($RequestedPath) {
        if (-not (Test-Path -LiteralPath $RequestedPath -PathType Leaf)) {
            throw "MpvPath does not exist: $RequestedPath"
        }
        $resolved = (Resolve-Path -LiteralPath $RequestedPath).Path
        Write-Ok "Using mpv: $resolved"
        return $resolved
    }

    $mpv = Find-Mpv
    if ($mpv) {
        Write-Ok "Found mpv: $mpv"
        return $mpv
    }

    Write-Warn "mpv was not found. Installing the shinchiro Windows build."
    $installed = Install-WingetPackage -Id "shinchiro.mpv"
    if ($installed) {
        $mpv = Find-Mpv
    }

    if (-not $mpv) {
        Write-Warn "mpv could not be installed automatically."
        Write-Warn "Install mpv from https://mpv.io/installation/ and set RUSS_TUI_MPV to its full path."
        return $null
    }

    Write-Ok "mpv is ready: $mpv"
    return $mpv
}

function Copy-InstallFiles {
    param(
        [string]$Source,
        [string]$Destination
    )

    Write-Step "Installing russ-tui to $Destination"
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null

    $sourceExe = Join-Path $Source "target\release\russ-tui.exe"
    if (-not (Test-Path -LiteralPath $sourceExe -PathType Leaf)) {
        throw "The release build did not produce $sourceExe."
    }

    $destinationExe = Join-Path $Destination "russ-tui.exe"
    try {
        Copy-Item -LiteralPath $sourceExe -Destination $destinationExe -Force
    }
    catch {
        throw "Could not copy russ-tui.exe. Close any running russ-tui instance and rerun the installer. Details: $($_.Exception.Message)"
    }

    $sourceData = Join-Path $Source "data"
    if (-not (Test-Path -LiteralPath $sourceData -PathType Container)) {
        throw "The source tree is missing the bundled data directory."
    }
    $destinationData = Join-Path $Destination "data"
    if (Test-Path -LiteralPath $destinationData) {
        Remove-Item -LiteralPath $destinationData -Recurse -Force
    }
    Copy-Item -LiteralPath $sourceData -Destination $Destination -Recurse -Force

    foreach ($file in @("README.md", "LICENSE")) {
        $sourceFile = Join-Path $Source $file
        if (Test-Path -LiteralPath $sourceFile -PathType Leaf) {
            Copy-Item -LiteralPath $sourceFile -Destination (Join-Path $Destination $file) -Force
        }
    }

    $sourceDocs = Join-Path $Source "docs"
    if (Test-Path -LiteralPath $sourceDocs -PathType Container) {
        $destinationDocs = Join-Path $Destination "docs"
        if (Test-Path -LiteralPath $destinationDocs) {
            Remove-Item -LiteralPath $destinationDocs -Recurse -Force
        }
        Copy-Item -LiteralPath $sourceDocs -Destination $Destination -Recurse -Force
    }

    $launcherCmd = @'
@echo off
setlocal
cd /d "%~dp0"
"%~dp0russ-tui.exe" %*
set "code=%ERRORLEVEL%"
if not "%code%"=="0" pause
exit /b %code%
'@
    Set-Content -LiteralPath (Join-Path $Destination "russ-tui.cmd") -Value $launcherCmd -Encoding ASCII

    $launcherPs1 = @'
$ErrorActionPreference = "Stop"
Set-Location -LiteralPath $PSScriptRoot
& (Join-Path $PSScriptRoot "russ-tui.exe")
exit $LASTEXITCODE
'@
    Set-Content -LiteralPath (Join-Path $Destination "russ-tui.ps1") -Value $launcherPs1 -Encoding ASCII

    Write-Ok "Installed the app, bundled catalog, docs, and launchers."
}

try {
    if (-not $InstallDir) {
        if (-not $env:LOCALAPPDATA) {
            throw "LOCALAPPDATA is not set. Supply -InstallDir explicitly."
        }
        $InstallDir = Join-Path $env:LOCALAPPDATA "russ-tui"
    }
    $InstallDir = [IO.Path]::GetFullPath($InstallDir)

    if ($SkipMpv -and $MpvPath) {
        throw "Use either -SkipMpv or -MpvPath, not both."
    }

    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    }
    catch {
        # Older PowerShell versions may not expose Tls12; Invoke-WebRequest will report any real failure.
    }

    Write-Host ""
    Write-Host "russ-tui Windows installer" -ForegroundColor Magenta
    Write-Host "Repository: $RepoUrl"

    $source = Resolve-ProjectSource $SourceDir
    Write-Ok "Source: $source"

    $cargo = Ensure-Rust -SkipInstall:$SkipRust
    Ensure-BuildTools -CargoPath $cargo -SkipInstall:$SkipBuildTools
    $mpv = Ensure-Mpv -RequestedPath $MpvPath -SkipInstall:$SkipMpv

    Write-Step "Building the release binary"
    Push-Location $source
    try {
        & $cargo build --release --locked
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build --release --locked failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }
    Write-Ok "Release build complete."

    Copy-InstallFiles -Source $source -Destination $InstallDir

    if ($mpv) {
        Write-Step "Configuring mpv"
        [Environment]::SetEnvironmentVariable("RUSS_TUI_MPV", $mpv, "User")
        $env:RUSS_TUI_MPV = $mpv
        Write-Ok "Set RUSS_TUI_MPV for your user account."
    }

    if (-not $NoPath) {
        Write-Step "Updating your user PATH"
        Add-DirectoryToUserPath $InstallDir
    }

    $installedExe = Join-Path $InstallDir "russ-tui.exe"
    $launcher = Join-Path $InstallDir "russ-tui.cmd"

    Write-Host ""
    Write-Host "russ-tui is installed." -ForegroundColor Green
    Write-Host "  App:      $installedExe"
    Write-Host "  Launcher: $launcher"
    if ($mpv) {
        Write-Host "  mpv:      $mpv"
    }
    if ($NoPath) {
        Write-Host "  PATH:     not changed (-NoPath)"
    }
    else {
        Write-Host "  PATH:     $InstallDir (new terminals)"
    }

    if ($Launch) {
        Write-Step "Starting russ-tui"
        Push-Location $InstallDir
        try {
            & $installedExe
            if ($LASTEXITCODE -ne 0) {
                Write-Warn "russ-tui exited with code $LASTEXITCODE."
            }
        }
        finally {
            Pop-Location
        }
    }
    else {
        Write-Host ""
        Write-Host "Run it with:" -ForegroundColor Cyan
        Write-Host "  $launcher"
        Write-Host "Or rerun this installer with -Launch."
    }
}
finally {
    if ($script:DownloadTemp) {
        Remove-Item -LiteralPath $script:DownloadTemp -Recurse -Force -ErrorAction SilentlyContinue
    }
}
