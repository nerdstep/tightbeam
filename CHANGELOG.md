# Changelog

Changes people using Tightbeam will notice. Add a line under **Unreleased** as they land. `scripts/release.ps1` renames that section to the new version, and the version's section becomes the text of its GitHub release.

## Unreleased

- Messages stay in the order they were written when the two computers' clocks disagree. Before, a reply could appear above the message it answered if the replier's clock was behind. Messages from older versions are still ordered by their clock.

## 0.4.0 — 2026-11-23

- Replies. Hover a message and choose Reply to answer it; the reply shows the message it answers, and clicking that scrolls to it. Older versions show replies as plain messages.
- Reactions. Hover a message and choose one of six emoji. Each person has one reaction per message: pick another to change it, or the same one to take it back. Reactions sent while the other person is offline arrive when you're both online. Older versions show a reaction as a message holding just the emoji.
- Edit and delete your messages from the ⋯ menu that appears when you hover one, or press ↑ in an empty message box to edit your last message. Edited messages are marked "edited". A deleted message leaves a placeholder, and its text is erased on both computers, not just hidden. Older versions show an edit as a new message, and a delete as an empty one.
- The header shows "Typing…" while the other person is writing.
- A bolder app icon, easier to make out in the tray and taskbar.

## 0.3.0 — 2026-11-23

- Seen receipts. A sent message now shows "seen" after "delivered", once it has been on screen in the other person's Tightbeam. If they run an older version, messages stop at "delivered".
- Slash commands. Type `/` in the message box for a list. To send a message that starts with a slash, begin it with `//`.
  - `/clear` erases the conversation on this computer. The other person keeps their copy, and messages that haven't reached them yet are still delivered.
  - `/nudge` shakes the other person's window and plays a sound. If their window is closed to the tray, they get a notification instead. Older versions ignore nudges.

## 0.2.0 — 2026-11-23

- The settings menu shows which version you're running, and has Quit Tightbeam.
- Automatic updates. Tightbeam downloads new versions from GitHub in the background and offers to restart into them. Turn this off with "Automatic updates" in settings, or check by hand with "Check for updates". This is the first version that can update itself, so install it by hand.

## 0.1.0 — 2026-11-23

First release.

- Find each other on the local network, pair once with a six-digit code, and chat directly over an encrypted connection. No server, no account.
- Full history on both computers. Messages written while the other person is offline are delivered when you're both online again.
- Keeps running in the tray (menu bar on macOS) when the window is closed, with notifications and an unread count.
- Optional start at login; remembers the window's size and position; clickable links.
