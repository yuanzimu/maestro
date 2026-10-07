# Build workspace engine binaries and copy them as Tauri sidecars
# (the "-<triple>.exe" suffix is a hard convention of Tauri externalBin).
# NOTE: keep this file ASCII-only. Windows PowerShell 5.1 reads BOM-less
# UTF-8 scripts as GBK and multibyte comments can swallow newlines.
# Usage: powershell prepare-sidecars.ps1 [-Triple aarch64-pc-windows-msvc] [-WorkspaceTarget C:\cargo-target\maestro] [-DesktopTarget C:\cargo-target\maestro-desktop]
#   -DesktopTarget is kept for caller compatibility but no longer used:
#   mock-cli is a root-workspace crate now (decision 33, dev-only --
#   release bundles drop it via tauri.release.conf.json overlay).

param(
    [string]$Triple = "aarch64-pc-windows-msvc",
    [string]$WorkspaceTarget = "C:\cargo-target\maestro",
    [string]$DesktopTarget = "C:\cargo-target\maestro-desktop"
)

$ErrorActionPreference = "Stop"
$desktop = Split-Path $PSScriptRoot -Parent
$repo = Split-Path $desktop -Parent
$bin = Join-Path $desktop "src-tauri\bin"
New-Item -ItemType Directory -Force $bin | Out-Null

# 1) engine pair + demo worker (all from the root workspace; mock-cli
#    moved out of the desktop crate so release bundles can exclude it)
$env:CARGO_TARGET_DIR = $WorkspaceTarget
Push-Location $repo
cargo build --release -p maestro-daemon -p mock-cli
Pop-Location
Copy-Item (Join-Path $WorkspaceTarget "release\maestro-daemon.exe") (Join-Path $bin "maestro-daemon-$Triple.exe") -Force
Copy-Item (Join-Path $WorkspaceTarget "release\maestro-rounder.exe") (Join-Path $bin "maestro-rounder-$Triple.exe") -Force
Copy-Item (Join-Path $WorkspaceTarget "release\mock-cli.exe") (Join-Path $bin "mock-cli-$Triple.exe") -Force

Write-Host "sidecars ready:"
Get-ChildItem $bin | Select-Object Name, Length | Format-Table -AutoSize
