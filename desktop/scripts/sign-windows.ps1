# Authenticode signing with graceful degradation (Sprint C C0-2 / C1-6).
# Credentials come from env (see .env.example at repo root; CI uses secrets).
# Without credentials the script prints a loud UNSIGNED warning and exits 0,
# so dry-runs produce a clearly-labeled unsigned artifact instead of either
# silently shipping or hard-failing (C0-2 contract: no silent degradation).
#
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File sign-windows.ps1 -Path <file.exe>
# Env:
#   WINDOWS_CERT_PFX        Base64-encoded PFX (preferred for CI secrets)
#   WINDOWS_CERT_PFX_PATH    Local PFX path (alternative)
#   WINDOWS_CERT_PASSWORD    PFX password
# NOTE: ASCII-only (PowerShell 5.1 GBK pitfall).
param(
    [Parameter(Mandatory = $true)][string]$Path
)

if (-not (Test-Path $Path)) { Write-Error "file not found: $Path"; exit 1 }

if (-not $env:WINDOWS_CERT_PFX -and -not $env:WINDOWS_CERT_PFX_PATH) {
    Write-Warning "==== UNSIGNED BUILD (degraded mode) ===="
    Write-Warning "No Authenticode certificate configured."
    Write-Warning "Set WINDOWS_CERT_PFX (base64) or WINDOWS_CERT_PFX_PATH"
    Write-Warning "plus WINDOWS_CERT_PASSWORD. Until then SmartScreen will"
    Write-Warning "warn about an unknown publisher."
    Write-Host "SIGN_RESULT=unsigned"
    exit 0
}

if (-not (Get-Command signtool -ErrorAction SilentlyContinue)) {
    Write-Error "signtool not found in PATH (install Windows SDK)"
    exit 1
}

$pfxPath = $env:WINDOWS_CERT_PFX_PATH
$cleanup = $null
if ($env:WINDOWS_CERT_PFX) {
    $cleanup = [System.IO.Path]::GetTempFileName() + ".pfx"
    [System.IO.File]::WriteAllBytes($cleanup, [Convert]::FromBase64String($env:WINDOWS_CERT_PFX))
    $pfxPath = $cleanup
}

$stamp = "http://timestamp.digicert.com"
& signtool sign /fd SHA256 /td SHA256 /tr $stamp /f $pfxPath /p $env:WINDOWS_CERT_PASSWORD $Path
$signExit = $LASTEXITCODE
if ($cleanup) { Remove-Item $cleanup -Force }
if ($signExit -ne 0) { Write-Error "signtool sign failed (exit $signExit)"; exit 1 }

& signtool verify /pa $Path
if ($LASTEXITCODE -ne 0) { Write-Error "signtool verify failed"; exit 1 }
Write-Host "SIGN_RESULT=signed"
