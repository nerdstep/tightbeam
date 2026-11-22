# Builds the Mac app on the Mac mini from what's on GitHub, and leaves it at
# /Users/Shared/Tightbeam.app.
#
#   .\scripts\build-mac.ps1                 build main
#   .\scripts\build-mac.ps1 -Branch ui-work build another pushed branch

param([string]$Branch = 'main')
$ErrorActionPreference = 'Stop'

# The Mac builds from GitHub, so warn about local commits that haven't been pushed.
$root = Split-Path -Parent $PSScriptRoot
git -C $root fetch --quiet origin $Branch 2>$null
$ahead = git -C $root rev-list --count "origin/$Branch..$Branch" 2>$null
if ($ahead -and [int]$ahead -gt 0) {
    Write-Warning "$ahead local commit(s) on $Branch aren't pushed; the Mac will build without them."
}

ssh mac-mini "bash ~/tightbeam/scripts/mac/build.sh $Branch"
if ($LASTEXITCODE) { throw 'Mac build failed' }
