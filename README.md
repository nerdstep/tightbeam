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
ui/                  the interface (plain HTML/CSS/JS, no build step)
src-tauri/src/chat.rs  the engine: discovery, pairing, protocol, sync
src-tauri/src/db.rs    SQLite storage
src-tauri/src/lib.rs   Tauri commands and events
src-tauri/tests/       two real engines pairing and chatting on one machine
prototype/           the original Node/browser proof of concept
```

## Develop

Needs Rust (via rustup) and the Tauri CLI (`cargo install tauri-cli --version "^2" --locked`).

```powershell
cd src-tauri
cargo tauri dev        # run the app
cargo test             # run the two-peer test (about 5 seconds)
cargo tauri build      # make an installer in target/release/bundle/
```

**Two copies on one PC.** From the project root, run `.\scripts\two-windows.ps1` (add `-Fresh` to start over). It gives each copy its own data folder and uses loopback, because Windows refuses to send from a machine's LAN address to that same address. Close any running Tightbeam first.

**Start over.** Delete the app's data folder: `%APPDATA%\chat.tightbeam.desktop` on Windows, `~/Library/Application Support/chat.tightbeam.desktop` on macOS.

## macOS notes

- The Mac version has to be built on a Mac. Tauri can't build it from Windows.
- `src-tauri/Info.plist` declares the mDNS service and a Local Network usage reason. Without them, macOS silently blocks discovery.
- The app is unsigned, so the first launch needs right-click → Open.
