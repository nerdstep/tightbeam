# Changelog

Changes people using Tightbeam will notice. Add a line under **Unreleased** as they land. `scripts/release.ps1` renames that section to the new version, and the version's section becomes the text of its GitHub release.

## Unreleased

- Replies, quotes and search show /me messages the way the conversation does ("Alex waves"), instead of the raw text. A quote of a deleted message says "Message deleted", as the message itself does.
- Send any file, not just pictures: pick files with the paperclip, or drop them on the window. Files up to 1 GB go straight from disk, so big ones don't slow the app down. Changing or moving the original afterwards doesn't affect what was sent. The other side needs version 0.7 or later.
- More kinds of received files open in their app with one click: PDFs, text and Office documents, audio and video, as well as pictures. Programs, scripts, web pages and archives can only be shown in their folder.
- After unpairing and pairing with someone new, they get only the messages written from then on. Before, they also received every message sent to the previous person. Editing or deleting those older messages stays on your computer.
- Messages from someone you were paired with before keep their name, in the conversation, in search and when replying, instead of showing the name of whoever you're paired with now.
- Clicking the quote in a reply jumps to the original message even when it's further back than the history loaded so far. Before, it did nothing until you'd scrolled back that far.
- A reply quoting a vanishing message shows "Message vanished" as soon as it vanishes, instead of "Deleted message" until the app restarts.
- A problem with what you're sending, like a file that's too big or a message that didn't go, shows just above the message box instead of at the top of the window.

## 0.7.0 — 2026-11-24

- Pictures: paste a screenshot, drop pictures on the window, or pick them with the new button beside the message box, with an optional caption. Click one to see it full size, open it, save a copy, or show it in its folder. Pictures can be up to 1 GB, and a transfer that's interrupted picks up where it stopped. Both computers need this version or later.
- Windows: after an update, the Start menu, desktop and taskbar show the app's current icon instead of an old cached one.

## 0.6.0 — 2026-11-24

- More commands:
  - `/vanish 1m your message` sends a message that erases itself from both computers once the time is up, counting from when the other person sees it. It shows a countdown, and its notification doesn't include the text. It needs this version or later on both computers.
  - `/dnd` turns on do not disturb for an hour, or for as long as you say (`/dnd 30m`, `/dnd 2h`); `/dnd off` ends it. No notifications or nudge sounds, the other person sees "Do not disturb", and their nudges are paused.
  - `/me waves` shows as "Alex waves" rather than a bubble.
  - `/shrug`, `/tableflip` and `/unflip` send ¯\\_(ツ)\_/¯, (╯°□°)╯︵ ┻━┻ and ┬─┬ノ( º \_ºノ), after anything you type with them.
  - `/status` shows the connection's address and round-trip time, and which version each app runs.

## 0.5.0 — 2026-11-24

- Messages stay in the order they were written when the two computers' clocks disagree. Before, a reply could appear above the message it answered if the replier's clock was behind. Messages from older versions are still ordered by their clock.
- Away status. After 10 minutes without keyboard or mouse input on your computer, the other person sees you as "Away" until you're back. Type `/away` to set it yourself, with an optional note like `/away lunch`; it lasts until `/back` or your next message. Older versions don't show it.
- Search. Click the magnifier in the header, press Ctrl+F (Cmd+F on macOS), or type `/search` and some words. Matches are listed newest first with the words highlighted; pick one to jump to it. Edited messages are found by their new text, and deleted ones aren't found.

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
