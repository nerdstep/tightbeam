<#
.SYNOPSIS
  Cuts a release: sets the version, dates the changelog, commits, tags v<version>
  and pushes.

.DESCRIPTION
  The pushed tag starts the Release workflow on GitHub, which runs CI on the tagged
  commit and builds the Windows installer and the macOS app into a draft release.
  Check the draft on GitHub, then publish it.

  Before tagging, this checks that main is pushed, that CI passed on it, and that
  CHANGELOG.md has entries under "## Unreleased". The version lives only in
  src-tauri/Cargo.toml (tauri.conf.json falls back to it).

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

# Before anything changes: a missing tool halfway through would leave edited files behind.
foreach ($tool in 'git', 'gh', 'cargo') {
  if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "$tool isn't on PATH." }
}

if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Version should look like 1.2.3, not '$Version'." }
$tag = "v$Version"

# The tree, the tag and the remote.
if ((git branch --show-current) -ne 'main') { throw 'Release from main.' }
if (git status --porcelain) { throw 'Commit or stash your changes first.' }
Invoke-Git fetch --quiet origin
$head = git rev-parse HEAD
if ($head -ne (git rev-parse origin/main)) { throw 'main and origin/main differ. Push (or pull) first, and let CI run.' }
git rev-parse --quiet --verify "refs/tags/$tag" > $null
if ($LASTEXITCODE -eq 0) { throw "Tag $tag already exists." }

# CI on the commit being released. The Release workflow runs it again on the tagged
# commit; this just fails sooner.
$ci = gh run list --workflow ci.yml --commit $head --event push --limit 1 --json status,conclusion,url | ConvertFrom-Json
if (-not $ci) { throw "No CI run for $($head.Substring(0, 7)) yet." }
if ($ci[0].status -ne 'completed') { throw "CI is still running: $($ci[0].url)" }
if ($ci[0].conclusion -ne 'success') { throw "CI $($ci[0].conclusion): $($ci[0].url)" }

# The changelog: "## Unreleased" needs entries, and they move under "## <version> - <date>".
$changelog = 'CHANGELOG.md'
$log = [IO.File]::ReadAllText((Resolve-Path $changelog))
$unreleased = [regex]::Match($log, '(?ms)^## Unreleased[ \t]*\r?\n(.*?)(?=^## |\z)')
if (-not $unreleased.Success) { throw "$changelog has no '## Unreleased' section." }
if ($unreleased.Groups[1].Value.Trim() -eq '') { throw "Nothing under '## Unreleased' in $changelog. Add what changed." }
$heading = "## $Version $([char]0x2014) $(Get-Date -Format 'yyyy-MM-dd')" # an em dash
$log = $log.Remove($unreleased.Index, $unreleased.Length).Insert(
  $unreleased.Index, "## Unreleased`n`n$heading`n`n$($unreleased.Groups[1].Value.Trim())`n`n")
[IO.File]::WriteAllText((Resolve-Path $changelog), $log.TrimEnd() + "`n")

# The version.
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
}

Invoke-Git commit --quiet -m "Release $tag" -- $changelog $manifest src-tauri/Cargo.lock
Invoke-Git tag -a $tag -m "Tightbeam $Version"
# Both or neither, so a tag never points at a commit GitHub doesn't have.
Invoke-Git push --atomic origin main $tag

$repo = (git remote get-url origin) -replace '\.git$', '' -replace '^git@github\.com:', 'https://github.com/'
Write-Host ""
Write-Host "Pushed $tag. CI and the builds take about 15 minutes:"
Write-Host "  $repo/actions"
Write-Host "Then check and publish the draft:"
Write-Host "  $repo/releases"
