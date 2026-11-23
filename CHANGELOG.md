# Changelog

Changes people using Tightbeam will notice. Add a line under **Unreleased** as they land. `scripts/release.ps1` renames that section to the new version, and the version's section becomes the text of its GitHub release.

## Unreleased

- Seen receipts. A sent message now shows "seen" after "delivered", once it has been on screen in the other person's Tightbeam. If they run an older version, messages stop at "delivered".

## 0.2.0 — 2026-11-23

- The settings menu shows which version you're running, and has Quit Tightbeam.
- Automatic updates. Tightbeam downloads new versions from GitHub in the background and offers to restart into them. Turn this off with "Automatic updates" in settings, or check by hand with "Check for updates". This is the first version that can update itself, so install it by hand.

## 0.1.0 — 2026-11-23

First release.

- Find each other on the local network, pair once with a six-digit code, and chat directly over an encrypted connection. No server, no account.
- Full history on both computers. Messages written while the other person is offline are delivered when you're both online again.
- Keeps running in the tray (menu bar on macOS) when the window is closed, with notifications and an unread count.
- Optional start at login; remembers the window's size and position; clickable links.
