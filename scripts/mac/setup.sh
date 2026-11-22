#!/usr/bin/env bash
# One-time toolchain setup for the Mac build account. Safe to rerun.
# Run from Windows with scripts/setup-mac.ps1.
set -euo pipefail

if ! xcode-select -p >/dev/null 2>&1; then
  echo "Xcode command-line tools are missing. From an admin account, run: xcode-select --install" >&2
  exit 1
fi

if [ ! -x "$HOME/.cargo/bin/rustup" ]; then
  echo "Installing Rust..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
# shellcheck disable=SC1091
source "$HOME/.cargo/env"

if [ ! -x "$HOME/.bun/bin/bun" ]; then
  echo "Installing bun..."
  curl -fsSL https://bun.sh/install | bash
fi

echo
echo "Ready: $(rustc --version), bun $("$HOME/.bun/bin/bun" --version)"
