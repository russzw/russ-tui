#requires -Version 5.1
<#
.SYNOPSIS
    Builds and runs russ-tui from a source checkout on Windows.

.DESCRIPTION
    Runs cargo build --release from the repository root, then launches the
    release binary from that directory so the bundled data/movies.json catalog
    resolves correctly. State is stored separately in %APPDATA%\russ-tui.

.PARAMETER NoBuild
    Launch the existing release binary without running cargo.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\run.ps1

.EXAMPLE
    .\run.ps1 -NoBuild
#>
[CmdletBinding()]
param(
    [switch]$NoBuild
)

$ErrorActionPreference = "Stop"

$root = $PSScriptRoot
if (-not $root) {
    $root = (Get-Location).Path
}

if (-not (Test-Path -LiteralPath (Join-Path $root "Cargo.toml") -PathType Leaf)) {
    throw "Cargo.toml was not found. Run run.ps1 from the russ-tui repository."
}

function Get-Cargo {
    $cargoHome = $env:CARGO_HOME
    if (-not $cargoHome) {
        $cargoHome = Join-Path $env:USERPROFILE ".cargo"
    }

    $candidate = Join-Path $cargoHome "bin\cargo.exe"
    if (Test-Path -LiteralPath $candidate -PathType Leaf) {
        return $candidate
    }

    $command = Get-Command cargo.exe -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    return $null
}

$cargo = Get-Cargo
if (-not $cargo) {
    throw "Cargo was not found. Run install.ps1, or install Rust from https://rustup.rs."
}

$exe = Join-Path $root "target\release\russ-tui.exe"

Push-Location $root
try {
    if (-not $NoBuild) {
        Write-Host "Building russ-tui (release)..." -ForegroundColor Cyan
        & $cargo build --release --locked
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build --release --locked failed with exit code $LASTEXITCODE."
        }
    }

    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
        throw "The release binary was not found at $exe. Run without -NoBuild first."
    }

    & $exe
    exit $LASTEXITCODE
}
finally {
    Pop-Location
}
