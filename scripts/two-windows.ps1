# Opens two Tightbeam windows on this PC ("alice" and "bob") so you can pair and chat
# with yourself. Each gets its own data folder, and both use loopback, because Windows
# won't send from this PC's LAN address to itself.
#
#   .\scripts\two-windows.ps1          reuse previous test data (stay paired)
#   .\scripts\two-windows.ps1 -Fresh   start both from scratch
#
# Close any running Tightbeam first (including `cargo tauri dev`), or the build fails
# because Windows locks the running .exe.

param([switch]$Fresh)
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
Push-Location (Join-Path $root 'src-tauri')
try {
    cargo build
    if ($LASTEXITCODE) { throw 'Build failed (is Tightbeam still running?)' }
} finally {
    Pop-Location
}

$exe = Join-Path $root 'src-tauri\target\debug\tightbeam.exe'
try {
    foreach ($who in 'alice', 'bob') {
        $dir = Join-Path $env:TEMP "tightbeam-$who"
        if ($Fresh) { Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue }
        $env:TIGHTBEAM_DATA_DIR = $dir
        $env:TIGHTBEAM_LOOPBACK = '1'
        Start-Process $exe
        Write-Host "Started $who (data: $dir)"
    }
} finally {
    Remove-Item Env:TIGHTBEAM_DATA_DIR, Env:TIGHTBEAM_LOOPBACK -ErrorAction SilentlyContinue
}
