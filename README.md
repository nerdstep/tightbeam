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

**In the background:** closing the window hides it. Tightbeam keeps running in the tray (menu bar on macOS) so messages still arrive, with notifications and an unread count. Quit from the tray icon, or with Cmd+Q on macOS. Opening Tightbeam again brings back the running window. Settings (the gear in the header) toggle notifications and start-at-login, which starts it in the tray.

**Privacy:** the app never asks your router to open a port (UPnP is off), never contacts relay or DNS servers, and only advertises private LAN addresses.

## Layout

```text
ui/src/api.ts             typed wrappers for the Rust commands, plus the shared types
ui/src/store.ts           app state (TanStack Store), fed by events from Rust; history paging
ui/src/components/        the screens: Setup, Pair, Chat, PairDialog, Notice
ui/src/components/ui/     shadcn/ui components (generated; update with the shadcn CLI)
src-tauri/src/chat.rs     the engine: discovery, pairing, protocol, sync
src-tauri/src/db.rs       SQLite storage and history paging
src-tauri/src/lib.rs      Tauri commands and events
src-tauri/tests/          two real engines chatting; paging through 50,000 messages
src-tauri/examples/seed.rs  fills two test profiles with a long synthetic history
```

The UI is React + TypeScript + Tailwind v4 with [shadcn/ui](https://ui.shadcn.com) on Base UI, built with Vite. The transcript uses shadcn's Message Scroller and loads history 200 messages at a time as you scroll up. Theme colors are shadcn's tokens in `ui/src/styles.css` (`bg-background`, `text-muted-foreground`, `bg-primary`, …), with dark-mode values that follow the system. Add components with `bunx shadcn@latest add <name>`.

To try a long history: open both test windows once (`.\scripts\two-windows.ps1 -Fresh`), pair them, close them, then `cd src-tauri; cargo run --example seed -- $env:TEMP\tightbeam-alice $env:TEMP\tightbeam-bob 50000`.

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

For the Rust, in `src-tauri`: `cargo fmt` and `cargo clippy --all-targets`. CI runs all of these checks and the tests on Windows and macOS, for every push to main and every pull request.

**Two copies on one PC.** From the project root, run `.\scripts\two-windows.ps1` (add `-Fresh` to start over). It gives each copy its own data folder and uses loopback, because Windows refuses to send from a machine's LAN address to that same address. Close any running Tightbeam first.

**Start over.** Delete the app's data folder: `%APPDATA%\chat.tightbeam.desktop` on Windows, `~/Library/Application Support/chat.tightbeam.desktop` on macOS.

## Releases

```powershell
.\scripts\release.ps1 0.2.0
```

This sets the version in `src-tauri/Cargo.toml`, the only place it's stored (`tauri.conf.json` falls back to it). It then commits, tags `v0.2.0` and pushes. The tag starts the Release workflow, which builds the Windows installer and the macOS app (Apple Silicon, `.dmg`) into a draft release on GitHub. Check the draft, then publish it.

The builds aren't signed with a paid certificate, so the first install shows a warning:

- **Windows:** SmartScreen. Choose "More info", then "Run anyway".
- **macOS:** open the app once, then go to System Settings → Privacy & Security and choose "Open Anyway".

## Building for macOS by hand

Tauri can't build Mac apps from Windows. To get a Mac build of an unreleased branch, build on a Mac over SSH: `scripts/setup-mac.ps1` prepares the Mac once, and `scripts/build-mac.ps1` builds the `.app` from GitHub. Both expect an SSH alias named `mac-mini`.

`src-tauri/Info.plist` declares the mDNS service and a Local Network usage reason. Without them, macOS silently blocks discovery.
