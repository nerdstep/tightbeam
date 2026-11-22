#!/usr/bin/env bash
# Builds a universal Tightbeam.app and puts it in /Users/Shared.
# Run from Windows with scripts/build-mac.ps1, or here with: bash scripts/mac/build.sh [branch]
set -euo pipefail

branch="${1:-main}"
# shellcheck disable=SC1091
source "$HOME/.cargo/env"
export PATH="$HOME/.bun/bin:$PATH"
cd "$(dirname "$0")/../.."

# This checkout only ever builds, so match GitHub exactly rather than merging.
git fetch --quiet origin
git checkout --quiet --force -B "$branch" "origin/$branch"
echo "Building $(git log -1 --format='%h %s')"

bun install --frozen-lockfile
# .app only: making a .dmg needs the logged-in desktop session, which SSH doesn't have.
bun tauri build --target universal-apple-darwin --bundles app

app="src-tauri/target/universal-apple-darwin/release/bundle/macos/Tightbeam.app"
rm -rf /Users/Shared/Tightbeam.app
cp -R "$app" /Users/Shared/
echo
echo "Done: /Users/Shared/Tightbeam.app"
