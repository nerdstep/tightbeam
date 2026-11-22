# One-time: clone the repo into the Mac build account and install Rust and bun there.
# Needs the `mac-mini` SSH alias (see README) and Xcode command-line tools on the Mac.

$ErrorActionPreference = 'Stop'
$repo = 'https://github.com/nerdstep/tightbeam.git'

ssh mac-mini "test -d ~/tightbeam || git clone $repo ~/tightbeam && cd ~/tightbeam && git pull --quiet && bash scripts/mac/setup.sh"
if ($LASTEXITCODE) { throw 'Mac setup failed' }
