# Roadmap

Tightbeam has no server: messages only move while both apps are running and can reach each other. That shapes the priorities below. Features that keep the app running and noticeable come before new chat features.

Items in each section are in suggested order.

## 0. Groundwork

- [x] **Ignore unknown frame types.** `read_loop` in `src-tauri/src/chat.rs` currently ends the session on any frame it doesn't recognize. Two peers will often run different builds, so any new frame type would break the connection to an older peer. Skip unknown frames instead (keep `Hello` / `PairRequest` strict). Do this before anything in sections 2 and 3 adds a frame type.

## 0.5. Long histories and UI foundation

Previously the whole history loaded at startup and every bubble re-rendered on each status event (acks, and mDNS discovery updates every second or two). That was fine for a few thousand messages but would get slow at tens of thousands.

- [x] **Move the UI to React + shadcn/ui (Base UI).** shadcn is React-only, so this replaces Preact. It brings the [Message Scroller](https://ui.shadcn.com/docs/components/base/message-scroller), which pins to the bottom, keeps your place when older messages are added above, and has a jump-to-latest button. Related components: Bubble, Message, and Marker (for day separators). State moves from Preact signals to TanStack Store, which other TanStack libraries (Virtual, Form) would share. The palette maps onto shadcn's tokens.
- [x] **Stop status updates from re-rendering messages.** Bubbles read only their own delivery state and are memoized, so acks and discovery events don't touch the transcript.
- [x] **Paged loading.** `get_messages(before, limit)` in Rust, with an index on `(ts, author, seq)`. Open with the latest ~200 messages and load older pages when the reader nears the top. The Message Scroller handles keeping the scroll position. We write the trigger (an `IntersectionObserver`).
- [x] **Load-test.** Use a generated 50,000-message database to check startup time and typing responsiveness. Result: the newest page loads in ~0.25 ms, and the app opens at the bottom. A window uses ~250 MB in total, mostly WebView2's baseline.
- [x] **Virtualize the transcript** with TanStack Virtual. Only on-screen rows (plus overscan) are in the DOM, so long scroll-backs stay smooth and flat in memory. `anchorTo: 'end'` keeps your place when older pages load, and `followOnAppend` follows new messages. This replaced the earlier `content-visibility` approach (blank rows on fast scrolls) and the plan to cap the loaded window.

## 1. Daily use

- [x] **Keep running in the background.** Closing the window hides it to the tray (Windows) or menu bar (macOS), with Open and Quit items. Add an optional "start at login" (`tauri-plugin-autostart`).
- [x] **Notifications.** Show a system notification for a new message when the window isn't focused (`tauri-plugin-notification`), with a setting to turn it off.
- [x] **Unread count.** Shown in the window title, tray tooltip and tray icon (a dot), plus the Dock badge on macOS or a taskbar dot on Windows. Cleared when the window is focused.
- [x] **Remember window size and position** (`tauri-plugin-window-state`).
- [x] **Clickable links**, opening in the default browser (`tauri-plugin-opener`).
- [ ] **Auto updates** with `tauri-plugin-updater`. The app checks a `latest.json` on GitHub Releases at launch (and every few hours), downloads the signed update, and offers "Update ready — restart". It needs:
  - An updater signing keypair (`tauri signer generate`). The public key goes in `tauri.conf.json`. The private key never goes in the repo; it's stored as the repository secret `TAURI_SIGNING_PRIVATE_KEY`, which the Release workflow passes to `tauri-action`. The action then signs the bundles and uploads `latest.json`.
  - On macOS, the app installed somewhere the person running it can write to. Dragging it from the release `.dmg` to Applications does that. A copy the build account puts in `/Users/Shared` can't replace itself.

## 2. Chat features

New frame types or message kinds. The per-author append-only log stays as it is: changes are new entries that refer to earlier ones, never rewrites.

- [x] **Seen receipts.** `Seen { up_to }`, the same shape as `Ack`, is sent when the window gains focus or a message arrives while it's focused, and again at the start of each session. "Seen" replaces "delivered".
- [x] **Typing indicator.** A `Typing` frame, never stored, sent at most every 2.5 seconds while typing. The other side shows "Typing…" until 4 seconds pass without one, their message arrives, or the connection drops.
- [x] **Reactions.** A log entry: `Msg` with `react_to: (author, seq)` and the emoji as its text. Each person's latest reaction to a message wins, and "" takes it back. Stored in `messages` with `kind = 'reaction'`, so they sync like messages. No notification or unread count for reactions yet.
- [x] **Replies.** `Msg` with an optional `reply_to: (author, seq)`, shown as a quoted snippet that jumps to the original when it's loaded.
- [x] **Edit and delete.** Log entries: `Msg` with `edit_of` (the new text) or `delete_of`, each pointing at one of the sender's own messages; anyone else's are stored and ignored. The latest edit wins, marked "edited", and a delete wins over any edit. Applying a delete blanks the stored text of the message and its edits (with `secure_delete`), on both sides.
- [x] **Slash commands.** Typing `/` at the start of the message box opens a filtered list of commands. Tab completes, an unknown command shows a hint instead of sending, and `//` sends a literal slash. Commands come from a small registry in `ui/src/commands.ts`. So far:
  - `/clear`: erases this computer's copy of the conversation. The peer's copy is untouched, nothing goes over the wire, and sequence numbers carry on (a `cleared` table remembers the highest one erased per author), so sync never asks for the erased messages again.
  - `/nudge`: a `Nudge` frame, live only. A shake and a sound in an open window, a taskbar flash or Dock bounce behind other windows, a notification from the tray. At most one every 3 seconds each way.
  - `/away [note]` and `/back`: see the away status below.
  - `/search [words]`: opens search.
- [ ] **More commands:**
  - `/me waves`: shown as an action line instead of a bubble. Older builds show the raw text.
  - `/shrug` and friends: they only insert text.
- [x] **Search** with SQLite FTS5. The `search` table holds each message's current text, kept up to date as entries are stored: an edit replaces it, and a delete or `/clear` removes it (FTS5's `secure-delete` plus `PRAGMA secure_delete`, so the words don't linger). Each word is a quoted prefix query, so FTS5 syntax typed in is just text. The newest 100 matches, with FTS5 snippets. Picking one loads the gap back to it (`get_messages_between`) and scrolls there.
- [x] **Stable ordering.** Messages used to be sorted by each author's wall clock, so clock skew could misorder them. Each entry now carries `hlc`, a hybrid logical clock: the author's wall clock, or one past the highest `hlc` stored on their side, whichever is later. Anything written after seeing a message sorts after it. `ts` is still the time shown. Entries from peers before 0.5 use `ts`, and `/clear` keeps the clock from running backwards.

## 3. Bigger pieces

- [ ] **Images and files**, including pasted screenshots. Use `iroh-blobs` for transfer. Messages carry a content hash, and the bytes sync separately.
- [x] **Automatic builds.** GitHub Actions (free for public repos, macOS included):
  - `ci.yml` runs the UI checks, plus rustfmt, clippy and the tests on Windows and macOS, for each push to main and each pull request.
  - `release.yml` runs CI on the tagged commit, then builds the Windows installer and the macOS `.dmg` into a draft release when a `v*` tag is pushed (`scripts/release.ps1`). The release text comes from `CHANGELOG.md`.
  - Dependabot sends monthly update PRs, including for the actions, which are pinned to commit hashes.
- [ ] **Pre-release channel.** Tags like `v0.2.0-beta.1` publish as pre-releases, which the updater ignores, so a build can be tried by hand before it reaches everyone. `release.ps1` needs to accept that version format.
- [ ] **Check the built files before upload.** On macOS: a valid signature, the Local Network keys in `Info.plist`, and the version string. On Windows: the installer's version.
- [ ] **Mac builds of any branch from Actions** (manual run, `.dmg` attached to the run), replacing the SSH build.
  - Building over SSH remains for one-off Mac builds of unreleased branches.
- [ ] **Off-network chat, opt-in.** Enable iroh's relay servers so peers can reach each other off the LAN. Messages stay end-to-end encrypted, but the relay operator can see that the two endpoints talk. Off by default, with a clear setting.

## Later

- [ ] **More than one device per person** (for example a laptop plus a desktop, and eventually phones via Tauri mobile). This is the biggest architectural change: each person's log has to sync across their own devices, and pairing becomes per person rather than per device.
- [ ] Voice notes.
- [x] **Away status.** `Away { note }` and `Back` frames, live only, sent on change and at the start of each session. After 10 minutes without input anywhere on the computer (`GetLastInputInfo` on Windows, `CGEventSourceSecondsSinceLastEventType` on macOS, checked every 5 seconds), you're away until input resumes. `/away [note]` sets it by hand, and it lasts until `/back` or your next message. The header shows the other person as "Away", with the note if there is one.
- [ ] Per-person colors or themes.
