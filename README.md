# Tightbeam

A private line between two people on the same network. There's no server and no account. The two apps find each other on the local network, pair once, and talk directly over an encrypted connection. Each side keeps the full history locally.

Windows and macOS. Rust + [Tauri](https://tauri.app) for the app, [iroh](https://iroh.computer) for networking, SQLite for history.

## How it works

- **Identity.** On first launch the app creates a keypair, stored in its SQLite file. The public key is your permanent ID.
- **Discovery.** Each app announces itself on the LAN over mDNS (`_tightbeam._udp.local`), along with its display name.
- **Pairing.** You click a nearby person, and both screens show the same six-digit code, derived from both keys. The other person accepts. From then on, each app only talks to that one key.
- **Connection.** Once paired, the app with the lower ID dials the other every few seconds until connected. The link is QUIC with TLS, authenticated by those keys.
- **Sync.** Messages are numbered per author. On connect, each side says "the last one I have from you is #N", and the other sends everything after it. Duplicates are ignored, so resending is always safe.
- **Delivery.** Your message shows "waiting" until the other app confirms it has it, then "delivered".

**Privacy:** the app never asks your router to open a port (UPnP is off), never contacts relay or DNS servers, and only advertises private LAN addresses.

## Layout

```text
ui/src/api.ts           typed wrappers for the Rust commands, plus the shared types
ui/src/store.ts         app state (Preact signals), fed by events from Rust
ui/src/components/      the screens: Setup, Pair, Chat, PairDialog, Notice
src-tauri/src/chat.rs   the engine: discovery, pairing, protocol, sync
src-tauri/src/db.rs     SQLite storage
src-tauri/src/lib.rs    Tauri commands and events
src-tauri/tests/        two real engines pairing and chatting on one machine
```

The UI is Preact + TypeScript + Tailwind v4, built with Vite. Theme colors are tokens in `ui/src/styles.css` (`bg-canvas`, `text-ink`, `border-line`, …), with dark-mode values that follow the system.

## Develop

Needs Rust (via rustup) and bun. The Tauri CLI comes with `bun install`.

```powershell
bun install            # once
bun tauri dev          # run the app; the UI hot-reloads as you edit
bun tauri build        # make an installer in src-tauri/target/release/bundle/
cd src-tauri; cargo test   # run the two-peer test (about 5 seconds)
```

`bun run dev` alone starts only Vite. The UI needs the Tauri bridge, so it won't work in a regular browser.

For the TypeScript, from the project root:

```powershell
bun run check          # type-check, lint (oxlint) and format check (oxfmt)
bun run fmt            # format
```

**Two copies on one PC.** From the project root, run `.\scripts\two-windows.ps1` (add `-Fresh` to start over). It gives each copy its own data folder and uses loopback, because Windows refuses to send from a machine's LAN address to that same address. Close any running Tightbeam first.

**Start over.** Delete the app's data folder: `%APPDATA%\chat.tightbeam.desktop` on Windows, `~/Library/Application Support/chat.tightbeam.desktop` on macOS.

## Building for macOS

Tauri can't build Mac apps from Windows. Mac builds run on a Mac over SSH: `scripts/setup-mac.ps1` prepares it once, and `scripts/build-mac.ps1` builds the `.app` (Apple Silicon) from GitHub. Both expect an SSH alias named `mac-mini`.

`src-tauri/Info.plist` declares the mDNS service and a Local Network usage reason. Without them, macOS silently blocks discovery.
