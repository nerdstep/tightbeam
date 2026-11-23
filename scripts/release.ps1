<#
.SYNOPSIS
  Cuts a release: sets the version, commits, tags v<version> and pushes.

.DESCRIPTION
  The pushed tag starts the Release workflow on GitHub, which builds the Windows
  installer and the macOS app into a draft release. Check the draft on GitHub, then
  publish it.

  The version lives only in src-tauri/Cargo.toml (tauri.conf.json falls back to it).
  Run from main with a clean working tree.

.EXAMPLE
  .\scripts\release.ps1 0.2.0
#>
param(
  [Parameter(Mandatory)][string]$Version
)

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

function Invoke-Git {
  git @args
  if ($LASTEXITCODE -ne 0) { throw "git $args failed" }
}

if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Version should look like 1.2.3, not '$Version'." }
$tag = "v$Version"

if ((git branch --show-current) -ne 'main') { throw 'Release from main.' }
if (git status --porcelain) { throw 'Commit or stash your changes first.' }
Invoke-Git fetch --quiet origin
if ([int](git rev-list --count HEAD..origin/main) -gt 0) { throw 'main is behind origin/main. Pull first.' }
git rev-parse --quiet --verify "refs/tags/$tag" > $null
if ($LASTEXITCODE -eq 0) { throw "Tag $tag already exists." }

$manifest = 'src-tauri/Cargo.toml'
$text = [IO.File]::ReadAllText((Resolve-Path $manifest))
$current = [regex]::Match($text, '(?m)^version = "([^"]*)"').Groups[1].Value
if ($current -ne $Version) {
  Write-Host "Version $current -> $Version"
  $text = [regex]::new('(?m)^version = "[^"]*"').Replace($text, "version = `"$Version`"", 1)
  [IO.File]::WriteAllText((Resolve-Path $manifest), $text)
  # Updates Tightbeam's own entry in Cargo.lock; dependencies stay as they are.
  cargo update --workspace --manifest-path $manifest --quiet
  if ($LASTEXITCODE -ne 0) { throw 'cargo update failed' }
  Invoke-Git commit --quiet -m "Release $tag" -- $manifest src-tauri/Cargo.lock
}

Invoke-Git tag -a $tag -m "Tightbeam $Version"
# Both or neither, so a tag never points at a commit GitHub doesn't have.
Invoke-Git push --atomic origin main $tag

$repo = (git remote get-url origin) -replace '\.git$', '' -replace '^git@github\.com:', 'https://github.com/'
Write-Host ""
Write-Host "Pushed $tag. The builds take about 10 minutes:"
Write-Host "  $repo/actions"
Write-Host "Then check and publish the draft:"
Write-Host "  $repo/releases"
